//! Trading: the one place where a proposal becomes a preview and a confirmed preview
//! becomes bridge writes.
//!
//! ```text
//! propose_* tool / UI close button
//!        -> Trading::propose   (snapshot -> gate::evaluate -> TokenStore::issue)
//!        -> PreviewDto         (token goes to the browser, never to the LLM)
//! POST /api/confirm/{token}
//!        -> Trading::confirm   (TokenStore::confirm -> write lock -> re-check -> bridge writes)
//! ```
//!
//! SPEC rule 3: [`Trading::confirm`] is the only code that sends a write command, and it
//! only sends the commands stored in a confirmed [`ActionPreview`].
//!
//! # Between preview and write
//!
//! A preview is valid for 15 s (30 s by voice), and other things happen in that time: another
//! session's preview is confirmed, the day's loss grows, the market moves. So the confirmed
//! action is checked again right before it is sent (TRD-2):
//!
//! - a **new order** goes through [`evaluate`] a second time with a fresh [`MarketSnapshot`]; if
//!   it now fails, or would now produce a different order than the one the user saw, nothing is
//!   sent (422) and the token stays consumed;
//! - a **close, cancel or modify** only needs its tickets to still exist (and the kill switch
//!   off): they reduce or keep risk, so no limit applies to them.
//!
//! Always the commands the user confirmed are sent, never the re-evaluated ones.
//!
//! Confirmed actions run one at a time (a write lock, TRD-1). The kill switch is checked after
//! the lock is taken and again immediately before each bridge call. The bridge client serves
//! its write lane before its read lane, so a write waits for at most the one exchange already
//! in flight (a read takes at most `bridge.timeout_ms`); that window cannot be closed because a
//! request that was sent cannot be taken back.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use jarvis_bridge_client::protocol::{
    Command, DealType, HISTORY_LIMIT_MAX, HistoryQuery, Position, TRADE_RETCODE_DONE,
    TRADE_RETCODE_DONE_PARTIAL, TRADE_RETCODE_PLACED, TradeResult,
};
use jarvis_bridge_client::{BridgeClient, BridgeError};
use jarvis_gate::intent::{self, Intent, Mismatch};
use jarvis_gate::{
    ActionPreview, ConfirmOutcome, EstPnl, GateError, IssuedToken, KillSwitch, KillSwitchState,
    Limits, MarketSnapshot, ProposedAction, TokenError, TokenStage, TokenStore, TokenTiming,
    evaluate,
};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::audit::{Audit, AuditKind};
use crate::clock::{ServerClock, TimeSource};
use crate::config::TradingConfig;
use crate::events::{Event, EventBus};
use crate::ratelimit::RateLimiter;
use crate::state::ModeMonitor;

/// `/api/confirm` allowance: this many requests per minute per client key.
pub const CONFIRM_RATE_LIMIT: usize = 10;
const CONFIRM_RATE_WINDOW_MS: i64 = 60_000;
/// Token bookkeeping older than this is dropped (the gate keeps its own history).
const META_RETENTION_MS: i64 = 10 * 60_000;
/// Pages of `get_history` read to total today's realised P/L (500 deals each).
const HISTORY_PAGES: usize = 5;
const DAY_SECS: i64 = 86_400;
/// The `reason` of the `gate_rejected` audit row written when the checks run again at confirm.
pub const RECHECK_REASON: &str = "re-check at confirm";

// ---------------------------------------------------------------------------
// Types shared with the HTTP layer
// ---------------------------------------------------------------------------

/// What the browser sees of a pending action (`PreviewDto` in docs/api.md).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreviewDto {
    pub id: Uuid,
    pub description: String,
    pub symbols: Vec<String>,
    pub tickets: Vec<u64>,
    pub total_volume: f64,
    pub est_pnl: EstPnl,
    pub is_batch: bool,
    pub count: usize,
    pub warnings: Vec<String>,
    pub token: String,
    pub stage: TokenStage,
    pub expires_at_ms: i64,
    pub timing: TokenTiming,
}

impl PreviewDto {
    fn new(preview: &ActionPreview, issued: &IssuedToken, timing: TokenTiming) -> Self {
        Self {
            id: preview.id,
            description: preview.description.clone(),
            symbols: preview.symbols.clone(),
            tickets: preview.tickets.clone(),
            total_volume: preview.total_volume,
            est_pnl: preview.est_pnl.clone(),
            is_batch: preview.is_batch,
            count: preview.count,
            warnings: preview.warnings.clone(),
            token: issued.token.clone(),
            stage: issued.stage,
            expires_at_ms: issued.expires_at_ms,
            timing,
        }
    }
}

/// Outcome of one bridge write, as reported to the UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TradeResultDto {
    /// The position or order the write was about. For a new order: the order ticket MT5
    /// assigned (null when the order was not accepted).
    pub ticket: Option<u64>,
    pub ok: bool,
    pub retcode: Option<i64>,
    pub retcode_description: Option<String>,
    pub order: Option<u64>,
    pub deal: Option<u64>,
    pub price: Option<f64>,
    pub volume: Option<f64>,
    pub error: Option<String>,
}

/// Body of `200` from `POST /api/confirm/{token}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ConfirmResponse {
    Executed {
        preview_id: Uuid,
        results: Vec<TradeResultDto>,
        /// Set when a batch stopped early: `"kill_switch"`. Unsent writes are listed in
        /// `results` as `ok: false`.
        #[serde(skip_serializing_if = "Option::is_none")]
        aborted: Option<String>,
    },
    NeedsSecondConfirmation {
        preview_id: Uuid,
        token: String,
        stage: TokenStage,
        expires_at_ms: i64,
        count: usize,
        message: String,
    },
}

/// Why a trading operation failed. The HTTP layer maps these to the documented codes.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TradingError {
    #[error("{0}")]
    Gate(GateError),
    #[error("{0}")]
    IntentMismatch(Mismatch),
    #[error("{0}")]
    Token(TokenError),
    /// Config says DEMO but the account is real (SPEC rule 7).
    #[error("configured for a demo account but MetaTrader 5 reports a real account")]
    ModeMismatch,
    #[error("the kill switch is engaged; trading is disabled")]
    KillSwitch,
    /// A bridge read (or the reachability check before writes) failed. Nothing was sent.
    #[error("{0}")]
    Bridge(BridgeError),
    /// Market data needed for the gate could not be assembled.
    #[error("{0}")]
    Snapshot(String),
    /// The user's latest message confirms or cancels the pending preview; the app handles
    /// that, so a new proposal would be wrong.
    #[error("the user is answering the pending preview")]
    UserIsAnswering,
    /// The checks made again at confirmation (TRD-2) refused the action. Nothing was sent and
    /// the token is spent.
    #[error("{0}")]
    RecheckRejected(String),
    #[error("too many confirmations; retry in {retry_after_secs} s")]
    RateLimited { retry_after_secs: u64 },
    /// MT5 rejected the (single) write; message includes the retcode.
    #[error("{0}")]
    TradeFailed(String),
    /// A write was sent but no reply arrived (ADR-0013). Never retried.
    #[error("{0}")]
    OutcomeUnknown(String),
    #[error("{0}")]
    Internal(String),
}

