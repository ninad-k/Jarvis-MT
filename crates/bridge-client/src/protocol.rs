//! Wire protocol between the Rust backend and the `JarvisBridge` MQL5 EA.
//!
//! This module is the source of truth for the JSON contract. `docs/protocol.md`
//! and the golden files in `tests/fixtures/` must match it.
//!
//! Request:  `{"id": "...", "ts": 1793341200000, "cmd": "get_price", "args": {"symbol": "XAUUSD"}}`
//! Reply:    `{"id": "...", "ok": true,  "data": {...},          "retcode": 0, "ts": 1793341200012}`
//!           `{"id": "...", "ok": false, "error": "message",     "retcode": 4301, "ts": 1793341200012}`
//!
//! Each message travels as two ZeroMQ frames, `[hex HMAC-SHA256, this JSON]`; see [`crate::auth`].

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

/// Protocol version reported by the EA in `ping`. Bump on breaking changes.
/// Version 2: every message is a signed two-frame message (ADR-0016).
pub const PROTOCOL_VERSION: u32 = 2;

/// Retcode used by the EA for malformed requests or invalid arguments.
pub const RETCODE_INVALID_REQUEST: i64 = -1;
/// Retcode used by the EA for an unknown or not-yet-implemented command.
pub const RETCODE_UNKNOWN_COMMAND: i64 = -2;
/// Retcode used by the EA when a request failed authentication: wrong or missing MAC, wrong
/// frame count, stale `ts`, or an `id` that was already used. The error text is always
/// [`crate::auth::AUTH_ERROR_TEXT`].
pub const RETCODE_AUTH_FAILED: i64 = -4;

/// Maximum number of deals `get_history` may return in one reply.
pub const HISTORY_LIMIT_MAX: u32 = 500;
/// Default `get_history` limit when none is given.
pub const HISTORY_LIMIT_DEFAULT: u32 = 100;

/// Most bars `get_rates` may return in one reply.
pub const RATES_COUNT_MAX: u32 = 1000;
/// Default `get_rates` count when none is given.
pub const RATES_COUNT_DEFAULT: u32 = 300;
/// Longest time range `get_calendar` accepts, in seconds (14 days).
pub const CALENDAR_RANGE_MAX_SECS: i64 = 14 * 24 * 3600;
/// Most events one `get_calendar` reply carries (the EA keeps the earliest ones).
pub const CALENDAR_EVENTS_MAX: usize = 500;

/// A command plus its arguments. Serialises as `{"cmd": "...", "args": {...}}`.
///
/// Sprint 1 contains read commands only. Write commands (`order_send`, ...)
/// are added in Sprint 2 behind the action gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "args", rename_all = "snake_case")]
pub enum Command {
    Ping {},
    GetPrice {
        symbol: String,
    },
    GetAccount {},
    GetPositions {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        symbol: Option<String>,
    },
    GetOrders {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        symbol: Option<String>,
    },
    GetHistory(HistoryQuery),
    /// Trading specification of one symbol (lot step, tick value, stops level...).
    GetSymbol {
        symbol: String,
    },
    /// Which broker and server the terminal is logged into (for per-broker symbol
    /// maps). Never includes the login, the account name or any credential.
    GetBroker {},
    /// One page of the broker's FULL symbol catalog (all symbols the server offers,
    /// not only Market Watch), ordered by name.
    ListSymbols(ListSymbolsQuery),
    /// Price bars (OHLC, tick volume, spread) of one symbol and timeframe, oldest first.
    GetRates(RatesQuery),
    /// Economic calendar events from MT5's built-in calendar.
    GetCalendar(CalendarQuery),

    // ---- Write commands (Sprint 2). Only `gate`-approved, user-confirmed requests
    // ---- may produce these. The EA refuses them on real accounts unless its
    // ---- `InpAllowReal` input is true, and caches replies by request `id` so a
    // ---- resent id never executes twice.
    OrderSend(OrderSendArgs),
    PositionModify(PositionModifyArgs),
    PositionClose(PositionCloseArgs),
    PositionClosePartial(PositionClosePartialArgs),
    OrderCancel(OrderCancelArgs),
}

