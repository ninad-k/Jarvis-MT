//! Wire protocol between the Rust backend and the `JarvisBridge` MQL5 EA.
//!
//! This module is the source of truth for the JSON contract. `docs/protocol.md`
//! and the golden files in `tests/fixtures/` must match it.
//!
//! Request:  `{"id": "...", "cmd": "get_price", "args": {"symbol": "XAUUSD"}}`
//! Reply:    `{"id": "...", "ok": true,  "data": {...},          "retcode": 0}`
//!           `{"id": "...", "ok": false, "error": "message",     "retcode": 4301}`

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

/// Protocol version reported by the EA in `ping`. Bump on breaking changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// Retcode used by the EA for malformed requests or invalid arguments.
pub const RETCODE_INVALID_REQUEST: i64 = -1;
/// Retcode used by the EA for an unknown or not-yet-implemented command.
pub const RETCODE_UNKNOWN_COMMAND: i64 = -2;

/// Maximum number of deals `get_history` may return in one reply.
pub const HISTORY_LIMIT_MAX: u32 = 500;
/// Default `get_history` limit when none is given.
pub const HISTORY_LIMIT_DEFAULT: u32 = 100;

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
        }
    }
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

/// A request envelope: `{"id", "cmd", "args"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
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