impl From<BridgeError> for TradingError {
    fn from(e: BridgeError) -> Self {
        TradingError::Bridge(e)
    }
}

impl From<GateError> for TradingError {
    fn from(e: GateError) -> Self {
        match e {
            GateError::KillSwitchEngaged => TradingError::KillSwitch,
            other => TradingError::Gate(other),
        }
    }
}

impl From<TokenError> for TradingError {
    fn from(e: TokenError) -> Self {
        match e {
            TokenError::KillSwitchEngaged => TradingError::KillSwitch,
            other => TradingError::Token(other),
        }
    }
}

/// Who asked for the action.
#[derive(Debug, Clone)]
pub enum ProposeSource {
    /// A `propose_*` tool call; `intent` is what the deterministic parser understood from
    /// the user's latest message.
    Llm { intent: Intent },
    /// A UI button (no utterance to cross-check).
    Ui,
}

#[derive(Debug, Clone)]
pub struct ProposeRequest {
    pub action: ProposedAction,
    /// Chat session; `None` for UI-created previews.
    pub session: Option<Uuid>,
    /// The turn came from voice: the confirm window starts after read-back (ADR-0007).
    pub voice: bool,
    pub source: ProposeSource,
}

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

/// Where confirmed write commands go. The bridge client is the only production
/// implementation; the trait exists so tests can simulate a write that times out after it
/// was sent (ADR-0013) without racing the mock. Only [`Trading::confirm`] ever calls it.
#[async_trait]
pub trait WriteSink: Send + Sync {
    async fn send_write(&self, command: &Command) -> Result<TradeResult, BridgeError>;
}

#[async_trait]
impl WriteSink for BridgeClient {
    async fn send_write(&self, command: &Command) -> Result<TradeResult, BridgeError> {
        match command {
            Command::OrderSend(args) => self.order_send(args.clone()).await,
            Command::PositionModify(args) => self.position_modify(args.clone()).await,
            Command::PositionClose(args) => self.position_close(args.clone()).await,
            Command::PositionClosePartial(args) => self.position_close_partial(args.clone()).await,
            Command::OrderCancel(args) => self.order_cancel(args.clone()).await,
            other => Err(BridgeError::Protocol(format!(
                "refusing to send non-write command {}",
                other.name()
            ))),
        }
    }
}

/// Everything [`Trading`] is built from.
pub struct TradingParts {
    pub bridge: BridgeClient,
    /// Destination of confirmed writes; `None` uses `bridge`.
    pub write_sink: Option<Arc<dyn WriteSink>>,
    pub clock: ServerClock,
    pub mode: Arc<ModeMonitor>,
    pub audit: Audit,
    pub limits: Limits,
    pub config: TradingConfig,
    pub kill_switch: KillSwitch,
    pub time: Arc<dyn TimeSource>,
    pub events: EventBus,
}

#[derive(Debug, Clone)]
struct TokenMeta {
    session: Option<Uuid>,
    preview_id: Uuid,
    /// What was proposed: confirmation checks it again against the market of that moment.
    action: Option<ProposedAction>,
    stage: TokenStage,
    source: &'static str,
    issued_ms: i64,
    expires_at_ms: i64,
    /// Confirmed, cancelled, replaced or announced as expired.
    resolved: bool,
}

#[derive(Default)]
struct Registry {
    tokens: HashMap<String, TokenMeta>,
    /// The live token of each chat session (`None` key = UI-created previews).
    live: HashMap<Option<Uuid>, String>,
}

pub struct Trading {
    bridge: BridgeClient,
    sink: Arc<dyn WriteSink>,
    clock: ServerClock,
    mode: Arc<ModeMonitor>,
    audit: Audit,
    limits: Limits,
    config: TradingConfig,
    kill_switch: KillSwitch,
    tokens: TokenStore,
    time: Arc<dyn TimeSource>,
    events: EventBus,
    registry: Mutex<Registry>,
    confirm_limiter: RateLimiter,
    /// Held while a confirmed action is re-checked and sent: writes go out one at a time and
    /// each one sees the effect of the one before (TRD-1, TRD-2). Never held by the kill switch.
    write_lock: tokio::sync::Mutex<()>,
}

impl Trading {
    pub fn new(parts: TradingParts) -> Arc<Self> {
        let ttl_ms = i64::try_from(parts.config.token_ttl_secs)
            .unwrap_or(15)
            .saturating_mul(1000);
        let cap_ms = i64::try_from(parts.config.voice_token_cap_secs)
            .unwrap_or(30)
            .saturating_mul(1000);
        Arc::new(Self {
            tokens: TokenStore::new(ttl_ms, cap_ms, parts.kill_switch.clone()),
            sink: parts
                .write_sink
                .unwrap_or_else(|| Arc::new(parts.bridge.clone())),
            bridge: parts.bridge,
            clock: parts.clock,
            mode: parts.mode,
            audit: parts.audit,
            limits: parts.limits,
            config: parts.config,
            kill_switch: parts.kill_switch,
            time: parts.time,
            events: parts.events,
            registry: Mutex::new(Registry::default()),
            confirm_limiter: RateLimiter::new(CONFIRM_RATE_LIMIT, CONFIRM_RATE_WINDOW_MS),
            write_lock: tokio::sync::Mutex::new(()),
        })
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub fn now_ms(&self) -> i64 {
        self.time.now_ms()
    }

    pub fn events(&self) -> &EventBus {
        &self.events
    }

    fn reg(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|p| p.into_inner())
    }

    async fn record(&self, session: Option<Uuid>, kind: AuditKind, payload: Value) {
        self.audit.record(session, kind, payload).await;
    }

    // ------------------------------------------------------------- propose

    /// Turn a proposal into a live preview with a confirmation token.
    pub async fn propose(&self, req: ProposeRequest) -> Result<PreviewDto, TradingError> {
        match self.propose_inner(&req).await {
            Ok(dto) => Ok(dto),
            Err(e) => {
                self.audit_propose_failure(&req, &e).await;
                Err(e)
            }
        }
    }

