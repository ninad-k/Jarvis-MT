use std::collections::HashMap;

use jarvis_bridge_client::protocol::{
    Account, Command, ExecType, Order, Position, Price, Side, SymbolInfo,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{GateError, Limits};

/// What the LLM (via a `propose_*` tool) or a UI button asks for. Symbols are
/// already resolved to broker names by the caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ProposedAction {
    Order(OrderIntent),
    Modify(ModifyIntent),
    Close(CloseSelector),
    Cancel(CancelSelector),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderIntent {
    pub symbol: String,
    pub side: Side,
    pub volume: f64,
    pub exec: ExecType,
    /// Required for limit/stop.
    pub price: Option<f64>,
    pub sl: Option<f64>,
    pub tp: Option<f64>,
}

/// `None` keeps the current value, `Some(0.0)` removes it. At least one must be set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModifyIntent {
    pub ticket: u64,
    pub sl: Option<f64>,
    pub tp: Option<f64>,
}

/// Which positions to close. Exactly one targeting mode:
/// - `tickets` non-empty: those positions (with optional `volume` for a partial close
///   of a single ticket);
/// - otherwise filters `symbol` and/or `side` (both `None` + `all = true` = close all).
///
/// More than one position matched => batch (rule 4).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CloseSelector {
    #[serde(default)]
    pub tickets: Vec<u64>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub side: Option<Side>,
    /// Must be true to close everything when no other filter is given.
    #[serde(default)]
    pub all: bool,
    /// Partial close volume; only with exactly one ticket.
    #[serde(default)]
    pub volume: Option<f64>,
}

/// Which pending orders to cancel (same rules as [`CloseSelector`], no volume).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CancelSelector {
    #[serde(default)]
    pub tickets: Vec<u64>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub all: bool,
}

/// Everything [`evaluate`] needs, fetched by the caller from the bridge just before.
#[derive(Debug, Clone, PartialEq)]
pub struct MarketSnapshot {
    pub account: Account,
    pub positions: Vec<Position>,
    pub orders: Vec<Order>,
    /// Current prices of every symbol involved.
    pub prices: HashMap<String, Price>,
    /// Trading specs of every symbol involved.
    pub symbols: HashMap<String, SymbolInfo>,
    /// Broker server time, Unix seconds.
    pub server_time: i64,
    /// Realised P/L (profit + commission + swap) of deals since server midnight today.
    pub realized_pnl_today: f64,
}

/// Estimated P/L in account currency. For closes: `current` is the floating P/L
/// being realised. For new orders: P/L if SL / TP is hit (from tick value), `None`
/// when not set.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EstPnl {
    pub current: Option<f64>,
    pub at_sl: Option<f64>,
    pub at_tp: Option<f64>,
}

/// What the user sees and confirms (SPEC rule 2). `commands` are the exact bridge
/// writes executed on confirmation, in order; nothing else is ever sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionPreview {
    pub id: Uuid,
    /// One-line, voice-friendly, e.g. "Buy 0.10 XAUUSD at market, SL 3990.00, TP 4020.00".
    pub description: String,
    pub symbols: Vec<String>,
    pub tickets: Vec<u64>,
    pub total_volume: f64,
    pub est_pnl: EstPnl,
    /// True when more than one position/order is affected (needs second confirmation).
    pub is_batch: bool,
    /// Number of bridge writes (positions/orders affected).
    pub count: usize,
    /// Non-blocking notes, e.g. "spread is wide (45 points)".
    pub warnings: Vec<String>,
    pub commands: Vec<Command>,
}

/// Check `action` against `limits` and the snapshot and build the preview.
///
/// Volumes are rounded DOWN to `volume_step` (and must stay >= `volume_min`), prices
/// to `digits`; SL/TP must be on the correct side and at least `stops_level` points
/// away. New orders get `magic = JARVIS_MAGIC` and comment `"jarvis:<first 8 hex of id>"`.
/// `kill_switch_engaged` makes every action fail with `KillSwitchEngaged`.
pub fn evaluate(
    action: &ProposedAction,
    snapshot: &MarketSnapshot,
    limits: &Limits,
    kill_switch_engaged: bool,
) -> Result<ActionPreview, GateError> {
    let _ = (action, snapshot, limits, kill_switch_engaged);
    todo!("Sprint 2 gate task")
}