impl Command {
    /// The wire name of the command, e.g. `"get_price"`.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Ping {} => "ping",
            Command::GetPrice { .. } => "get_price",
            Command::GetAccount {} => "get_account",
            Command::GetPositions { .. } => "get_positions",
            Command::GetOrders { .. } => "get_orders",
            Command::GetHistory(_) => "get_history",
            Command::GetSymbol { .. } => "get_symbol",
            Command::GetBroker {} => "get_broker",
            Command::ListSymbols(_) => "list_symbols",
            Command::GetRates(_) => "get_rates",
            Command::GetCalendar(_) => "get_calendar",
            Command::OrderSend(_) => "order_send",
            Command::PositionModify(_) => "position_modify",
            Command::PositionClose(_) => "position_close",
            Command::PositionClosePartial(_) => "position_close_partial",
            Command::OrderCancel(_) => "order_cancel",
        }
    }

    /// True for commands that change account state. These are never retried.
    pub fn is_write(&self) -> bool {
        matches!(
            self,
            Command::OrderSend(_)
                | Command::PositionModify(_)
                | Command::PositionClose(_)
                | Command::PositionClosePartial(_)
                | Command::OrderCancel(_)
        )
    }
}

/// Magic number stamped on every order Jarvis sends, so its trades are identifiable.
pub const JARVIS_MAGIC: i64 = 770_077;

/// MT5 trade retcodes that mean the request succeeded.
pub const TRADE_RETCODE_PLACED: i64 = 10008;
pub const TRADE_RETCODE_DONE: i64 = 10009;
pub const TRADE_RETCODE_DONE_PARTIAL: i64 = 10010;
/// Retcode the EA uses when it refuses a write (real account without `InpAllowReal`,
/// trading disabled in the terminal, etc.).
pub const RETCODE_WRITE_REFUSED: i64 = -3;

/// How a new order is placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecType {
    /// Market execution at the current price.
    Market,
    /// Pending limit order at `price`.
    Limit,
    /// Pending stop order at `price`.
    Stop,
}

/// `order_send` arguments. Volume must already be rounded to the symbol's
/// `volume_step` and prices to its `digits` (the EA re-validates and rejects otherwise).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderSendArgs {
    pub symbol: String,
    pub side: Side,
    pub volume: f64,
    #[serde(rename = "type")]
    pub exec: ExecType,
    /// Required for `limit`/`stop`, must be absent for `market`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sl: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tp: Option<f64>,
    /// Max slippage for market orders, in points. EA default 20.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deviation_points: Option<u32>,
    pub magic: i64,
    /// Max 31 chars (MT5 limit), e.g. `"jarvis:3f9a2c1d"`.
    pub comment: String,
}

/// `position_modify` arguments. A field that is absent keeps the current value;
/// `0` removes the SL/TP. At least one of `sl`/`tp` must be present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionModifyArgs {
    pub ticket: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sl: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tp: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionCloseArgs {
    pub ticket: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deviation_points: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionClosePartialArgs {
    pub ticket: u64,
    /// Volume to close; must be less than the position volume and a multiple of `volume_step`.
    pub volume: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deviation_points: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderCancelArgs {
    pub ticket: u64,
}

/// Reply data of every write command. The envelope `retcode` equals `retcode` here;
/// `ok` is true only for 10008/10009/10010.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradeResult {
    pub retcode: i64,
    /// `TradeResultRetcodeDescription()`-style text, e.g. "done".
    pub retcode_description: String,
    /// Order ticket created or affected (0 if none).
    pub order: u64,
    /// Deal ticket (0 if none, e.g. pending order placed).
    pub deal: u64,
    pub volume: f64,
    pub price: f64,
    pub comment: String,
}

/// `get_broker` reply data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrokerInfo {
    /// `ACCOUNT_COMPANY`, e.g. "CFI Financial Markets L.L.C". The key of a symbol map.
    pub company: String,
    /// `ACCOUNT_SERVER`, e.g. "CFI3-Demo".
    pub server: String,
    pub trade_mode: TradeMode,
}