    async fn audit_propose_failure(&self, req: &ProposeRequest, e: &TradingError) {
        let (kind, reason) = match e {
            TradingError::Gate(GateError::LimitExceeded { rule, .. }) => {
                (AuditKind::GateRejected, format!("limit:{rule}"))
            }
            TradingError::Gate(_) => (AuditKind::GateRejected, "gate".to_owned()),
            TradingError::IntentMismatch(_) => (AuditKind::GateRejected, "intent_mismatch".into()),
            TradingError::KillSwitch => (AuditKind::GateRejected, "kill_switch".into()),
            TradingError::ModeMismatch => (AuditKind::GateRejected, "mode_mismatch".into()),
            TradingError::UserIsAnswering => (AuditKind::GateRejected, "user_is_answering".into()),
            _ => (AuditKind::Error, "propose".into()),
        };
        self.record(
            req.session,
            kind,
            json!({
                "reason": reason,
                "message": e.to_string(),
                "action": serde_json::to_value(&req.action).unwrap_or(Value::Null),
                "source": source_name(&req.source),
            }),
        )
        .await;
    }

    async fn propose_inner(&self, req: &ProposeRequest) -> Result<PreviewDto, TradingError> {
        if self.mode.mismatch() {
            return Err(TradingError::ModeMismatch);
        }
        if self.kill_switch.is_engaged() {
            return Err(TradingError::KillSwitch);
        }
        if let ProposeSource::Llm { intent } = &req.source {
            if matches!(intent, Intent::Confirm | Intent::Cancel) && self.has_live(req.session) {
                return Err(TradingError::UserIsAnswering);
            }
            intent::cross_check(intent, &req.action).map_err(TradingError::IntentMismatch)?;
        }

        let snapshot = self.snapshot(&req.action).await?;
        let preview = evaluate(
            &req.action,
            &snapshot,
            &self.limits,
            self.kill_switch.is_engaged(),
        )?;

        let timing = if req.voice {
            TokenTiming::AfterReadback
        } else {
            TokenTiming::Immediate
        };
        let now = self.time.now_ms();
        let issued = self.tokens.issue(preview.clone(), timing, now);

        // One live preview per session: the new one replaces the old one.
        let replaced = {
            let mut reg = self.reg();
            reg.tokens.insert(
                issued.token.clone(),
                TokenMeta {
                    session: req.session,
                    preview_id: preview.id,
                    action: Some(req.action.clone()),
                    stage: issued.stage,
                    source: source_name(&req.source),
                    issued_ms: now,
                    expires_at_ms: issued.expires_at_ms,
                    resolved: false,
                },
            );
            let previous = reg.live.insert(req.session, issued.token.clone());
            if let Some(prev) = &previous
                && let Some(m) = reg.tokens.get_mut(prev)
            {
                m.resolved = true;
            }
            previous
        };
        if let Some(prev) = replaced {
            // Cancels the previous token and its batch sibling; "already used" is fine.
            let _ = self.tokens.cancel(&prev, now);
            self.record(
                req.session,
                AuditKind::Cancel,
                json!({ "token": token_prefix(&prev), "reason": "replaced by a newer preview" }),
            )
            .await;
        }

        self.record(
            req.session,
            AuditKind::Preview,
            json!({
                "preview_id": preview.id,
                "description": preview.description,
                "symbols": preview.symbols,
                "tickets": preview.tickets,
                "total_volume": preview.total_volume,
                "est_pnl": preview.est_pnl,
                "is_batch": preview.is_batch,
                "count": preview.count,
                "warnings": preview.warnings,
                "commands": serde_json::to_value(&preview.commands).unwrap_or(Value::Null),
                "stage": issued.stage,
                "timing": timing,
                "expires_at_ms": issued.expires_at_ms,
                "token": token_prefix(&issued.token),
                "source": source_name(&req.source),
            }),
        )
        .await;
        Ok(PreviewDto::new(&preview, &issued, timing))
    }

    fn has_live(&self, session: Option<Uuid>) -> bool {
        let reg = self.reg();
        reg.live
            .get(&session)
            .and_then(|t| reg.tokens.get(t))
            .is_some_and(|m| !m.resolved)
    }

    /// Fetch everything `evaluate` needs, just before it runs.
    async fn snapshot(&self, action: &ProposedAction) -> Result<MarketSnapshot, TradingError> {
        let ping = self.bridge.ping().await?;
        self.clock
            .observe(ping.server_time, chrono::Utc::now().timestamp());
        self.mode.observe(ping.trade_mode);
        if self.mode.mismatch() {
            return Err(TradingError::ModeMismatch);
        }
        let account = self.bridge.get_account().await?;
        self.mode.observe(account.trade_mode);
        if self.mode.mismatch() {
            return Err(TradingError::ModeMismatch);
        }
        let positions = self.bridge.get_positions(None).await?.positions;
        let orders = self.bridge.get_orders(None).await?.orders;

        let mut prices = HashMap::new();
        let mut symbols = HashMap::new();
        for symbol in involved_symbols(action, &positions) {
            prices.insert(symbol.clone(), self.bridge.get_price(&symbol).await?);
            symbols.insert(symbol.clone(), self.bridge.get_symbol(&symbol).await?);
        }

        let (realized_pnl_today, complete) = self.realized_pnl_today(ping.server_time).await?;
        if !complete && matches!(action, ProposedAction::Order(_)) {
            // The daily-loss limit cannot be checked, so a new order must not pass.
            return Err(TradingError::Snapshot(
                "today's trade history is too long to total, so the daily loss limit cannot be \
                 checked; new orders are refused. Closing and cancelling still work."
                    .to_owned(),
            ));
        }
        Ok(MarketSnapshot {
            account,
            positions,
            orders,
            prices,
            symbols,
            server_time: ping.server_time,
            realized_pnl_today,
        })
    }

    /// Realised P/L (profit + commission + swap of trade deals) since server midnight.
    /// Returns `(total, complete)`; `complete` is false when the day held more deals than
    /// could be paged through. The sum is done here, never by the model (ADR-0010).
    async fn realized_pnl_today(&self, server_time: i64) -> Result<(f64, bool), BridgeError> {
        let from = server_time - server_time.rem_euclid(DAY_SECS);
        let mut to = from + DAY_SECS - 1;
        let mut seen: HashSet<u64> = HashSet::new();
        let mut sum = 0.0;
        for _ in 0..HISTORY_PAGES {
            let history = self
                .bridge
                .get_history(HistoryQuery {
                    from,
                    to,
                    symbol: None,
                    limit: Some(HISTORY_LIMIT_MAX),
                })
                .await?;
            let mut fresh = 0;
            for d in &history.deals {
                if seen.insert(d.ticket) {
                    fresh += 1;
                    if matches!(d.deal_type, DealType::Buy | DealType::Sell) {
                        sum += d.profit + d.commission + d.swap;
                    }
                }
            }
            if !history.truncated {
                return Ok((round2(sum), true));
            }
            // Newest first: continue below the oldest deal of this page (ties are de-duplicated).
            let Some(oldest) = history.deals.iter().map(|d| d.time).min() else {
                return Ok((round2(sum), false));
            };
            if fresh == 0 || oldest < from {
                return Ok((round2(sum), false));
            }
            to = oldest;
        }
        Ok((round2(sum), false))
    }

