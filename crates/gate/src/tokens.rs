//! One-time confirmation tokens (SPEC rules 2–4, ADR-0007/0008).
//!
//! Time is passed in explicitly as Unix milliseconds so tests are deterministic.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::{ActionPreview, KillSwitch};

/// When the confirmation window starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenTiming {
    /// Text/button flow: valid for `ttl_ms` from issue.
    Immediate,
    /// Voice flow: valid until `readback_finished + ttl_ms`, but never later than
    /// `issued + cap_ms` (and if read-back never finishes, expires at the cap).
    AfterReadback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenStage {
    /// Single action, or the first of two confirmations for a batch.
    First,
    /// Second confirmation of a batch; its message repeats the count.
    Second,
}

/// A token handed to the UI (never to the LLM).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssuedToken {
    /// 256-bit random, hex (64 chars).
    pub token: String,
    pub preview_id: uuid::Uuid,
    pub stage: TokenStage,
    /// Current expiry, Unix ms (moves when read-back finishes).
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmOutcome {
    /// Execute these exact commands now. The token is consumed.
    Execute(ActionPreview),
    /// Batch: first confirmation accepted; ask again with this fresh token.
    /// `message` repeats the count, e.g. "Confirm again to close 5 positions".
    NeedsSecond { token: IssuedToken, count: usize, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("unknown confirmation token")]
    Unknown,
    #[error("confirmation token expired")]
    Expired,
    #[error("confirmation token already used")]
    AlreadyUsed,
    #[error("the kill switch is engaged; trading is disabled")]
    KillSwitchEngaged,
}

/// In-memory store. Tokens are single-use, expire, and are invalidated when the kill
/// switch is engaged. Used/expired tokens are remembered for a while so a replay
/// reports `AlreadyUsed`/`Expired` rather than `Unknown`.
pub struct TokenStore {
    ttl_ms: i64,
    cap_ms: i64,
    kill_switch: KillSwitch,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    _private: (),
}

impl TokenStore {
    /// `ttl_ms` default 15_000, `cap_ms` default 30_000.
    pub fn new(ttl_ms: i64, cap_ms: i64, kill_switch: KillSwitch) -> Self {
        Self { ttl_ms, cap_ms, kill_switch, inner: Mutex::new(Inner::default()) }
    }

    /// Issue the first token for a preview.
    pub fn issue(&self, preview: ActionPreview, timing: TokenTiming, now_ms: i64) -> IssuedToken {
        let _ = (preview, timing, now_ms, self.ttl_ms, self.cap_ms);
        todo!("Sprint 2 gate task")
    }

    /// Voice flow: TTS finished reading the preview; start the `ttl_ms` window.
    /// Returns the updated token, or an error if it is unknown/used/expired.
    pub fn readback_finished(&self, token: &str, now_ms: i64) -> Result<IssuedToken, TokenError> {
        let _ = (token, now_ms);
        todo!("Sprint 2 gate task")
    }

    /// Consume a token. Checks the kill switch first.
    pub fn confirm(&self, token: &str, now_ms: i64) -> Result<ConfirmOutcome, TokenError> {
        let _ = (token, now_ms, &self.kill_switch);
        todo!("Sprint 2 gate task")
    }

    /// User said/pressed cancel. Consumes the token (and its batch sibling).
    pub fn cancel(&self, token: &str, now_ms: i64) -> Result<(), TokenError> {
        let _ = (token, now_ms);
        todo!("Sprint 2 gate task")
    }

    /// Drop every live token (called when the kill switch is engaged).
    pub fn revoke_all(&self) {
        todo!("Sprint 2 gate task")
    }

    /// Remove entries older than a few minutes. Call periodically.
    pub fn purge(&self, now_ms: i64) {
        let _ = now_ms;
        todo!("Sprint 2 gate task")
    }
}
