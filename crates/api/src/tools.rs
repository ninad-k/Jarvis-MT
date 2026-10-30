//! LLM tools. SPEC rule 1: the model can only call read tools (and, from Sprint 2,
//! `propose_*` tools). Nothing here can write to the bridge.
//!
//! The registry is built on the closed [`ReadTool`] enum: there is no `register()`
//! method, so a write tool cannot be added by accident. Adding a tool means adding an
//! enum variant, which a reviewer sees.

use std::sync::Arc;

use chrono::{DateTime, NaiveDate};
use jarvis_bridge_client::protocol::{
    Command, Deal, DealType, HISTORY_LIMIT_MAX, History, HistoryQuery, Orders, Positions, Price,
    Side,
};
use jarvis_bridge_client::{BridgeClient, BridgeError};
use jarvis_llm::ToolSpec;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::clock::ServerClock;
use crate::symbols::SymbolResolver;

/// Deals returned to the model when it does not pass `limit`.
const HISTORY_DEFAULT_LIMIT: u32 = 50;

/// The complete set of tools the model may call. Read-only by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadTool {
    GetPrice,
    GetAccount,
    GetPositions,
    GetOrders,
    GetHistory,
}

impl ReadTool {
    pub const ALL: [ReadTool; 5] = [
        ReadTool::GetPrice,
        ReadTool::GetAccount,
        ReadTool::GetPositions,
        ReadTool::GetOrders,
        ReadTool::GetHistory,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ReadTool::GetPrice => "get_price",
            ReadTool::GetAccount => "get_account",
            ReadTool::GetPositions => "get_positions",
            ReadTool::GetOrders => "get_orders",
            ReadTool::GetHistory => "get_history",
        }
    }

    pub fn from_name(name: &str) -> Option<ReadTool> {
        ReadTool::ALL.into_iter().find(|t| t.name() == name)
    }

    fn description(self) -> &'static str {
        match self {
            ReadTool::GetPrice => {
                "Get the current bid/ask price of one symbol from MetaTrader 5. You may pass the \
                 user's own word (for example \"gold\", \"nasdaq\", \"dow jones\") or a broker \
                 symbol (\"XAUUSD\"); aliases are resolved for you. Always call this for any \
                 question about a current price."
            }
            ReadTool::GetAccount => {
                "Get the trading account summary: currency, balance, equity, floating profit, \
                 used margin, free margin, margin level (percent) and leverage. Takes no \
                 arguments. Never includes login or server details."
            }
            ReadTool::GetPositions => {
                "List the currently open positions (ticket, symbol, side, volume, open price, \
                 current price, stop loss, take profit, profit, swap). Optionally filter by \
                 symbol (aliases accepted) and/or side (buy or sell). Includes the count and \
                 total profit of the returned positions."
            }
            ReadTool::GetOrders => {
                "List the pending (not yet filled) orders: ticket, symbol, type, volume, price, \
                 stop loss, take profit. Optionally filter by symbol (aliases accepted)."
            }
            ReadTool::GetHistory => {
                "List closed-trade history (deals), newest first, between two dates. `from` and \
                 `to` are dates (YYYY-MM-DD, in the broker's server time) or full RFC 3339 \
                 timestamps; when `to` is a date it means the end of that day. Use the same date \
                 for both to get one day. Returns up to `limit` deals and says if it was \
                 truncated. Deal times in the result are broker server time. Optionally filter \
                 by symbol. `summary` holds totals computed over every deal in the range \
                 (net = profit + commission + swap, overall and per symbol): quote those \
                 numbers for any total and never add up deals yourself. `summary.complete` is \
                 false when the range held more than 500 deals."
            }
        }
    }

    fn schema(self) -> Value {
        match self {
            ReadTool::GetPrice => json!({
                "type": "object",
                "properties": {
                    "symbol": {
                        "type": "string",
                        "description": "Symbol or common name, e.g. \"gold\", \"XAUUSD\", \"nasdaq\"."
                    }
                },
                "required": ["symbol"],
                "additionalProperties": false
            }),
            ReadTool::GetAccount => json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            ReadTool::GetPositions => json!({
                "type": "object",
                "properties": {
                    "symbol": {
                        "type": "string",
                        "description": "Only positions on this symbol (optional)."
                    },
                    "side": {
                        "type": "string",
                        "enum": ["buy", "sell"],
                        "description": "Only long (buy) or short (sell) positions (optional)."
                    }
                },
                "additionalProperties": false
            }),
            ReadTool::GetOrders => json!({
                "type": "object",
                "properties": {
                    "symbol": {
                        "type": "string",
                        "description": "Only pending orders on this symbol (optional)."
                    }
                },
                "additionalProperties": false
            }),
            ReadTool::GetHistory => json!({
                "type": "object",
                "properties": {
                    "from": {
                        "type": "string",
                        "description": "Start: YYYY-MM-DD (start of that server-time day) or RFC 3339."
                    },
                    "to": {
                        "type": "string",
                        "description": "End: YYYY-MM-DD (end of that server-time day) or RFC 3339."
                    },
                    "symbol": {
                        "type": "string",
                        "description": "Only deals on this symbol (optional)."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": HISTORY_LIMIT_MAX,
                        "description": "Maximum number of deals (default 50)."
                    }
                },
                "required": ["from", "to"],
                "additionalProperties": false
            }),
        }
    }

    pub fn spec(self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_owned(),
            description: self.description().to_owned(),
            parameters: self.schema(),
        }
    }
}