    // ------------------------------------------------------------- confirm

    /// Handle `POST /api/confirm/{token}`.
    ///
    /// `rate_key` identifies the caller for the 10/min limit (session or IP).
    pub async fn confirm(
        self: &Arc<Self>,
        token: &str,
        rate_key: &str,
    ) -> Result<ConfirmResponse, TradingError> {
        let now = self.time.now_ms();
        if let Err(retry_after_secs) = self.confirm_limiter.check(rate_key, now) {
            return Err(TradingError::RateLimited { retry_after_secs });
        }
        let meta = self.reg().tokens.get(token).cloned();
        let session = meta.as_ref().and_then(|m| m.session);
        let blocked = if self.mode.mismatch() {
            Some((TradingError::ModeMismatch, "mode_mismatch"))
        } else if self.kill_switch.is_engaged() {
            Some((TradingError::KillSwitch, "kill_switch"))
        } else {
            None
        };
        if let Some((error, reason)) = blocked {
            self.record(
                session,
                AuditKind::Confirm,
                json!({ "token": token_prefix(token), "accepted": false, "reason": reason }),
            )
            .await;
            return Err(error);
        }
        let outcome = match self.tokens.confirm(token, now) {
            Ok(o) => o,
            Err(e) => {
                self.record(
                    session,
                    AuditKind::Confirm,
                    json!({ "token": token_prefix(token), "accepted": false, "reason": e.to_string() }),
                )
                .await;
                return Err(e.into());
            }
        };

        match outcome {
            ConfirmOutcome::NeedsSecond {
                token: second,
                count,
                message,
            } => {
                self.record(
                    session,
                    AuditKind::Confirm,
                    json!({
                        "token": token_prefix(token),
                        "accepted": true,
                        "stage": "first",
                        "preview_id": second.preview_id,
                        "count": count,
                    }),
                )
                .await;
                {
                    let mut reg = self.reg();
                    if let Some(first) = reg.tokens.get_mut(token) {
                        first.resolved = true;
                    }
                    let (source, issued_ms) = meta
                        .as_ref()
                        .map_or(("llm", now), |m| (m.source, m.issued_ms));
                    reg.tokens.insert(
                        second.token.clone(),
                        TokenMeta {
                            session,
                            preview_id: second.preview_id,
                            action: meta.as_ref().and_then(|m| m.action.clone()),
                            stage: second.stage,
                            source,
                            issued_ms,
                            expires_at_ms: second.expires_at_ms,
                            resolved: false,
                        },
                    );
                    if reg.live.get(&session).is_some_and(|t| t == token) {
                        reg.live.insert(session, second.token.clone());
                    }
                }
                Ok(ConfirmResponse::NeedsSecondConfirmation {
                    preview_id: second.preview_id,
                    token: second.token,
                    stage: second.stage,
                    expires_at_ms: second.expires_at_ms,
                    count,
                    message,
                })
            }
            ConfirmOutcome::Execute(preview) => {
                let stage = meta.as_ref().map_or(TokenStage::First, |m| m.stage);
                {
                    let mut reg = self.reg();
                    if let Some(m) = reg.tokens.get_mut(token) {
                        m.resolved = true;
                    }
                    if reg.live.get(&session).is_some_and(|t| t == token) {
                        reg.live.remove(&session);
                    }
                }
                self.record(
                    session,
                    if stage == TokenStage::Second {
                        AuditKind::ConfirmSecond
                    } else {
                        AuditKind::Confirm
                    },
                    json!({
                        "token": token_prefix(token),
                        "accepted": true,
                        "stage": stage,
                        "preview_id": preview.id,
                        "count": preview.count,
                    }),
                )
                .await;
                // Run the writes on their own task: if the HTTP client disconnects, a
                // half-finished batch must still run to the end and be audited.
                let this = Arc::clone(self);
                let action = meta.and_then(|m| m.action);
                tokio::spawn(async move { this.execute(preview, session, action).await })
                    .await
                    .map_err(|e| TradingError::Internal(format!("execution task failed: {e}")))?
            }
        }
    }