/// Maximum page size of `list_symbols`.
pub const LIST_SYMBOLS_LIMIT_MAX: u32 = 1000;

/// `list_symbols` arguments: `offset` 0-based into the name-ordered catalog, `limit`
/// 1..=1000 (default 500).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListSymbolsQuery {
    #[serde(default)]
    pub offset: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Bar period of `get_rates`, serialised as its MT5 name (`"M15"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Timeframe {
    M1,
    M5,
    M15,
    M30,
    H1,
    H4,
    D1,
}

impl Timeframe {
    /// Every timeframe, shortest first.
    pub const ALL: [Timeframe; 7] = [
        Timeframe::M1,
        Timeframe::M5,
        Timeframe::M15,
        Timeframe::M30,
        Timeframe::H1,
        Timeframe::H4,
        Timeframe::D1,
    ];

    /// The wire name, e.g. `"M15"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Timeframe::M1 => "M1",
            Timeframe::M5 => "M5",
            Timeframe::M15 => "M15",
            Timeframe::M30 => "M30",
            Timeframe::H1 => "H1",
            Timeframe::H4 => "H4",
            Timeframe::D1 => "D1",
        }
    }

    /// Length of one bar in seconds.
    pub fn seconds(self) -> i64 {
        match self {
            Timeframe::M1 => 60,
            Timeframe::M5 => 300,
            Timeframe::M15 => 900,
            Timeframe::M30 => 1800,
            Timeframe::H1 => 3600,
            Timeframe::H4 => 14_400,
            Timeframe::D1 => 86_400,
        }
    }
}

impl std::fmt::Display for Timeframe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Timeframe {
    type Err = String;

    /// Case-insensitive: `"m15"` and `"M15"` both parse.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Timeframe::ALL
            .into_iter()
            .find(|t| t.as_str().eq_ignore_ascii_case(s))
            .ok_or_else(|| format!("unknown timeframe {s:?} (use M1 M5 M15 M30 H1 H4 or D1)"))
    }
}

/// `get_rates` arguments. `count` is 1..=[`RATES_COUNT_MAX`], default [`RATES_COUNT_DEFAULT`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RatesQuery {
    pub symbol: String,
    pub timeframe: Timeframe,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

impl RatesQuery {
    /// The checks the EA makes, so a bad query fails without a round trip.
    pub fn validate(&self) -> Result<(), String> {
        if self.symbol.trim().is_empty() {
            return Err("get_rates: 'symbol' must not be empty".to_owned());
        }
        if let Some(count) = self.count
            && !(1..=RATES_COUNT_MAX).contains(&count)
        {
            return Err(format!(
                "get_rates: 'count' must be between 1 and {RATES_COUNT_MAX}, got {count}"
            ));
        }
        Ok(())
    }
}

/// One price bar. `time` is the bar open time in server time (Unix seconds).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    pub time: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub tick_volume: u64,
    /// Spread of the bar in points.
    pub spread: i64,
}

/// `get_rates` reply data: bars oldest first; the last one is still forming.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rates {
    pub symbol: String,
    pub timeframe: Timeframe,
    pub digits: u32,
    pub bars: Vec<Bar>,
}

/// Importance of a calendar event. Ordered, so `Importance::High >= Importance::Moderate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Importance {
    Low,
    Moderate,
    High,
}

impl Importance {
    /// The wire name, e.g. `"moderate"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Importance::Low => "low",
            Importance::Moderate => "moderate",
            Importance::High => "high",
        }
    }
}

impl std::fmt::Display for Importance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Importance {
    type Err = String;