/// What came back from executing one tool call.
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    /// Text given to the model (compact JSON, or an error message).
    pub content: String,
    pub is_error: bool,
    /// The bridge command sent, for the audit log (`None` if nothing was sent).
    pub bridge_request: Option<Value>,
    /// The bridge reply or error, for the audit log.
    pub bridge_reply: Option<Value>,
}

impl ToolOutcome {
    fn error(message: impl Into<String>) -> Self {
        Self {
            content: message.into(),
            is_error: true,
            bridge_request: None,
            bridge_reply: None,
        }
    }
}

/// Executes the read tools against the bridge. Cheap to clone.
#[derive(Clone)]
pub struct ToolRegistry {
    bridge: BridgeClient,
    resolver: Arc<SymbolResolver>,
    clock: ServerClock,
}

impl ToolRegistry {
    pub fn new(bridge: BridgeClient, resolver: Arc<SymbolResolver>, clock: ServerClock) -> Self {
        Self {
            bridge,
            resolver,
            clock,
        }
    }

    /// Tool specs sent to the model: exactly the five read tools.
    pub fn specs(&self) -> Vec<ToolSpec> {
        ReadTool::ALL.into_iter().map(ReadTool::spec).collect()
    }

    /// Run one tool call. Never panics; every failure becomes an `is_error` outcome
    /// with a message the model can relay to the user.
    pub async fn execute(&self, name: &str, arguments: &Value) -> ToolOutcome {
        let Some(tool) = ReadTool::from_name(name) else {
            return ToolOutcome::error(format!(
                "unknown tool '{name}'. Available tools: {}",
                ReadTool::ALL.map(ReadTool::name).join(", ")
            ));
        };
        let args = if arguments.is_null() {
            json!({})
        } else {
            arguments.clone()
        };
        let mut exchange = Exchange::default();
        let result = match tool {
            ReadTool::GetPrice => self.get_price(args, &mut exchange).await,
            ReadTool::GetAccount => self.get_account(&mut exchange).await,
            ReadTool::GetPositions => self.get_positions(args, &mut exchange).await,
            ReadTool::GetOrders => self.get_orders(args, &mut exchange).await,
            ReadTool::GetHistory => self.get_history(args, &mut exchange).await,
        };
        let (content, is_error) = match result {
            Ok(content) => (content, false),
            Err(message) => (message, true),
        };
        ToolOutcome {
            content,
            is_error,
            bridge_request: exchange.request,
            bridge_reply: exchange.reply,
        }
    }

    fn symbol(&self, raw: &str) -> Result<String, String> {
        self.resolver.resolve(raw).ok_or_else(|| {
            format!("'{raw}' is not a usable symbol name. Ask the user which instrument they mean.")
        })
    }