    /// Check a confirmed preview once more and send its commands one by one.
    ///
    /// Runs under the write lock, so a second confirmation waits until this one has finished
    /// and then sees its effect on the account.
    async fn execute(
        &self,
        preview: ActionPreview,
        session: Option<Uuid>,
        action: Option<ProposedAction>,
    ) -> Result<ConfirmResponse, TradingError> {
        if let Some(bad) = preview.commands.iter().find(|c| !c.is_write()) {
            // Cannot happen with a correct gate; refuse rather than send a read as a "trade".
            return Err(TradingError::Internal(format!(
                "preview contained a non-write command ({}); nothing was sent",
                bad.name()
            )));
        }

        // TRD-1: one confirmed action at a time. The kill switch is looked at again as soon as
        // this action's turn comes, because the wait can be as long as the one before it.
        let _turn = self.write_lock.lock().await;
        if self.kill_switch.is_engaged() {
            self.record(
                session,
                AuditKind::KillSwitch,
                json!({ "blocked_preview": preview.id, "sent": 0 }),
            )
            .await;
            return Err(TradingError::KillSwitch);
        }

        // Is the bridge up, and is it still the account we think it is? Nothing is sent yet,
        // so a failure here is a clean "bridge unavailable", not an unknown outcome.
        match self.bridge.ping().await {
            Ok(info) => {
                self.clock
                    .observe(info.server_time, chrono::Utc::now().timestamp());
                self.mode.observe(info.trade_mode);
            }
            Err(e) => {
                self.record(
                    session,
                    AuditKind::Error,
                    json!({ "where": "confirm", "preview_id": preview.id, "message": e.to_string(), "sent": 0 }),
                )
                .await;
                return Err(TradingError::Bridge(e));
            }
        }
        if self.mode.mismatch() {
            return Err(TradingError::ModeMismatch);
        }

        // TRD-2: the limits and the market are looked at again, right before the write.
        let gone = self.recheck(&preview, session, action.as_ref()).await?;

        let total = preview.commands.len();
        let mut results: Vec<TradeResultDto> = Vec::with_capacity(total);
        let mut aborted: Option<String> = None;
        let mut unknown: Option<(usize, BridgeError)> = None;
        // Writes that were handed to the bridge (a skipped ticket does not count).
        let mut attempted = 0usize;

        for (index, command) in preview.commands.iter().enumerate() {
            // Rule 6: the kill switch is re-checked immediately before every single write.
            if self.kill_switch.is_engaged() {
                aborted = Some(
                    self.abort_for_kill_switch(session, &preview, index, attempted, &mut results)
                        .await?,
                );
                break;
            }
            if let Some(ticket) = command_ticket(command)
                && gone.contains(&ticket)
            {
                self.record(
                    session,
                    AuditKind::TradeResult,
                    json!({
                        "preview_id": preview.id,
                        "index": index,
                        "cmd": command.name(),
                        "ok": false,
                        "ticket": ticket,
                        "error": "not sent: the ticket no longer exists (re-check at confirm)",
                    }),
                )
                .await;
                results.push(failed_result(
                    Some(ticket),
                    None,
                    format!("not sent: {} no longer exists", noun_of(command, ticket)),
                ));
                continue;
            }

            self.record(
                session,
                AuditKind::BridgeRequest,
                json!({
                    "preview_id": preview.id,
                    "index": index,
                    "request": serde_json::to_value(command).unwrap_or(Value::Null),
                }),
            )
            .await;
            // The audit write above awaited: look at the switch once more, with nothing but the
            // call itself between this check and the bridge (TRD-1).
            if self.kill_switch.is_engaged() {
                aborted = Some(
                    self.abort_for_kill_switch(session, &preview, index, attempted, &mut results)
                        .await?,
                );
                break;
            }
            attempted += 1;
            match self.send_write(command).await {
                Ok(tr) => {
                    self.record(
                        session,
                        AuditKind::BridgeReply,
                        json!({ "preview_id": preview.id, "index": index, "reply": tr }),
                    )
                    .await;
                    let dto = ok_result(command, &tr);
                    self.record_trade_result(session, preview.id, index, command, &dto)
                        .await;
                    results.push(dto);
                }
                Err(e) if outcome_unknown(&e) => {
                    self.record(
                        session,
                        AuditKind::BridgeReply,
                        json!({ "preview_id": preview.id, "index": index, "error": e.to_string() }),
                    )
                    .await;
                    self.record(
                        session,
                        AuditKind::TradeResult,
                        json!({
                            "preview_id": preview.id,
                            "index": index,
                            "outcome": "unknown",
                            "error": e.to_string(),
                            "not_sent": total - index - 1,
                        }),
                    )
                    .await;
                    unknown = Some((index, e));
                    break;
                }
                Err(e) => {
                    self.record(
                        session,
                        AuditKind::BridgeReply,
                        json!({ "preview_id": preview.id, "index": index, "error": e.to_string() }),
                    )
                    .await;
                    let dto = error_result(command, &e);
                    self.record_trade_result(session, preview.id, index, command, &dto)
                        .await;
                    results.push(dto);
                }
            }
        }

        let any_ok = results.iter().any(|r| r.ok);
        if any_ok || unknown.is_some() {
            // Whatever happened, the UI must re-read positions.
            self.events.emit(Event::PositionsChanged);
        }
        if any_ok {
            self.events.emit(Event::TradeExecuted {
                preview_id: preview.id,
                results: results.clone(),
            });
        }

        if let Some((index, e)) = unknown {
            return Err(TradingError::OutcomeUnknown(format!(
                "The bridge did not answer write {} of {total} ({e}), so it is unknown whether \
                 it was executed. {} earlier write(s) completed. The remaining writes were NOT \
                 sent. Check your positions in MetaTrader 5 before doing anything else.",
                index + 1,
                results.len()
            )));
        }
        if total == 1 && !any_ok {
            let message = results
                .first()
                .and_then(|r| r.error.clone())
                .unwrap_or_else(|| "MetaTrader 5 rejected the request".to_owned());
            return Err(TradingError::TradeFailed(message));
        }
        Ok(ConfirmResponse::Executed {
            preview_id: preview.id,
            results,
            aborted,
        })
    }

    /// The kill switch went on while this action was waiting or running. Nothing more is sent.
    /// Fails with `KillSwitch` when not a single write went out; otherwise lists the rest as
    /// not sent and returns the reason for `aborted`.
    async fn abort_for_kill_switch(
        &self,
        session: Option<Uuid>,
        preview: &ActionPreview,
        index: usize,
        attempted: usize,
        results: &mut Vec<TradeResultDto>,
    ) -> Result<String, TradingError> {
        let total = preview.commands.len();
        self.record(
            session,
            AuditKind::KillSwitch,
            json!({
                "blocked_preview": preview.id,
                "sent": attempted,
                "skipped": total - index,
            }),
        )
        .await;
        if attempted == 0 {
            return Err(TradingError::KillSwitch);
        }
        for skipped in &preview.commands[index..] {
            results.push(failed_result(
                command_ticket(skipped),
                None,
                "not sent: the kill switch was engaged".to_owned(),
            ));
        }
        Ok("kill_switch".to_owned())
    }

    /// TRD-2: look at a confirmed action again, just before it is sent.
    ///
    /// - A new order is evaluated against a fresh snapshot, exactly as at preview time. It must
    ///   pass, and the order it would now produce must be the order the user confirmed (symbol,
    ///   side, type, volume, price, SL and TP). Nothing is adjusted: a market that moved so
    ///   that the stops no longer fit means "refused", and the confirmed commands are what gets
    ///   sent, never the new ones.
    /// - A close, cancel or modify needs its tickets to exist. The tickets that are gone are
    ///   returned (the caller skips and reports them); if none is left the action is refused.
    ///
    /// Every refusal is audited as `gate_rejected` with the reason `re-check at confirm`.
    async fn recheck(
        &self,
        preview: &ActionPreview,
        session: Option<Uuid>,
        action: Option<&ProposedAction>,
    ) -> Result<HashSet<u64>, TradingError> {
        let opens = preview
            .commands
            .iter()
            .any(|c| matches!(c, Command::OrderSend(_)));
        if opens {
            self.recheck_order(preview, session, action).await?;
            return Ok(HashSet::new());
        }
        self.recheck_tickets(preview, session).await
    }