    /// Case-insensitive.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        [Importance::Low, Importance::Moderate, Importance::High]
            .into_iter()
            .find(|i| i.as_str().eq_ignore_ascii_case(s))
            .ok_or_else(|| format!("unknown importance {s:?} (use low, moderate or high)"))
    }
}

/// `get_calendar` arguments. `from`/`to` are server-time Unix seconds. An empty `currencies`
/// means all currencies; no `min_importance` means `moderate` (applied by the EA). Currency
/// codes are case-insensitive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarQuery {
    pub from: i64,
    pub to: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub currencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_importance: Option<Importance>,
}

impl CalendarQuery {
    /// The checks the EA makes, so a bad query fails without a round trip.
    pub fn validate(&self) -> Result<(), String> {
        if self.to <= self.from {
            return Err(format!(
                "get_calendar: 'to' ({}) must be after 'from' ({})",
                self.to, self.from
            ));
        }
        if self.to - self.from > CALENDAR_RANGE_MAX_SECS {
            return Err(format!(
                "get_calendar: the range must be at most {} days",
                CALENDAR_RANGE_MAX_SECS / 86_400
            ));
        }
        for c in &self.currencies {
            if c.len() != 3 || !c.bytes().all(|b| b.is_ascii_alphabetic()) {
                return Err(format!(
                    "get_calendar: currency {c:?} must be a three-letter code such as USD"
                ));
            }
        }
        Ok(())
    }
}

/// One economic calendar event. `actual`, `forecast` and `previous` are `null` on the wire
/// when MT5 has no value (an event that has not happened yet has no `actual`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarEvent {
    /// Release time, server time (Unix seconds).
    pub time: i64,
    /// Currency of the country the event belongs to, e.g. `"USD"`.
    pub currency: String,
    /// ISO 3166 alpha-2 country code, e.g. `"US"` (`"EU"` for the euro area).
    pub country: String,
    /// Event name in the terminal's language.
    pub event: String,
    pub importance: Importance,
    pub actual: Option<f64>,
    pub forecast: Option<f64>,
    pub previous: Option<f64>,
    /// Multiplier and unit of the values, e.g. `"%"`, `"K"`, `"M"`; empty when there is none.
    pub unit: String,
}

/// `get_calendar` reply data: events sorted by time, at most [`CALENDAR_EVENTS_MAX`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Calendar {
    pub events: Vec<CalendarEvent>,
}

/// MT5 errors that mean "the history is not (fully) loaded yet", not "the request is wrong":
/// `ERR_HISTORY_NOT_FOUND` 4401, `ERR_HISTORY_TIMEOUT` 4403, `ERR_HISTORY_LOAD_ERRORS` 4405 and
/// `ERR_INDICATOR_DATA_NOT_FOUND` 4806 (what a series read reports while the data is being
/// built). The client retries `get_rates` once when the EA answers with one of them.
pub const HISTORY_NOT_READY_RETCODES: [i64; 4] = [4401, 4403, 4405, 4806];

/// How a symbol may be traded (`SYMBOL_TRADE_MODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolTradeMode {
    Full,
    LongOnly,
    ShortOnly,
    CloseOnly,
    Disabled,
}

/// One entry of the broker's symbol catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogSymbol {
    pub name: String,
    pub description: String,
    /// `SYMBOL_PATH`, e.g. "FX Gold Elite\\XAUUSD_".
    pub path: String,
    pub trade_mode: SymbolTradeMode,
    /// Shown in Market Watch (`SYMBOL_VISIBLE`/selected): a strong hint of what is traded.
    pub visible: bool,
    pub digits: u32,
    pub currency_base: String,
    pub currency_profit: String,
}

/// `list_symbols` reply data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolCatalogPage {
    pub symbols: Vec<CatalogSymbol>,
    /// Total symbols in the catalog (all pages).
    pub total: u32,
    pub offset: u32,
}

/// Seconds in a day: the end of a session that runs to 24:00.
pub const SECONDS_PER_DAY: i64 = 86_400;

