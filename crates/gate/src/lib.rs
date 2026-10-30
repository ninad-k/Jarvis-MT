//! The action gate: the only path from "the user wants a trade" to a bridge write.
//!
//! - [`evaluate`] turns a [`ProposedAction`] plus a [`MarketSnapshot`] into an
//!   [`ActionPreview`] after checking every [`Limits`] rule (SPEC rule 5). Pure and
//!   deterministic: no I/O, no clock reads (time comes from the snapshot).
//! - [`TokenStore`] issues one-time confirmation tokens (rule 2), handles the batch
//!   double confirmation (rule 4) and the voice read-back timing (ADR-0007).
//! - [`KillSwitch`] blocks every write instantly (rule 6).
//! - [`intent`] is a deterministic parser for confirm/cancel and trading phrases
//!   (English + Hinglish) used to cross-check what the LLM proposed (rule 9).
//!
//! CONTRACT STUB: public types and signatures are fixed; bodies are implemented in
//! the Sprint 2 gate task.

pub mod intent;
mod killswitch;
mod limits;
mod preview;
mod tokens;

pub use killswitch::{KillSwitch, KillSwitchState};
pub use limits::{Limits, TradingHours, Weekday};
pub use preview::{
    ActionPreview, CancelSelector, CloseSelector, EstPnl, MarketSnapshot, ModifyIntent,
    OrderIntent, ProposedAction, evaluate,
};
pub use tokens::{ConfirmOutcome, IssuedToken, TokenError, TokenStage, TokenStore, TokenTiming};

/// Why the gate refused to build a preview. Every variant has a message that can be
/// shown to the user and given to the LLM.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GateError {
    /// A limit from config was hit. `rule` is one of `max_lot`, `max_positions`,
    /// `max_daily_loss`, `allowed_symbols`, `trading_hours`.
    #[error("limit {rule} blocks this: {detail}")]
    LimitExceeded { rule: &'static str, detail: String },
    #[error("the kill switch is engaged; trading is disabled")]
    KillSwitchEngaged,
    /// The request cannot be resolved without asking the user (rule 9).
    #[error("{question}")]
    Ambiguous { question: String },
    /// Structurally invalid (e.g. limit order without price, SL on wrong side).
    #[error("invalid request: {0}")]
    Invalid(String),
    /// Selector matched nothing (e.g. "close my gold trades" with no gold positions).
    #[error("nothing to do: {0}")]
    NothingToDo(String),
}