    async fn recheck_order(
        &self,
        preview: &ActionPreview,
        session: Option<Uuid>,
        action: Option<&ProposedAction>,
    ) -> Result<(), TradingError> {
        let Some(action @ ProposedAction::Order(_)) = action else {
            // An order whose proposal is not on record cannot be checked, so it is not sent.
            let message = "The proposal behind this order is no longer on record, so the limits \
                           cannot be checked again. Nothing was sent; ask for the order again.";
            self.reject_at_confirm(session, preview, "no_proposal", message)
                .await;
            return Err(TradingError::RecheckRejected(message.to_owned()));
        };
        let snapshot = match self.snapshot(action).await {
            Ok(s) => s,
            Err(e) => {
                self.record(
                    session,
                    AuditKind::Error,
                    json!({
                        "where": "confirm",
                        "reason": RECHECK_REASON,
                        "preview_id": preview.id,
                        "message": e.to_string(),
                        "sent": 0,
                    }),
                )
                .await;
                return Err(e);
            }
        };
        let fresh = match evaluate(
            action,
            &snapshot,
            &self.limits,
            self.kill_switch.is_engaged(),
        ) {
            Ok(fresh) => fresh,
            Err(GateError::KillSwitchEngaged) => return Err(TradingError::KillSwitch),
            Err(e) => {
                let rule = match &e {
                    GateError::LimitExceeded { rule, .. } => format!("limit:{rule}"),
                    _ => "gate".to_owned(),
                };
                let message = format!(
                    "The checks were run again at confirmation and now refuse this order: {e}. \
                     Nothing was sent; ask for the order again if you still want it."
                );
                self.reject_at_confirm(session, preview, &rule, &message)
                    .await;
                return Err(TradingError::RecheckRejected(message));
            }
        };
        if let Err(what) = same_order(&preview.commands, &fresh.commands) {
            let message = format!(
                "The order the checks would build now is not the one you confirmed ({what}). \
                 Nothing was sent; ask for the order again."
            );
            self.reject_at_confirm(session, preview, "order_changed", &message)
                .await;
            return Err(TradingError::RecheckRejected(message));
        }
        Ok(())
    }

    async fn recheck_tickets(
        &self,
        preview: &ActionPreview,
        session: Option<Uuid>,
    ) -> Result<HashSet<u64>, TradingError> {
        let wants_positions = preview.commands.iter().any(|c| {
            matches!(
                c,
                Command::PositionClose(_)
                    | Command::PositionClosePartial(_)
                    | Command::PositionModify(_)
            )
        });
        let wants_orders = preview
            .commands
            .iter()
            .any(|c| matches!(c, Command::OrderCancel(_)));
        let mut open: HashSet<u64> = HashSet::new();
        let read = async {
            if wants_positions {
                open.extend(
                    self.bridge
                        .get_positions(None)
                        .await?
                        .positions
                        .iter()
                        .map(|p| p.ticket),
                );
            }
            if wants_orders {
                open.extend(
                    self.bridge
                        .get_orders(None)
                        .await?
                        .orders
                        .iter()
                        .map(|o| o.ticket),
                );
            }
            Ok::<(), BridgeError>(())
        };
        if let Err(e) = read.await {
            self.record(
                session,
                AuditKind::Error,
                json!({
                    "where": "confirm",
                    "reason": RECHECK_REASON,
                    "preview_id": preview.id,
                    "message": e.to_string(),
                    "sent": 0,
                }),
            )
            .await;
            return Err(TradingError::Bridge(e));
        }
        let gone: HashSet<u64> = preview
            .commands
            .iter()
            .filter_map(command_ticket)
            .filter(|t| !open.contains(t))
            .collect();
        let wanted = preview.commands.iter().filter_map(command_ticket).count();
        if wanted > 0 && gone.len() >= wanted {
            let message = match preview
                .commands
                .first()
                .and_then(|c| command_ticket(c).map(|t| noun_of(c, t)))
            {
                Some(noun) if wanted == 1 => format!("{noun} no longer exists"),
                _ => format!("none of the {wanted} tickets exist any more"),
            };
            self.reject_at_confirm(session, preview, "ticket_gone", &message)
                .await;
            return Err(TradingError::RecheckRejected(format!(
                "The checks were run again at confirmation: {message}. Nothing was sent."
            )));
        }
        if !gone.is_empty() {
            let mut list: Vec<u64> = gone.iter().copied().collect();
            list.sort_unstable();
            self.record(
                session,
                AuditKind::GateRejected,
                json!({
                    "reason": RECHECK_REASON,
                    "rule": "ticket_gone",
                    "preview_id": preview.id,
                    "partial": true,
                    "tickets_skipped": list,
                }),
            )
            .await;
        }
        Ok(gone)
    }

    async fn reject_at_confirm(
        &self,
        session: Option<Uuid>,
        preview: &ActionPreview,
        rule: &str,
        message: &str,
    ) {
        self.record(
            session,
            AuditKind::GateRejected,
            json!({
                "reason": RECHECK_REASON,
                "rule": rule,
                "preview_id": preview.id,
                "message": message,
                "commands": serde_json::to_value(&preview.commands).unwrap_or(Value::Null),
            }),
        )
        .await;
    }

    async fn record_trade_result(
        &self,
        session: Option<Uuid>,
        preview_id: Uuid,
        index: usize,
        command: &Command,
        dto: &TradeResultDto,
    ) {
        self.record(
            session,
            AuditKind::TradeResult,
            json!({
                "preview_id": preview_id,
                "index": index,
                "cmd": command.name(),
                "ok": dto.ok,
                "ticket": dto.ticket,
                "retcode": dto.retcode,
                "retcode_description": dto.retcode_description,
                "order": dto.order,
                "deal": dto.deal,
                "price": dto.price,
                "volume": dto.volume,
                "error": dto.error,
            }),
        )
        .await;
    }

    /// The only place a write leaves this crate (SPEC rule 3).
    async fn send_write(&self, command: &Command) -> Result<TradeResult, BridgeError> {
        let deviation = Some(self.config.deviation_points);
        let command = match command {
            Command::OrderSend(args) => {
                let mut args = args.clone();
                args.deviation_points = args.deviation_points.or(deviation);
                Command::OrderSend(args)
            }
            Command::PositionClose(args) => {
                let mut args = args.clone();
                args.deviation_points = args.deviation_points.or(deviation);
                Command::PositionClose(args)
            }
            Command::PositionClosePartial(args) => {
                let mut args = args.clone();
                args.deviation_points = args.deviation_points.or(deviation);
                Command::PositionClosePartial(args)
            }
            other => other.clone(),
        };
        self.sink.send_write(&command).await
    }

    // ------------------------------------------------------ cancel / readback

    /// Handle `POST /api/cancel/{token}`; also cancels the batch sibling.
    pub async fn cancel(&self, token: &str) -> Result<(), TradingError> {
        let now = self.time.now_ms();
        let meta = self.reg().tokens.get(token).cloned();
        let session = meta.as_ref().and_then(|m| m.session);
        self.tokens.cancel(token, now)?;
        {
            let mut reg = self.reg();
            if let Some(m) = reg.tokens.get_mut(token) {
                m.resolved = true;
            }
            if reg.live.get(&session).is_some_and(|t| t == token) {
                reg.live.remove(&session);
            }
        }
        self.record(
            session,
            AuditKind::Cancel,
            json!({
                "token": token_prefix(token),
                "preview_id": meta.map(|m| m.preview_id),
                "reason": "user",
            }),
        )
        .await;
        Ok(())
    }