/// Today's quote and trade sessions of a symbol, for the current server weekday.
///
/// Each entry is `[from, to]` in seconds since 00:00 server time. `from` is inclusive and
/// `to` exclusive; a session that ends at 24:00 has `to` = [`SECONDS_PER_DAY`]. An empty list
/// means there is no such session today (a weekend, or a market that is closed all day).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sessions {
    /// Quote sessions (`SymbolInfoSessionQuote`): when prices are streamed.
    pub quote: Vec<[i64; 2]>,
    /// Trade sessions (`SymbolInfoSessionTrade`): when orders are accepted.
    pub trade: Vec<[i64; 2]>,
}

impl Sessions {
    /// True when `second_of_day` (0..86400, server time) falls inside a quote session.
    #[must_use]
    pub fn quote_open_at(&self, second_of_day: i64) -> bool {
        contains(&self.quote, second_of_day)
    }

    /// True when `second_of_day` (0..86400, server time) falls inside a trade session.
    #[must_use]
    pub fn trade_open_at(&self, second_of_day: i64) -> bool {
        contains(&self.trade, second_of_day)
    }
}

fn contains(sessions: &[[i64; 2]], second_of_day: i64) -> bool {
    sessions
        .iter()
        .any(|[from, to]| *from <= second_of_day && second_of_day < *to)
}

/// `get_symbol` reply data.
///
/// The fields from `spread` on arrived with EA 0.6.0. They are optional so that the reply of an
/// older EA still parses; a newer EA always sends all of them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub symbol: String,
    pub description: String,
    pub digits: u32,
    pub point: f64,
    pub volume_min: f64,
    pub volume_max: f64,
    pub volume_step: f64,
    pub contract_size: f64,
    /// Account-currency value of one `tick_size` move for 1.0 lot.
    pub tick_value: f64,
    pub tick_size: f64,
    /// Minimum SL/TP distance from price, in points.
    pub stops_level: i64,
    /// False when the symbol is close-only or disabled for trading.
    pub trade_allowed: bool,
    pub currency_profit: String,
    /// Current spread in points (`SYMBOL_SPREAD`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spread: Option<i64>,
    /// Swap for a long position, in the unit named by `swap_mode` (`SYMBOL_SWAP_LONG`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_long: Option<f64>,
    /// Swap for a short position, in the unit named by `swap_mode` (`SYMBOL_SWAP_SHORT`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_short: Option<f64>,
    /// How the swaps are calculated (`SYMBOL_SWAP_MODE`), the `SYMBOL_SWAP_MODE_*` name in
    /// lower snake case: `disabled`, `points`, `currency_symbol`, `currency_margin`,
    /// `currency_deposit`, `interest_current`, `interest_open`, `reopen_current`,
    /// `reopen_bid`, or `unknown` for a mode this EA does not know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_mode: Option<String>,
    /// `SYMBOL_TRADE_MODE`; `trade_allowed` is the coarse version of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trade_mode: Option<SymbolTradeMode>,
    /// Today's sessions, by the server's current weekday.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions: Option<Sessions>,
    /// The server's time of day is inside a trade session and the trade mode is not
    /// `disabled`. A symbol with no trade session today is closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trade_open_now: Option<bool>,
}

/// Arguments of `get_history`. Times are Unix seconds (server time as MT5 reports it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryQuery {
    pub from: i64,
    pub to: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// 1..=500, default 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// A request envelope: `{"id", "ts", "cmd", "args"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    /// Sender's clock, Unix milliseconds. The client always sets it; the EA refuses a request
    /// without one. Optional here only so that older golden files still parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<i64>,
    #[serde(flatten)]
    pub command: Command,
}

/// A reply envelope as it arrives on the wire. `data` is decoded later into
/// the type the caller expects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub retcode: i64,
    /// EA clock, Unix milliseconds (informational; the MAC and the `id` bind a reply to its
    /// request).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<i64>,
}

