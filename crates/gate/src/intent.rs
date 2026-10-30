//! Deterministic intent parser (English + Hinglish). It does not replace the LLM; it
//! (1) recognises "confirm"/"cancel" in speech or text so the UI can call
//! `/api/confirm/{token}` or `/api/cancel/{token}`, and (2) extracts the trading facts
//! the user actually said so [`cross_check`] can reject an LLM proposal that differs
//! (wrong side, symbol, volume) — SPEC rule 9, ADR-0006.

use std::collections::HashMap;

use jarvis_bridge_client::protocol::Side;
use serde::{Deserialize, Serialize};

use crate::ProposedAction;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Intent {
    /// "confirm", "yes do it", "haan kar do", "go ahead".
    Confirm,
    /// "cancel", "no", "stop", "nahi", "ruk jao", "mat karo".
    Cancel,
    /// Open a new trade. Every field is what the user said, `None` if not said.
    Order(OrderFacts),
    /// Close positions: "close all", "sab band karo", "close my gold longs".
    Close(CloseFacts),
    /// Change SL/TP: "move my stop to 3990", "gold ka tp 4050 karo".
    Modify(ModifyFacts),
    /// Cancel pending orders: "cancel my pending orders".
    CancelOrders(CloseFacts),
    /// Anything else (questions, chit-chat): leave to the LLM.
    Other,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OrderFacts {
    pub side: Option<Side>,
    /// Broker symbol after alias resolution.
    pub symbol: Option<String>,
    pub volume: Option<f64>,
    /// "limit"/"stop" said explicitly; `None` = market or unspecified.
    pub pending: Option<jarvis_bridge_client::protocol::ExecType>,
    pub price: Option<f64>,
    pub sl: Option<f64>,
    pub tp: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CloseFacts {
    pub all: bool,
    pub symbol: Option<String>,
    pub side: Option<Side>,
    pub tickets: Vec<u64>,
    /// Partial: "close half", "close 0.05".
    pub volume: Option<f64>,
    pub half: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModifyFacts {
    pub symbol: Option<String>,
    pub ticket: Option<u64>,
    pub sl: Option<f64>,
    pub tp: Option<f64>,
}

/// Parse one utterance. `aliases` maps lowercase spoken names to broker symbols
/// (same table as `[symbols.aliases]`); bare broker symbols (e.g. "xauusd") are also
/// recognised. Numbers may be digits ("0.1"), words ("point one", "zero point five",
/// "half a lot", "ek lot", "aadha lot") and Hindi number words.
pub fn parse(text: &str, aliases: &HashMap<String, String>) -> Intent {
    let _ = (text, aliases);
    todo!("Sprint 2 gate task")
}

/// A disagreement between what the user said and what the LLM proposed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{field}: you said {said}, but the proposal has {proposed}")]
pub struct Mismatch {
    pub field: &'static str,
    pub said: String,
    pub proposed: String,
}

/// Reject a proposal that contradicts facts the user stated in `utterance_intent`.
/// Facts the user did not state are not checked (the LLM may fill them from context,
/// e.g. "close it" after discussing one position). `Other`/`Confirm`/`Cancel` intents
/// never block.
pub fn cross_check(utterance_intent: &Intent, proposal: &ProposedAction) -> Result<(), Mismatch> {
    let _ = (utterance_intent, proposal);
    todo!("Sprint 2 gate task")
}
