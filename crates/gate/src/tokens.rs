//! One-time confirmation tokens (SPEC rules 2–4, ADR-0007/0008).
//!
//! Time is passed in explicitly as Unix milliseconds so tests are deterministic.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use jarvis_bridge_client::protocol::Command;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    /// 64 lowercase hex characters from two v4 UUIDs (CSPRNG-backed; 244 random bits).
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
    NeedsSecond {
        token: IssuedToken,
        count: usize,
        message: String,
    },
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

/// How long used/cancelled/expired tokens are remembered after their expiry, so a
/// replay reports `AlreadyUsed`/`Expired` instead of `Unknown`.
const TOMBSTONE_MS: i64 = 5 * 60 * 1000;

/// In-memory store. Tokens are single-use, expire, and are invalidated when the kill
/// switch is engaged. Used/expired tokens are remembered for a while so a replay
/// reports `AlreadyUsed`/`Expired` rather than `Unknown`.
pub struct TokenStore {
    ttl_ms: i64,
    cap_ms: i64,
    kill_switch: KillSwitch,
    inner: Mutex<Inner>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Live,
    Used,
    Cancelled,
    Expired,
}

struct Entry {
    preview: ActionPreview,
    stage: TokenStage,
    timing: TokenTiming,
    issued_ms: i64,
    expires_ms: i64,
    readback_done: bool,
    state: State,
    /// The token created from (or that created) this one in a batch flow.
    sibling: Option<String>,
}

impl Entry {
    fn issued(&self, token: &str) -> IssuedToken {
        IssuedToken {
            token: token.to_owned(),
            preview_id: self.preview.id,
            stage: self.stage,
            expires_at_ms: self.expires_ms,
        }
    }

    /// Flip a live entry to `Expired` once `now_ms` reaches its expiry (the expiry
    /// instant itself is already expired) and report its resulting state.
    fn refresh(&mut self, now_ms: i64) -> State {
        if self.state == State::Live && now_ms >= self.expires_ms {
            self.state = State::Expired;
        }
        self.state
    }
}

#[derive(Default)]
struct Inner {
    tokens: HashMap<String, Entry>,
}

impl Inner {
    fn fresh_token(&self) -> String {
        loop {
            let token = format!(
                "{:032x}{:032x}",
                Uuid::new_v4().as_u128(),
                Uuid::new_v4().as_u128()
            );
            if !self.tokens.contains_key(&token) {
                return token;
            }
        }
    }

    fn revoke_all(&mut self) {
        for e in self.tokens.values_mut() {
            if e.state == State::Live {
                e.state = State::Cancelled;
            }
        }
    }
}

/// Map a non-live state to the error a caller should see.
fn dead_state_error(state: State) -> TokenError {
    match state {
        State::Expired => TokenError::Expired,
        State::Used | State::Cancelled | State::Live => TokenError::AlreadyUsed,
    }
}

/// "close 3 positions", "cancel 2 pending orders", from the commands the preview runs.
fn batch_phrase(preview: &ActionPreview, count: usize) -> String {
    let plural = |one: &str, many: &str| {
        if count == 1 {
            one.to_owned()
        } else {
            many.to_owned()
        }
    };
    match preview.commands.first() {
        Some(Command::PositionClose(_) | Command::PositionClosePartial(_)) => {
            format!("close {count} {}", plural("position", "positions"))
        }
        Some(Command::OrderCancel(_)) => {
            format!(
                "cancel {count} {}",
                plural("pending order", "pending orders")
            )
        }
        _ => format!("execute {count} {}", plural("action", "actions")),
    }
}