    fn optional_symbol(&self, raw: Option<&str>) -> Result<Option<String>, String> {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(None),
            Some(s) => self.symbol(s).map(Some),
        }
    }

    async fn get_price(&self, args: Value, ex: &mut Exchange) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Args {
            symbol: String,
        }
        let Args { symbol: raw } = parse_args(args, "get_price")?;
        let symbol = self.symbol(&raw)?;
        let price: Price = self
            .call(
                Command::GetPrice {
                    symbol: symbol.clone(),
                },
                ex,
                Some(&raw),
            )
            .await?;
        compact(&price)
    }

    async fn get_account(&self, ex: &mut Exchange) -> Result<String, String> {
        // `Account` has no login/name/server field (rule 8): nothing to redact, by type.
        let account: jarvis_bridge_client::protocol::Account =
            self.call(Command::GetAccount {}, ex, None).await?;
        compact(&account)
    }

    async fn get_positions(&self, args: Value, ex: &mut Exchange) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Args {
            symbol: Option<String>,
            side: Option<String>,
        }
        let Args { symbol: raw, side } = parse_args(args, "get_positions")?;
        let side = parse_side(side.as_deref())?;
        let symbol = self.optional_symbol(raw.as_deref())?;
        let positions: Positions = self
            .call(Command::GetPositions { symbol }, ex, raw.as_deref())
            .await?;
        let mut list = positions.positions;
        if let Some(side) = side {
            list.retain(|p| p.side == side);
        }
        let total_profit = round2(list.iter().map(|p| p.profit).sum());
        compact(&json!({ "count": list.len(), "total_profit": total_profit, "positions": list }))
    }

    async fn get_orders(&self, args: Value, ex: &mut Exchange) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Args {
            symbol: Option<String>,
        }
        let Args { symbol: raw } = parse_args(args, "get_orders")?;
        let symbol = self.optional_symbol(raw.as_deref())?;
        let orders: Orders = self
            .call(Command::GetOrders { symbol }, ex, raw.as_deref())
            .await?;
        compact(&json!({ "count": orders.orders.len(), "orders": orders.orders }))
    }

    async fn get_history(&self, args: Value, ex: &mut Exchange) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Args {
            from: String,
            to: String,
            symbol: Option<String>,
            limit: Option<u32>,
        }
        let a: Args = parse_args(args, "get_history")?;
        let offset = self.clock.offset_secs();
        let from =
            parse_time(&a.from, false, offset).map_err(|e| format!("invalid 'from': {e}"))?;
        let to = parse_time(&a.to, true, offset).map_err(|e| format!("invalid 'to': {e}"))?;
        if from > to {
            return Err(format!("'from' ({}) is after 'to' ({})", a.from, a.to));
        }
        let symbol = self.optional_symbol(a.symbol.as_deref())?;
        let limit = a
            .limit
            .unwrap_or(HISTORY_DEFAULT_LIMIT)
            .clamp(1, HISTORY_LIMIT_MAX) as usize;
        // Always fetch the maximum so the totals cover the whole range; only the newest
        // `limit` deals are listed to the model. The model must never sum P/L itself.
        let query = HistoryQuery {
            from,
            to,
            symbol,
            limit: Some(HISTORY_LIMIT_MAX),
        };
        let history: History = self
            .call(Command::GetHistory(query), ex, a.symbol.as_deref())
            .await?;
        let summary = summarize_deals(&history.deals, history.truncated);
        let listed: Vec<&Deal> = history.deals.iter().take(limit).collect();
        compact(&json!({
            "summary": summary,
            "listed": listed.len(),
            "deals": listed,
        }))
    }

    /// One bridge round trip, recorded in `ex` for the audit log.
    async fn call<T: DeserializeOwned + Serialize>(
        &self,
        command: Command,
        ex: &mut Exchange,
        user_symbol: Option<&str>,
    ) -> Result<T, String> {
        ex.request = serde_json::to_value(&command).ok();
        match self.bridge.call::<T>(command).await {
            Ok(value) => {
                ex.reply = serde_json::to_value(&value).ok();
                Ok(value)
            }
            Err(e) => {
                ex.reply = Some(json!({ "error": e.to_string() }));
                Err(bridge_error_message(&e, user_symbol))
            }
        }
    }
}

#[derive(Default)]
struct Exchange {
    request: Option<Value>,
    reply: Option<Value>,
}

fn parse_args<T: DeserializeOwned>(args: Value, tool: &str) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| {
        format!("invalid arguments for {tool}: {e}. Check the tool's parameter schema and retry.")
    })
}