    /// Handle `POST /api/readback/{token}`: starts the voice confirm window. Returns the new
    /// expiry (Unix ms).
    pub fn readback_finished(&self, token: &str) -> Result<i64, TradingError> {
        let issued = self.tokens.readback_finished(token, self.time.now_ms())?;
        if let Some(m) = self.reg().tokens.get_mut(token) {
            m.expires_at_ms = issued.expires_at_ms;
        }
        Ok(issued.expires_at_ms)
    }

    // ----------------------------------------------------------- kill switch

    pub fn kill_switch_state(&self) -> KillSwitchState {
        self.kill_switch.state()
    }

    pub fn kill_switch_engaged(&self) -> bool {
        self.kill_switch.is_engaged()
    }

    /// Engage the kill switch and revoke every live token (SPEC rule 6).
    pub async fn engage_kill_switch(&self, reason: &str) -> KillSwitchState {
        let now = self.time.now_ms();
        self.kill_switch.engage(reason, now);
        self.tokens.revoke_all();
        {
            let mut reg = self.reg();
            reg.live.clear();
            for m in reg.tokens.values_mut() {
                m.resolved = true;
            }
        }
        let state = self.kill_switch.state();
        self.persist_kill_switch(&state).await;
        self.record(
            None,
            AuditKind::KillSwitch,
            json!({ "engaged": true, "reason": reason }),
        )
        .await;
        self.events.emit(Event::KillSwitch {
            engaged: true,
            reason: Some(reason.to_owned()),
        });
        state
    }

    /// Release the kill switch. The caller has already authorised the release.
    pub async fn release_kill_switch(&self) -> KillSwitchState {
        self.kill_switch.release(self.time.now_ms());
        let state = self.kill_switch.state();
        self.persist_kill_switch(&state).await;
        self.record(None, AuditKind::KillSwitch, json!({ "engaged": false }))
            .await;
        self.events.emit(Event::KillSwitch {
            engaged: false,
            reason: None,
        });
        state
    }

    /// Save the kill switch so a restart does not undo it. A failed save is logged loudly: the
    /// in-memory switch is already in effect, and a failed *release* save leaves the saved
    /// state engaged, which errs on the safe side.
    async fn persist_kill_switch(&self, state: &KillSwitchState) {
        if let Err(e) = self.audit.save_kill_switch(state).await {
            tracing::error!("KILL SWITCH STATE NOT SAVED: it will not survive a restart: {e:#}");
        }
    }

    /// Startup: re-engage the kill switch if it was engaged when the server last stopped. If
    /// the saved state cannot be read, trading stays off (fail closed).
    pub async fn restore_kill_switch(&self) {
        let now = self.time.now_ms();
        let (reason, changed_at_ms) = match self.audit.load_kill_switch().await {
            Ok(Some(saved)) if saved.engaged => (
                saved
                    .reason
                    .unwrap_or_else(|| "engaged before the restart".to_owned()),
                saved.changed_at_ms.unwrap_or(now),
            ),
            Ok(_) => return,
            Err(e) => {
                tracing::error!("cannot read the saved kill switch state: {e:#}");
                (
                    "the saved kill switch state could not be read".to_owned(),
                    now,
                )
            }
        };
        self.kill_switch.engage(reason.clone(), changed_at_ms);
        tracing::warn!(
            "the kill switch was engaged when the server last stopped and stays engaged ({reason}); release it in the app with a fresh authenticator code"
        );
        self.record(
            None,
            AuditKind::KillSwitch,
            json!({ "engaged": true, "reason": reason, "restored_after_restart": true }),
        )
        .await;
    }

    // ----------------------------------------------------------------- purge

