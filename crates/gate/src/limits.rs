use serde::{Deserialize, Serialize};

/// `[limits]` from `config/jarvis.toml`. Checked before any preview is issued.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// Max volume of one new order, and max total volume of one batch action.
    pub max_lot: f64,
    /// Max open positions + pending orders after a new order.
    pub max_positions: u32,
    /// Positive amount in account currency. New orders are blocked once today's
    /// realised P/L + current floating P/L is at or below `-max_daily_loss`.
    /// Closing/cancelling is still allowed (it reduces risk).
    pub max_daily_loss: f64,
    /// Broker symbols that may be traded. Empty means none (fail closed).
    pub allowed_symbols: Vec<String>,
    /// When new orders may be placed, in broker server time. `None` = always.
    /// Closing/modifying/cancelling is allowed outside these hours.
    #[serde(default)]
    pub trading_hours: Option<TradingHours>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

/// A daily window on the given days, server time, `"HH:MM"` inclusive start,
/// exclusive end. `start > end` means the window crosses midnight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradingHours {
    pub days: Vec<Weekday>,
    pub start: String,
    pub end: String,
}

impl Limits {
    /// Validate config values (positive numbers, parseable times). Called at startup.
    pub fn validate(&self) -> Result<(), String> {
        todo!("Sprint 2 gate task")
    }
}

impl TradingHours {
    /// Is `server_time` (Unix seconds in broker server time) inside the window?
    pub fn contains(&self, server_time: i64) -> bool {
        let _ = server_time;
        todo!("Sprint 2 gate task")
    }
}