fn parse_side(side: Option<&str>) -> Result<Option<Side>, String> {
    match side.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") => Ok(None),
        Some("buy" | "long") => Ok(Some(Side::Buy)),
        Some("sell" | "short") => Ok(Some(Side::Sell)),
        Some(other) => Err(format!("invalid side '{other}': use \"buy\" or \"sell\"")),
    }
}

/// -> MT5 server-time Unix seconds. `YYYY-MM-DD` is a server-local date (start or end of that
/// day); an RFC 3339 timestamp is a real instant, shifted by the server's UTC offset.
fn parse_time(text: &str, end_of_day: bool, server_offset_secs: i64) -> Result<i64, String> {
    let text = text.trim();
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        let dt = if end_of_day {
            date.and_hms_opt(23, 59, 59)
        } else {
            date.and_hms_opt(0, 0, 0)
        };
        return dt
            .map(|dt| dt.and_utc().timestamp())
            .ok_or_else(|| format!("'{text}' is not a valid date"));
    }
    DateTime::parse_from_rfc3339(text)
        .map(|dt| dt.timestamp() + server_offset_secs)
        .map_err(|_| format!("'{text}' is neither YYYY-MM-DD nor an RFC 3339 timestamp"))
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Totals over trade deals (buy/sell; balance and credit deals are excluded), so the
/// model quotes computed numbers instead of doing arithmetic. Per-symbol rows are
/// sorted by symbol for stable output.
fn summarize_deals(deals: &[Deal], truncated: bool) -> Value {
    #[derive(Default)]
    struct Totals {
        deals: usize,
        profit: f64,
        commission: f64,
        swap: f64,
    }
    impl Totals {
        fn add(&mut self, d: &Deal) {
            self.deals += 1;
            self.profit += d.profit;
            self.commission += d.commission;
            self.swap += d.swap;
        }
        fn to_json(&self) -> Value {
            json!({
                "deals": self.deals,
                "net_profit": round2(self.profit + self.commission + self.swap),
                "gross_profit": round2(self.profit),
                "commission": round2(self.commission),
                "swap": round2(self.swap),
            })
        }
    }

    let mut total = Totals::default();
    let mut by_symbol: std::collections::BTreeMap<&str, Totals> = Default::default();
    for d in deals
        .iter()
        .filter(|d| matches!(d.deal_type, DealType::Buy | DealType::Sell))
    {
        total.add(d);
        by_symbol.entry(d.symbol.as_str()).or_default().add(d);
    }
    let per_symbol: Vec<Value> = by_symbol
        .iter()
        .map(|(symbol, t)| {
            let mut row = t.to_json();
            row["symbol"] = json!(symbol);
            row
        })
        .collect();
    let mut summary = total.to_json();
    summary["complete"] = json!(!truncated);
    summary["by_symbol"] = json!(per_symbol);
    summary
}

fn compact<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| format!("could not encode tool result: {e}"))
}