    /// Housekeeping, run every few seconds: drops old tokens, announces previews that
    /// expired unanswered, forgets idle rate-limit keys. Returns how many previews
    /// expired.
    pub fn purge(&self) -> usize {
        let now = self.time.now_ms();
        self.tokens.purge(now);
        self.confirm_limiter.purge(now);
        let mut expired = Vec::new();
        {
            let mut reg = self.reg();
            for (token, m) in reg.tokens.iter_mut() {
                if !m.resolved && m.expires_at_ms <= now {
                    m.resolved = true;
                    expired.push(token.clone());
                }
            }
            reg.tokens
                .retain(|_, m| now - m.issued_ms < META_RETENTION_MS);
            let Registry { tokens, live } = &mut *reg;
            live.retain(|_, t| tokens.get(t).is_some_and(|m| !m.resolved));
        }
        for token in &expired {
            self.events.emit(Event::PreviewExpired {
                token: token.clone(),
            });
        }
        expired.len()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn source_name(source: &ProposeSource) -> &'static str {
    match source {
        ProposeSource::Llm { .. } => "llm",
        ProposeSource::Ui => "ui",
    }
}

/// First 8 characters of a token: enough to correlate audit rows, useless to replay.
fn token_prefix(token: &str) -> String {
    token.chars().take(8).collect()
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Symbols whose price and trading specs the gate needs for `action`: the traded symbol of
/// a new order or a modify, and the symbols of a partial close. Full closes and cancels work
/// from the positions/orders alone, so a broken quote can never stop a risk-reducing action.
fn involved_symbols(action: &ProposedAction, positions: &[Position]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    match action {
        ProposedAction::Order(o) => {
            out.insert(o.symbol.clone());
        }
        ProposedAction::Modify(m) => {
            out.extend(
                positions
                    .iter()
                    .filter(|p| p.ticket == m.ticket)
                    .map(|p| p.symbol.clone()),
            );
        }
        ProposedAction::Close(sel) if sel.volume.is_some() => {
            out.extend(
                positions
                    .iter()
                    .filter(|p| sel.tickets.contains(&p.ticket))
                    .map(|p| p.symbol.clone()),
            );
        }
        ProposedAction::Close(_) | ProposedAction::Cancel(_) => {}
    }
    out
}

/// True when a write may have reached MT5 although no usable reply came back.
///
/// `BridgeError` has no explicit "not sent" variant, so the two failures that happen before
/// anything is delivered (connect refused, send failed) are recognised by their text; every
/// other transport/protocol/timeout failure is treated as unknown (ADR-0013).
pub fn outcome_unknown(e: &BridgeError) -> bool {
    match e {
        BridgeError::Timeout(_) => true,
        BridgeError::Transport(m) => {
            !(m.starts_with("cannot connect to") || m.starts_with("send failed"))
        }
        BridgeError::Protocol(m) => !m.starts_with("could not encode request"),
        BridgeError::Remote { .. } => false,
    }
}

fn is_success_retcode(retcode: i64) -> bool {
    matches!(
        retcode,
        TRADE_RETCODE_PLACED | TRADE_RETCODE_DONE | TRADE_RETCODE_DONE_PARTIAL
    )
}

/// The ticket a write is about (not known for a new order until MT5 replies).
fn command_ticket(command: &Command) -> Option<u64> {
    match command {
        Command::PositionModify(a) => Some(a.ticket),
        Command::PositionClose(a) => Some(a.ticket),
        Command::PositionClosePartial(a) => Some(a.ticket),
        Command::OrderCancel(a) => Some(a.ticket),
        _ => None,
    }
}

/// "position 123" / "pending order 456", for messages.
fn noun_of(command: &Command, ticket: u64) -> String {
    match command {
        Command::OrderCancel(_) => format!("pending order {ticket}"),
        _ => format!("position {ticket}"),
    }
}

/// Is the order built now the order that was confirmed? Compared on what the user saw and
/// agreed to: symbol, side, type, volume, price, SL and TP. (The comment carries the preview id
/// and always differs.)
fn same_order(confirmed: &[Command], fresh: &[Command]) -> Result<(), String> {
    let (Some(Command::OrderSend(a)), Some(Command::OrderSend(b)), 1, 1) = (
        confirmed.first(),
        fresh.first(),
        confirmed.len(),
        fresh.len(),
    ) else {
        return Err("the commands differ".to_owned());
    };
    let near = |x: f64, y: f64| (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0);
    let near_opt = |x: Option<f64>, y: Option<f64>| match (x, y) {
        (None, None) => true,
        (Some(x), Some(y)) => near(x, y),
        _ => false,
    };
    if a.symbol != b.symbol {
        return Err(format!("symbol {} against {}", a.symbol, b.symbol));
    }
    if a.side != b.side {
        return Err("side".to_owned());
    }
    if a.exec != b.exec {
        return Err("order type".to_owned());
    }
    if !near(a.volume, b.volume) {
        return Err(format!("volume {} against {}", a.volume, b.volume));
    }
    if !near_opt(a.price, b.price) {
        return Err("price".to_owned());
    }
    if !near_opt(a.sl, b.sl) {
        return Err("stop loss".to_owned());
    }
    if !near_opt(a.tp, b.tp) {
        return Err("take profit".to_owned());
    }
    Ok(())
}

fn ok_result(command: &Command, tr: &TradeResult) -> TradeResultDto {
    let ok = is_success_retcode(tr.retcode);
    let ticket = command_ticket(command).or((tr.order != 0 && ok).then_some(tr.order));
    TradeResultDto {
        ticket,
        ok,
        retcode: Some(tr.retcode),
        retcode_description: Some(tr.retcode_description.clone()),
        order: Some(tr.order),
        deal: Some(tr.deal),
        price: Some(tr.price),
        volume: Some(tr.volume),
        error: (!ok).then(|| {
            format!(
                "MT5 did not accept the request: {} (retcode {})",
                tr.retcode_description, tr.retcode
            )
        }),
    }
}

fn error_result(command: &Command, e: &BridgeError) -> TradeResultDto {
    match e {
        BridgeError::Remote { message, retcode } => TradeResultDto {
            ticket: command_ticket(command),
            ok: false,
            retcode: Some(*retcode),
            retcode_description: Some(message.clone()),
            order: None,
            deal: None,
            price: None,
            volume: None,
            error: Some(format!(
                "MT5 rejected the request: {message} (retcode {retcode})"
            )),
        },
        other => failed_result(
            command_ticket(command),
            None,
            format!("not executed: {other}"),
        ),
    }
}

fn failed_result(ticket: Option<u64>, retcode: Option<i64>, error: String) -> TradeResultDto {
    TradeResultDto {
        ticket,
        ok: false,
        retcode,
        retcode_description: None,
        order: None,
        deal: None,
        price: None,
        volume: None,
        error: Some(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_bridge_client::protocol::{PositionCloseArgs, PositionModifyArgs};

    #[test]
    fn unknown_outcome_classification() {
        assert!(outcome_unknown(&BridgeError::Timeout(3000)));
        assert!(outcome_unknown(&BridgeError::Transport(
            "receive failed: broken pipe".into()
        )));
        assert!(!outcome_unknown(&BridgeError::Transport(
            "cannot connect to tcp://127.0.0.1:5555: refused".into()
        )));
        assert!(!outcome_unknown(&BridgeError::Transport(
            "send failed: closed".into()
        )));
        assert!(outcome_unknown(&BridgeError::Protocol(
            "reply id mismatch".into()
        )));
        assert!(!outcome_unknown(&BridgeError::Protocol(
            "could not encode request: x".into()
        )));
        assert!(!outcome_unknown(&BridgeError::Remote {
            message: "m".into(),
            retcode: 10013
        }));
    }

    #[test]
    fn result_mapping() {
        let close = Command::PositionClose(PositionCloseArgs {
            ticket: 7,
            deviation_points: None,
        });
        let tr = TradeResult {
            retcode: 10009,
            retcode_description: "done".into(),
            order: 11,
            deal: 12,
            volume: 0.1,
            price: 2391.1,
            comment: String::new(),
        };
        let dto = ok_result(&close, &tr);
        assert!(dto.ok);
        assert_eq!(dto.ticket, Some(7));
        assert_eq!(dto.deal, Some(12));
        assert_eq!(dto.error, None);

        let bad = TradeResult {
            retcode: 10013,
            retcode_description: "invalid request".into(),
            ..tr
        };
        let dto = ok_result(&close, &bad);
        assert!(!dto.ok);
        assert!(dto.error.unwrap().contains("10013"));

        let modify = Command::PositionModify(PositionModifyArgs {
            ticket: 9,
            sl: Some(1.0),
            tp: None,
        });
        let dto = error_result(
            &modify,
            &BridgeError::Remote {
                message: "invalid stops".into(),
                retcode: 10016,
            },
        );
        assert!(!dto.ok);
        assert_eq!(dto.ticket, Some(9));
        assert_eq!(dto.retcode, Some(10016));
        assert!(dto.error.unwrap().contains("invalid stops"));
    }

    #[test]
    fn confirm_response_shapes() {
        let id = Uuid::nil();
        let v = serde_json::to_value(ConfirmResponse::NeedsSecondConfirmation {
            preview_id: id,
            token: "t".into(),
            stage: TokenStage::Second,
            expires_at_ms: 5,
            count: 3,
            message: "Confirm again".into(),
        })
        .unwrap();
        assert_eq!(v["status"], "needs_second_confirmation");
        assert_eq!(v["stage"], "second");
        let v = serde_json::to_value(ConfirmResponse::Executed {
            preview_id: id,
            results: vec![],
            aborted: None,
        })
        .unwrap();
        assert_eq!(v["status"], "executed");
        assert!(v.get("aborted").is_none());
    }

    #[test]
    fn token_prefix_is_short() {
        assert_eq!(token_prefix("0123456789abcdef"), "01234567");
        assert_eq!(token_prefix("ab"), "ab");
    }
}