impl Reply {
    /// Decode `data` into `T`. Fails if `ok` is false or `data` does not match `T`.
    pub fn decode<T: DeserializeOwned>(self) -> Result<T, ReplyError> {
        if !self.ok {
            return Err(ReplyError::Remote {
                message: self.error.unwrap_or_else(|| "unknown error".to_owned()),
                retcode: self.retcode,
            });
        }
        let data = self.data.unwrap_or(Value::Null);
        serde_json::from_value(data).map_err(|e| ReplyError::Decode(e.to_string()))
    }
}

/// Why a reply could not be turned into a value.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ReplyError {
    #[error("bridge error (retcode {retcode}): {message}")]
    Remote { message: String, retcode: i64 },
    #[error("could not decode reply data: {0}")]
    Decode(String),
}

/// Account trade mode as reported by MT5 (`ACCOUNT_TRADE_MODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TradeMode {
    Demo,
    Contest,
    Real,
}

/// `ping` reply data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PingInfo {
    pub protocol_version: u32,
    pub ea_version: String,
    pub trade_mode: TradeMode,
    /// `TERMINAL_CONNECTED`: terminal is connected to the trade server.
    pub terminal_connected: bool,
    /// `TERMINAL_TRADE_ALLOWED` && `MQL_TRADE_ALLOWED`.
    pub trade_allowed: bool,
    /// `TimeTradeServer()`, Unix seconds.
    pub server_time: i64,
}

/// `get_price` reply data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub symbol: String,
    pub bid: f64,
    pub ask: f64,
    pub last: f64,
    pub spread_points: i64,
    pub digits: u32,
    /// Last tick time, Unix seconds.
    pub time: i64,
}

/// `get_account` reply data. The EA never sends login, name or server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub currency: String,
    pub balance: f64,
    pub equity: f64,
    pub profit: f64,
    pub margin: f64,
    pub free_margin: f64,
    /// Percent. 0 when there is no used margin.
    pub margin_level: f64,
    pub leverage: i64,
    pub trade_mode: TradeMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Buy,
    Sell,
}

/// One open position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub ticket: u64,
    pub symbol: String,
    pub side: Side,
    pub volume: f64,
    pub price_open: f64,
    pub price_current: f64,
    /// `null` on the wire when not set (MT5 reports 0).
    pub sl: Option<f64>,
    pub tp: Option<f64>,
    pub profit: f64,
    pub swap: f64,
    pub magic: i64,
    pub comment: String,
    /// Open time, Unix seconds.
    pub time: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Positions {
    pub positions: Vec<Position>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderType {
    BuyLimit,
    SellLimit,
    BuyStop,
    SellStop,
    BuyStopLimit,
    SellStopLimit,
}

/// One pending order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub ticket: u64,
    pub symbol: String,
    #[serde(rename = "type")]
    pub order_type: OrderType,
    pub volume_initial: f64,
    pub volume_current: f64,
    pub price_open: f64,
    pub sl: Option<f64>,
    pub tp: Option<f64>,
    /// Setup time, Unix seconds.
    pub time_setup: i64,
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Orders {
    pub orders: Vec<Order>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DealType {
    Buy,
    Sell,
    Balance,
    Credit,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DealEntry {
    In,
    Out,
    Inout,
    OutBy,
}

/// One deal from account history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deal {
    pub ticket: u64,
    pub order: u64,
    pub position_id: u64,
    /// Empty for balance/credit deals.
    pub symbol: String,
    #[serde(rename = "type")]
    pub deal_type: DealType,
    pub entry: DealEntry,
    pub volume: f64,
    pub price: f64,
    pub profit: f64,
    pub commission: f64,
    pub swap: f64,
    /// Unix seconds.
    pub time: i64,
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct History {
    pub deals: Vec<Deal>,
    /// True when more deals matched than `limit` allowed.
    pub truncated: bool,
}