/// Message for the model. It is written so the model can relay it to the user.
fn bridge_error_message(e: &BridgeError, user_symbol: Option<&str>) -> String {
    match e {
        BridgeError::Timeout(_) | BridgeError::Transport(_) => format!(
            "MetaTrader 5 is not reachable right now ({e}). Tell the user the MT5 bridge looks \
             offline; do not guess any values."
        ),
        BridgeError::Remote { message, .. }
            if message.to_lowercase().contains("unknown symbol") =>
        {
            let name = user_symbol.unwrap_or("that symbol");
            format!(
                "{message}. The broker has no symbol matching '{name}'. Ask the user to check \
                 the symbol name (it may need a broker-specific name or suffix)."
            )
        }
        BridgeError::Remote { .. } | BridgeError::Protocol(_) => {
            format!("MT5 bridge error: {e}. Tell the user the request failed; do not guess values.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_bridge_client::protocol::DealEntry;

    fn deal(symbol: &str, deal_type: DealType, profit: f64, commission: f64, swap: f64) -> Deal {
        Deal {
            ticket: 1,
            order: 1,
            position_id: 1,
            symbol: symbol.to_owned(),
            deal_type,
            entry: DealEntry::Out,
            volume: 0.1,
            price: 1.0,
            profit,
            commission,
            swap,
            time: 0,
            comment: String::new(),
        }
    }

    #[test]
    fn history_summary_totals_trade_deals_per_symbol() {
        let deals = [
            deal("XAUUSD", DealType::Sell, -100.0, -1.5, -0.25),
            deal("XAUUSD", DealType::Buy, 40.0, -1.5, 0.0),
            deal("GBPUSD", DealType::Buy, 45.54, -0.32, -2.01),
            deal("", DealType::Balance, 10_000.0, 0.0, 0.0), // deposit: excluded
        ];
        let s = summarize_deals(&deals, false);
        assert_eq!(s["deals"], 3);
        assert_eq!(s["gross_profit"], -14.46);
        assert_eq!(s["net_profit"], -20.04);
        assert_eq!(s["complete"], true);
        let rows = s["by_symbol"].as_array().unwrap();
        assert_eq!(rows[0]["symbol"], "GBPUSD");
        assert_eq!(rows[0]["net_profit"], 43.21);
        assert_eq!(rows[1]["symbol"], "XAUUSD");
        assert_eq!(rows[1]["deals"], 2);
        assert_eq!(rows[1]["net_profit"], -63.25);
        assert_eq!(summarize_deals(&deals, true)["complete"], false);
    }

    #[test]
    fn specs_are_exactly_the_five_read_tools() {
        let names: Vec<String> = ReadTool::ALL.into_iter().map(|t| t.spec().name).collect();
        assert_eq!(
            names,
            [
                "get_price",
                "get_account",
                "get_positions",
                "get_orders",
                "get_history"
            ]
        );
        for n in &names {
            for banned in [
                "propose_",
                "order_",
                "position_",
                "close",
                "modify",
                "cancel",
                "send",
            ] {
                assert!(!n.starts_with(banned), "{n} looks like a write tool");
            }
        }
    }

    #[test]
    fn every_spec_has_description_and_object_schema() {
        for t in ReadTool::ALL {
            let spec = t.spec();
            assert!(spec.description.len() > 40, "{}", spec.name);
            assert_eq!(spec.parameters["type"], "object", "{}", spec.name);
            assert!(spec.parameters.get("properties").is_some(), "{}", spec.name);
            assert_eq!(ReadTool::from_name(&spec.name), Some(t));
        }
        assert_eq!(
            ReadTool::GetPrice.spec().parameters["required"],
            json!(["symbol"])
        );
        assert_eq!(
            ReadTool::GetHistory.spec().parameters["required"],
            json!(["from", "to"])
        );
    }

    #[test]
    fn unknown_tool_name_is_not_a_read_tool() {
        assert_eq!(ReadTool::from_name("order_send"), None);
        assert_eq!(ReadTool::from_name("propose_order"), None);
    }

    #[test]
    fn parse_time_date_and_rfc3339() {
        assert_eq!(parse_time("2026-10-30", false, 0), Ok(1_793_318_400));
        assert_eq!(
            parse_time("2026-10-30", true, 0),
            Ok(1_793_318_400 + 86_399)
        );
        assert_eq!(
            parse_time("2026-10-30T12:00:00Z", true, 0),
            Ok(1_793_318_400 + 43_200)
        );
        assert_eq!(
            parse_time("2026-10-30T12:00:00+02:00", false, 0),
            Ok(1_793_318_400 + 36_000)
        );
        // Server at UTC+3: dates are already server-local, instants are shifted.
        assert_eq!(parse_time("2026-10-30", false, 10_800), Ok(1_793_318_400));
        assert_eq!(
            parse_time("2026-10-30T12:00:00Z", false, 10_800),
            Ok(1_793_318_400 + 43_200 + 10_800)
        );
        assert!(parse_time("yesterday", false, 0).is_err());
        assert!(parse_time("2026-13-40", false, 0).is_err());
    }

    #[test]
    fn side_parsing() {
        assert_eq!(parse_side(None), Ok(None));
        assert_eq!(parse_side(Some("BUY")), Ok(Some(Side::Buy)));
        assert_eq!(parse_side(Some("short")), Ok(Some(Side::Sell)));
        assert!(parse_side(Some("hold")).is_err());
    }

    #[test]
    fn unknown_symbol_error_suggests_checking_the_name() {
        let e = BridgeError::Remote {
            message: "unknown symbol: XAUUS".into(),
            retcode: 4301,
        };
        let msg = bridge_error_message(&e, Some("goldd"));
        assert!(msg.contains("check the symbol name"), "{msg}");
        let down = bridge_error_message(&BridgeError::Timeout(3000), None);
        assert!(down.contains("not reachable"), "{down}");
    }
}