impl TokenStore {
    /// `ttl_ms` default 15_000, `cap_ms` default 30_000.
    pub fn new(ttl_ms: i64, cap_ms: i64, kill_switch: KillSwitch) -> Self {
        Self {
            ttl_ms,
            cap_ms,
            kill_switch,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Poisoning cannot leave the map in a state that matters (every critical section
    /// is a handful of field writes), so recover instead of panicking.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Issue the first token for a preview.
    pub fn issue(&self, preview: ActionPreview, timing: TokenTiming, now_ms: i64) -> IssuedToken {
        let expires_ms = match timing {
            TokenTiming::Immediate => now_ms.saturating_add(self.ttl_ms),
            // Until read-back finishes the window is the cap; `readback_finished` shortens it.
            TokenTiming::AfterReadback => now_ms.saturating_add(self.cap_ms),
        };
        let mut inner = self.lock();
        let token = inner.fresh_token();
        let entry = Entry {
            preview,
            stage: TokenStage::First,
            timing,
            issued_ms: now_ms,
            expires_ms,
            readback_done: false,
            state: State::Live,
            sibling: None,
        };
        let issued = entry.issued(&token);
        inner.tokens.insert(token, entry);
        issued
    }

    /// Voice flow: TTS finished reading the preview; start the `ttl_ms` window.
    /// Returns the updated token, or an error if it is unknown/used/expired.
    ///
    /// Only the first call counts: later calls return the token unchanged so a repeated
    /// callback can never extend the window. Tokens that are not `AfterReadback` (and
    /// second-stage tokens) are returned unchanged.
    pub fn readback_finished(&self, token: &str, now_ms: i64) -> Result<IssuedToken, TokenError> {
        let mut inner = self.lock();
        let entry = inner.tokens.get_mut(token).ok_or(TokenError::Unknown)?;
        match entry.refresh(now_ms) {
            State::Live => {}
            other => return Err(dead_state_error(other)),
        }
        if entry.timing == TokenTiming::AfterReadback
            && entry.stage == TokenStage::First
            && !entry.readback_done
        {
            let cap_expiry = entry.issued_ms.saturating_add(self.cap_ms);
            entry.expires_ms = now_ms.saturating_add(self.ttl_ms).min(cap_expiry);
            entry.readback_done = true;
        }
        Ok(entry.issued(token))
    }

    /// Consume a token. Checks the kill switch first.
    pub fn confirm(&self, token: &str, now_ms: i64) -> Result<ConfirmOutcome, TokenError> {
        let mut inner = self.lock();
        if self.kill_switch.is_engaged() {
            inner.revoke_all();
            return Err(TokenError::KillSwitchEngaged);
        }
        let entry = inner.tokens.get_mut(token).ok_or(TokenError::Unknown)?;
        match entry.refresh(now_ms) {
            State::Live => {}
            other => return Err(dead_state_error(other)),
        }
        entry.state = State::Used;

        let first_of_batch = entry.preview.is_batch && entry.stage == TokenStage::First;
        if !first_of_batch {
            return Ok(ConfirmOutcome::Execute(entry.preview.clone()));
        }

        // Batch, first stage: nothing executes yet. Issue the second-stage token.
        let preview = entry.preview.clone();
        let count = preview.count;
        let message = format!("Confirm again to {}", batch_phrase(&preview, count));
        let second_token = inner.fresh_token();
        let second = Entry {
            preview,
            stage: TokenStage::Second,
            timing: TokenTiming::Immediate,
            issued_ms: now_ms,
            expires_ms: now_ms.saturating_add(self.ttl_ms),
            readback_done: false,
            state: State::Live,
            sibling: Some(token.to_owned()),
        };
        let issued = second.issued(&second_token);
        if let Some(first) = inner.tokens.get_mut(token) {
            first.sibling = Some(second_token.clone());
        }
        inner.tokens.insert(second_token, second);
        Ok(ConfirmOutcome::NeedsSecond {
            token: issued,
            count,
            message,
        })
    }

    /// User said/pressed cancel. Consumes the token (and its batch sibling).
    ///
    /// Cancelling the first-stage token of a batch after it was already confirmed cancels
    /// the live second-stage token and succeeds ("cancelling either token cancels
    /// both"). Cancelling stays allowed while the kill switch is engaged.
    pub fn cancel(&self, token: &str, now_ms: i64) -> Result<(), TokenError> {
        let mut inner = self.lock();
        let sibling = {
            let entry = inner.tokens.get_mut(token).ok_or(TokenError::Unknown)?;
            entry.refresh(now_ms);
            entry.sibling.clone()
        };
        let mut cancelled_any = false;
        for key in std::iter::once(token.to_owned()).chain(sibling) {
            if let Some(e) = inner.tokens.get_mut(&key)
                && e.refresh(now_ms) == State::Live
            {
                e.state = State::Cancelled;
                cancelled_any = true;
            }
        }
        if cancelled_any {
            Ok(())
        } else {
            let state = inner.tokens.get(token).map_or(State::Used, |e| e.state);
            Err(dead_state_error(state))
        }
    }

    /// Drop every live token (called when the kill switch is engaged).
    ///
    /// Revoked tokens are remembered like cancelled ones, so a replay reports
    /// `AlreadyUsed`.
    pub fn revoke_all(&self) {
        self.lock().revoke_all();
    }

    /// Remove entries older than a few minutes. Call periodically.
    ///
    /// An entry is dropped once `now_ms` is at least five minutes past its expiry,
    /// whatever its state; until then replays still report `AlreadyUsed`/`Expired`.
    pub fn purge(&self, now_ms: i64) {
        self.lock()
            .tokens
            .retain(|_, e| now_ms.saturating_sub(e.expires_ms) < TOMBSTONE_MS);
    }
}
