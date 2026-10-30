//! HTTP routes. The contract is `docs/api.md`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::handler::Handler;
use axum::http::request::Parts;
use axum::http::{Extensions, HeaderValue, Method, StatusCode, header};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use jarvis_bridge_client::BridgeError;
use jarvis_bridge_client::protocol::{Account, Orders, Positions, Price, Side};
use jarvis_gate::{CancelSelector, CloseSelector, KillSwitchState, ProposedAction};
use jarvis_llm::LlmError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::task::JoinSet;
use tower_http::cors::CorsLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use crate::agent::ToolCallSummary;
use crate::auth::http::{self as auth_http, MaybeSession, authenticate_headers, peer_ip_of};
use crate::security::{self, Security};
use crate::state::{AppState, Budget, ProviderSlot};
use crate::static_site::StaticSite;
use crate::trading::{ConfirmResponse, PreviewDto, ProposeRequest, ProposeSource, TradingError};
use crate::{voice, ws};

/// Longest accepted chat message, in characters.
pub const MAX_MESSAGE_CHARS: usize = 2000;
/// Largest request body of the JSON routes.
pub const JSON_BODY_LIMIT: usize = 64 * 1024;
/// Largest request body of `POST /api/voice/transcribe` (WAV audio).
pub const VOICE_BODY_LIMIT: usize = 5 * 1024 * 1024;

/// Routes that need no session: the health probe and login.
pub const PUBLIC_ROUTES: [(&str, &str); 2] = [("GET", "/api/health"), ("POST", "/api/auth/login")];

/// The whole API (and the static SPA) as a router. Tests call this with a hand-built
/// [`AppState`].
///
/// Every `/api` route sits behind the session middleware except [`PUBLIC_ROUTES`]; an
/// unknown `/api` path gets the same check (401 before 404). Everything outside `/api` is
/// the static site, when one is configured.
pub fn app(state: AppState) -> Router {
    // CORS is off unless `[server] dev_origins` names origins. The SPA is served from the API's
    // own origin (also in the desktop shell), and the Vite dev proxy is same-origin too.
    let dev_origins: Vec<HeaderValue> = state
        .config()
        .server
        .dev_origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();
    let cors = CorsLayer::new()
        .allow_origin(dev_origins)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE])
        .allow_credentials(true);

    // Time limits (NET-1). Most routes answer within `request_timeout_secs`; the ones that wait
    // for a language model, speech or a batch of trades get `long_request_timeout_secs`; the
    // WebSocket has none (the upgrade is answered at once, and what follows is not a request).
    // A request that runs out of time gets a 408 in the standard error shape (`security::apply`).
    let server = &state.config().server;
    let timeout = |secs: u64| {
        TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, Duration::from_secs(secs))
    };
    let (short, long) = (
        timeout(server.request_timeout_secs),
        timeout(server.long_request_timeout_secs),
    );

    let public = Router::new()
        .route("/api/health", get(health))
        .route("/api/auth/login", post(auth_http::login))
        .layer(short);

    let quick = Router::new()
        .route("/api/status", get(status))
        .route("/api/account", get(account))
        .route("/api/positions", get(positions))
        .route("/api/orders", get(orders))
        .route("/api/watchlist", get(watchlist))
        .route("/api/cancel/{token}", post(cancel))
        .route("/api/readback/{token}", post(readback))
        .route("/api/positions/{ticket}/close", post(close_position))
        .route("/api/positions/close", post(close_positions))
        .route("/api/orders/{ticket}/cancel", post(cancel_order))
        .route("/api/killswitch", get(killswitch_get).post(killswitch_set))
        .route("/api/audit", get(audit_log))
        .route("/api/auth/logout", post(auth_http::logout))
        .route("/api/auth/me", get(auth_http::me))
        .layer(short);

    // Chat and transcription also have a per-session request budget (NET-2), checked before the
    // body is read.
    let budget = |which: Budget| from_fn_with_state((state.clone(), which), spend_budget);
    let slow = Router::new()
        .route("/api/chat", post(chat).layer(budget(Budget::Chat)))
        .route("/api/confirm/{token}", post(confirm))
        .route("/api/voice/speak", post(voice::speak))
        .route(
            "/api/voice/transcribe",
            post(voice::transcribe)
                .layer(DefaultBodyLimit::max(VOICE_BODY_LIMIT))
                .layer(budget(Budget::Transcribe)),
        )
        .layer(long);

    let sockets = Router::new().route("/api/ws", get(ws::handler));

    let protected = quick
        .merge(slow)
        .merge(sockets)
        .route_layer(from_fn_with_state(
            state.clone(),
            auth_http::require_session,
        ));

    let config = state.config();
    let site = config.server.static_dir.as_deref().map(StaticSite::new);
    let script_hashes = config
        .server
        .static_dir
        .as_deref()
        .map(security::collect_inline_script_hashes)
        .unwrap_or_default();
    let hardening = Arc::new(Security::new(config.tls_enabled(), &script_hashes));
    // Host / Origin checks on /api (see `security::request_guard`).
    let guard = Arc::new(security::RequestGuard::new(
        &config.server.dev_origins,
        !config.auth.enabled,
    ));

    let fallback_state = state.clone();
    let fallback_handler = move |req: Request| {
        let state = fallback_state.clone();
        let site = site.clone();
        async move { fallback(state, site, req).await }
    };
    Router::new()
        .merge(public)
        .merge(protected)
        .fallback(fallback_handler.layer(short))
        .layer(DefaultBodyLimit::max(JSON_BODY_LIMIT))
        .layer(
            TraceLayer::new_for_http().make_span_with(|req: &axum::http::Request<_>| {
                // Confirmation tokens are bearer secrets: keep them out of the request log.
                tracing::debug_span!(
                    "http_request",
                    method = %req.method(),
                    path = %redact_path(req.uri().path()),
                )
            }),
        )
        .layer(cors)
        .layer(from_fn_with_state(guard, security::request_guard))
        .layer(from_fn_with_state(hardening, security::apply))
        .with_state(state)
}

/// Middleware of the expensive routes (NET-2): one request of the caller's budget, or a 429 with
/// `Retry-After`. The caller is the login session, or the peer address when login is off.
async fn spend_budget(
    State((state, budget)): State<(AppState, Budget)>,
    req: Request,
    next: Next,
) -> Response {
    let key = client_key_of(req.extensions());
    match state.spend_budget(budget, &key) {
        Ok(()) => next.run(req).await,
        Err(retry_after_secs) => {
            tracing::warn!(?budget, "request budget used up");
            ApiError::RateLimited { retry_after_secs }.into_response()
        }
    }
}

/// Unmatched requests: `/api/*` is a JSON 404 (after the session check), anything else is the
/// static site or a plain 404.
async fn fallback(state: AppState, site: Option<StaticSite>, req: Request) -> Response {
    let path = req.uri().path();
    if path == "/api" || path.starts_with("/api/") {
        return match authenticate_headers(&state, req.headers()).await {
            Err(e) => e.into_response(),
            Ok(_) => ApiError::NotFound("no such endpoint".to_owned()).into_response(),
        };
    }
    match site {
        Some(site) => site.serve(req).await,
        None => ApiError::NotFound("no web UI is configured (server.static_dir)".to_owned())
            .into_response(),
    }
}

/// `GET /api/health`: liveness for service monitors and the desktop launcher. No auth, no
/// details.
async fn health() -> Json<Value> {
    Json(json!({ "ok": true }))
}

/// `/api/confirm/<token>` -> `/api/confirm/<redacted>` (same for cancel and readback).
fn redact_path(path: &str) -> String {
    for prefix in ["/api/confirm/", "/api/cancel/", "/api/readback/"] {
        if path.starts_with(prefix) {
            return format!("{prefix}<redacted>");
        }
    }
    path.to_owned()
}

// ---------------------------------------------------------------------------
// Errors: `{"error": {"code", "message"}}`
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    BridgeUnavailable(String),
    BridgeError(String),
    LlmError(String),
    /// Additive: DEMO configured but the account is real (rule 7).
    ModeMismatch,
    /// 401: no valid session, a failed login, or a kill-switch release the guard refused.
    Unauthorized(String),
    /// 403: the request is understood and refused (foreign WebSocket origin).
    Forbidden(String),
    /// 404 for an unknown `/api` path or a missing web UI.
    NotFound(String),
    /// 405 (normalised from the framework's empty response).
    MethodNotAllowed,
    /// 408: the request took longer than the server allows (NET-1).
    RequestTimeout,
    /// 413: body over the route's limit.
    PayloadTooLarge,
    /// 422: the gate refused to build a preview.
    GateRejected(String),
    TokenUnknown,
    TokenExpired,
    TokenUsed,
    /// 423: the kill switch is engaged.
    KillSwitch,
    RateLimited {
        retry_after_secs: u64,
    },
    /// 503: voice disabled or models missing (voice task).
    VoiceUnavailable(String),
    /// 502: MT5 rejected a write.
    TradeFailed(String),
    /// 504: a write was sent and no reply came back.
    OutcomeUnknown(String),
    Internal,
}

impl ApiError {
    fn parts(&self) -> (StatusCode, &'static str, String) {
        match self {
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, "bad_request", m.clone()),
            ApiError::BridgeUnavailable(m) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "bridge_unavailable",
                m.clone(),
            ),
            ApiError::BridgeError(m) => (StatusCode::BAD_GATEWAY, "bridge_error", m.clone()),
            ApiError::LlmError(m) => (StatusCode::BAD_GATEWAY, "llm_error", m.clone()),
            ApiError::ModeMismatch => (
                StatusCode::CONFLICT,
                "mode_mismatch",
                "Jarvis is configured for a demo account but MetaTrader 5 reports a real \
                 account. Chat is disabled. Switch the terminal to a demo account, or set \
                 mode = \"live\" on purpose."
                    .to_owned(),
            ),
            ApiError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, "unauthorized", m.clone()),
            ApiError::Forbidden(m) => (StatusCode::FORBIDDEN, "forbidden", m.clone()),
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, "not_found", m.clone()),
            ApiError::MethodNotAllowed => (
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "this method is not allowed on that endpoint".to_owned(),
            ),
            ApiError::RequestTimeout => (
                StatusCode::REQUEST_TIMEOUT,
                "request_timeout",
                "the request took too long and was stopped".to_owned(),
            ),
            ApiError::PayloadTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "the request body is too large".to_owned(),
            ),
            ApiError::GateRejected(m) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "gate_rejected", m.clone())
            }
            ApiError::TokenUnknown => (
                StatusCode::NOT_FOUND,
                "token_unknown",
                "confirmation token not found".to_owned(),
            ),
            ApiError::TokenExpired => (
                StatusCode::GONE,
                "token_expired",
                "the confirmation window has closed; ask for the action again".to_owned(),
            ),
            ApiError::TokenUsed => (
                StatusCode::CONFLICT,
                "token_used",
                "this confirmation was already used or cancelled".to_owned(),
            ),
            ApiError::KillSwitch => (
                StatusCode::LOCKED,
                "kill_switch",
                "the kill switch is engaged; trading is disabled".to_owned(),
            ),
            ApiError::RateLimited { retry_after_secs } => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                format!("too many requests; retry in {retry_after_secs} s"),
            ),
            ApiError::VoiceUnavailable(m) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "voice_unavailable",
                m.clone(),
            ),
            ApiError::TradeFailed(m) => (StatusCode::BAD_GATEWAY, "trade_failed", m.clone()),
            ApiError::OutcomeUnknown(m) => {
                (StatusCode::GATEWAY_TIMEOUT, "outcome_unknown", m.clone())
            }
            ApiError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal error; see the server log".to_owned(),
            ),
        }
    }
}

impl From<BridgeError> for ApiError {
    fn from(e: BridgeError) -> Self {
        match e {
            BridgeError::Timeout(_) | BridgeError::Transport(_) => {
                ApiError::BridgeUnavailable(e.to_string())
            }
            BridgeError::Remote { .. } | BridgeError::Protocol(_) => {
                ApiError::BridgeError(e.to_string())
            }
        }
    }
}

impl From<LlmError> for ApiError {
    fn from(e: LlmError) -> Self {
        ApiError::LlmError(e.to_string())
    }
}

impl From<TradingError> for ApiError {
    fn from(e: TradingError) -> Self {
        use jarvis_gate::{GateError, TokenError};
        match e {
            TradingError::Gate(GateError::KillSwitchEngaged) | TradingError::KillSwitch => {
                ApiError::KillSwitch
            }
            TradingError::Gate(g) => ApiError::GateRejected(g.to_string()),
            TradingError::IntentMismatch(m) => ApiError::GateRejected(m.to_string()),
            TradingError::UserIsAnswering => ApiError::GateRejected(e.to_string()),
            TradingError::RecheckRejected(m) => ApiError::GateRejected(m),
            TradingError::Token(TokenError::Unknown) => ApiError::TokenUnknown,
            TradingError::Token(TokenError::Expired) => ApiError::TokenExpired,
            TradingError::Token(TokenError::AlreadyUsed) => ApiError::TokenUsed,
            TradingError::Token(TokenError::KillSwitchEngaged) => ApiError::KillSwitch,
            TradingError::ModeMismatch => ApiError::ModeMismatch,
            TradingError::Bridge(b) => b.into(),
            TradingError::Snapshot(m) => ApiError::BridgeError(m),
            TradingError::RateLimited { retry_after_secs } => {
                ApiError::RateLimited { retry_after_secs }
            }
            TradingError::TradeFailed(m) => ApiError::TradeFailed(m),
            TradingError::OutcomeUnknown(m) => ApiError::OutcomeUnknown(m),
            TradingError::Internal(m) => {
                tracing::error!("trading internal error: {m}");
                ApiError::Internal
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = self.parts();
        let retry_after = match &self {
            ApiError::RateLimited { retry_after_secs } => Some(*retry_after_secs),
            _ => None,
        };
        let mut response = (
            status,
            Json(json!({ "error": { "code": code, "message": message } })),
        )
            .into_response();
        if let Some(secs) = retry_after
            && let Ok(value) = HeaderValue::from_str(&secs.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

// ---------------------------------------------------------------------------
// GET /api/status
// ---------------------------------------------------------------------------

async fn status(State(state): State<AppState>, MaybeSession(session): MaybeSession) -> Json<Value> {
    let started = Instant::now();
    let ping = state.ping_bridge().await;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let bridge = match ping {
        Ok(info) => json!({
            "connected": true,
            "latency_ms": latency_ms,
            "ea_version": info.ea_version,
            "protocol_version": info.protocol_version,
            "trade_mode": info.trade_mode,
            "terminal_connected": info.terminal_connected,
            "error": null,
        }),
        Err(e) => json!({
            "connected": false,
            "latency_ms": null,
            "ea_version": null,
            "protocol_version": null,
            "trade_mode": null,
            "terminal_connected": null,
            "error": e.to_string(),
        }),
    };
    let providers: Vec<Value> = state
        .providers()
        .iter()
        .map(|p| {
            json!({
                "name": p.config.name,
                "kind": p.kind_str(),
                "model": p.config.model,
                "available": matches!(p.slot, ProviderSlot::Available(_)),
            })
        })
        .collect();
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "mode": state.config().mode.as_str(),
        "mode_mismatch": state.mode_mismatch(),
        "bridge": bridge,
        "llm": { "default": state.config().llm.default, "providers": providers },
        "kill_switch": state.trading().kill_switch_state(),
        "auth": state.auth_status(session.as_ref()),
        "voice": state.voice_status(),
        "limits": state.trading().limits(),
    }))
}

// ---------------------------------------------------------------------------
// GET /api/account, /api/positions
// ---------------------------------------------------------------------------

async fn account(State(state): State<AppState>) -> Result<Json<Account>, ApiError> {
    let account = state.bridge().get_account().await?;
    state.observe_trade_mode(account.trade_mode);
    Ok(Json(account))
}

async fn positions(State(state): State<AppState>) -> Result<Json<Positions>, ApiError> {
    Ok(Json(state.bridge().get_positions(None).await?))
}

/// `GET /api/orders`: pending orders, in the bridge's `Order` shape.
async fn orders(State(state): State<AppState>) -> Result<Json<Orders>, ApiError> {
    Ok(Json(state.bridge().get_orders(None).await?))
}

// ---------------------------------------------------------------------------
// GET /api/watchlist
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct WatchlistError {
    symbol: String,
    message: String,
}

#[derive(Serialize)]
struct Watchlist {
    prices: Vec<Price>,
    errors: Vec<WatchlistError>,
}

async fn watchlist(State(state): State<AppState>) -> Result<Json<Watchlist>, ApiError> {
    let names = state.config().symbols.watchlist.clone();
    let mut tasks = JoinSet::new();
    for (index, name) in names.iter().enumerate() {
        let bridge = state.bridge().clone();
        let resolved = state.resolver().resolve(name);
        let name = name.clone();
        tasks.spawn(async move {
            let result = match resolved {
                Some(symbol) => bridge.get_price(&symbol).await,
                None => Err(BridgeError::Remote {
                    message: format!("'{name}' is not a usable symbol name"),
                    retcode: 0,
                }),
            };
            (index, name, result)
        });
    }
    let mut results = Vec::with_capacity(names.len());
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(r) => results.push(r),
            Err(e) => {
                tracing::error!("watchlist task failed: {e}");
                return Err(ApiError::Internal);
            }
        }
    }
    results.sort_by_key(|(index, _, _)| *index);

    let mut prices = Vec::new();
    let mut errors = Vec::new();
    let mut unreachable: Option<BridgeError> = None;
    for (_, name, result) in results {
        match result {
            Ok(price) => prices.push(price),
            Err(e) => {
                if matches!(e, BridgeError::Timeout(_) | BridgeError::Transport(_)) {
                    unreachable.get_or_insert_with(|| e.clone());
                }
                errors.push(WatchlistError {
                    symbol: name,
                    message: e.to_string(),
                });
            }
        }
    }
    // Every symbol failed because the bridge itself is down: that is a 503, not a list of errors.
    if prices.is_empty()
        && let Some(e) = unreachable
    {
        return Err(e.into());
    }
    Ok(Json(Watchlist { prices, errors }))
}

// ---------------------------------------------------------------------------
// POST /api/chat
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ChatRequestBody {
    #[serde(default)]
    session_id: Option<String>,
    message: String,
    #[serde(default)]
    provider: Option<String>,
    /// The message was spoken: previews get the after-read-back confirm window.
    #[serde(default)]
    voice: bool,
}

#[derive(Serialize)]
struct ChatResponseBody {
    session_id: Uuid,
    reply: String,
    provider: String,
    tool_calls: Vec<ToolCallSummary>,
    preview: Option<PreviewDto>,
}

async fn chat(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<ChatResponseBody>, ApiError> {
    // Parse by hand so malformed bodies get the standard error shape.
    let req: ChatRequestBody = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))?;

    let message = req.message.trim();
    if message.is_empty() {
        return Err(ApiError::BadRequest("message must not be empty".to_owned()));
    }
    if message.chars().count() > MAX_MESSAGE_CHARS {
        return Err(ApiError::BadRequest(format!(
            "message is too long (maximum {MAX_MESSAGE_CHARS} characters)"
        )));
    }
    let session_id = match req
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some(s) => Some(
            Uuid::parse_str(s)
                .map_err(|_| ApiError::BadRequest("session_id is not a valid UUID".to_owned()))?,
        ),
    };
    let provider_name = req
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&state.config().llm.default);
    let entry = state.provider(provider_name).ok_or_else(|| {
        let known: Vec<&str> = state
            .providers()
            .iter()
            .map(|p| p.config.name.as_str())
            .collect();
        ApiError::BadRequest(format!(
            "unknown provider '{provider_name}' (available names: {})",
            known.join(", ")
        ))
    })?;
    let provider = match &entry.slot {
        ProviderSlot::Available(p) => p.clone(),
        ProviderSlot::Unavailable(reason) => {
            return Err(ApiError::BadRequest(format!(
                "provider '{provider_name}' is not available: {reason}"
            )));
        }
    };

    if state.mode_mismatch() {
        return Err(ApiError::ModeMismatch);
    }

    let outcome = match state
        .agent()
        .run_turn(provider.as_ref(), session_id, message, req.voice)
        .await
    {
        Ok(o) => o,
        Err(e) => {
            tracing::error!(provider = provider_name, "LLM call failed: {e}");
            return Err(e.into());
        }
    };
    Ok(Json(ChatResponseBody {
        session_id: outcome.session_id,
        reply: outcome.reply,
        provider: provider_name.to_owned(),
        tool_calls: outcome.tool_calls,
        preview: outcome.preview,
    }))
}

// ---------------------------------------------------------------------------
// Trading: confirm / cancel / readback
// ---------------------------------------------------------------------------

/// Who is calling, for rate limits: the login session when there is one (so `/api/confirm`
/// is limited per session), otherwise the peer IP. Requests with no peer address
/// (in-process tests) share one bucket.
fn client_key(parts: &Parts) -> String {
    client_key_of(&parts.extensions)
}

fn client_key_of(extensions: &Extensions) -> String {
    match extensions.get::<crate::auth::Session>() {
        Some(session) => format!("session:{}", session.id),
        None => peer_ip_of(extensions),
    }
}

async fn confirm(
    State(state): State<AppState>,
    Path(token): Path<String>,
    parts: Parts,
) -> Result<Json<ConfirmResponse>, ApiError> {
    let key = client_key(&parts);
    let response = state.trading().confirm(&token, &key).await?;
    Ok(Json(response))
}

async fn cancel(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.trading().cancel(&token).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn readback(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let expires_at_ms = state.trading().readback_finished(&token)?;
    Ok(Json(json!({ "expires_at_ms": expires_at_ms })))
}

// ---------------------------------------------------------------------------
// Trading: UI buttons (same gate path as the propose tools)
// ---------------------------------------------------------------------------

/// Parse an optional JSON body; an empty body means "all defaults".
fn optional_body<T: serde::de::DeserializeOwned + Default>(body: &Bytes) -> Result<T, ApiError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body)
        .map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))
}

async fn propose_from_ui(
    state: &AppState,
    action: ProposedAction,
) -> Result<Json<PreviewDto>, ApiError> {
    let preview = state
        .trading()
        .propose(ProposeRequest {
            action,
            session: None,
            voice: false,
            source: ProposeSource::Ui,
        })
        .await?;
    Ok(Json(preview))
}

#[derive(Default, Deserialize)]
struct CloseOneBody {
    /// Partial close volume; absent or null closes the whole position.
    #[serde(default)]
    volume: Option<f64>,
}

/// A ticket from the URL; a bad one gets the standard error shape.
fn parse_ticket(raw: &str) -> Result<u64, ApiError> {
    raw.parse::<u64>()
        .map_err(|_| ApiError::BadRequest(format!("'{raw}' is not a valid ticket number")))
}

async fn close_position(
    State(state): State<AppState>,
    Path(ticket): Path<String>,
    body: Bytes,
) -> Result<Json<PreviewDto>, ApiError> {
    let ticket = parse_ticket(&ticket)?;
    let body: CloseOneBody = optional_body(&body)?;
    propose_from_ui(
        &state,
        ProposedAction::Close(CloseSelector {
            tickets: vec![ticket],
            volume: body.volume,
            ..CloseSelector::default()
        }),
    )
    .await
}

#[derive(Default, Deserialize)]
struct CloseManyBody {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<Side>,
    #[serde(default)]
    all: bool,
}

async fn close_positions(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<PreviewDto>, ApiError> {
    let body: CloseManyBody = optional_body(&body)?;
    let symbol =
        match body
            .symbol
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            None => None,
            Some(s) => Some(state.resolver().resolve(s).ok_or_else(|| {
                ApiError::BadRequest(format!("'{s}' is not a usable symbol name"))
            })?),
        };
    propose_from_ui(
        &state,
        ProposedAction::Close(CloseSelector {
            tickets: Vec::new(),
            symbol,
            side: body.side,
            all: body.all,
            volume: None,
        }),
    )
    .await
}

async fn cancel_order(
    State(state): State<AppState>,
    Path(ticket): Path<String>,
) -> Result<Json<PreviewDto>, ApiError> {
    let ticket = parse_ticket(&ticket)?;
    propose_from_ui(
        &state,
        ProposedAction::Cancel(CancelSelector {
            tickets: vec![ticket],
            ..CancelSelector::default()
        }),
    )
    .await
}

// ---------------------------------------------------------------------------
// Kill switch
// ---------------------------------------------------------------------------

async fn killswitch_get(State(state): State<AppState>) -> Json<KillSwitchState> {
    Json(state.trading().kill_switch_state())
}

#[derive(Deserialize)]
struct KillSwitchBody {
    engaged: bool,
    #[serde(default)]
    reason: Option<String>,
    /// Required to release when auth is enabled (checked by the state's `ReleaseGuard`).
    #[serde(default)]
    totp: Option<String>,
}

async fn killswitch_set(
    State(state): State<AppState>,
    MaybeSession(session): MaybeSession,
    body: Bytes,
) -> Result<Json<KillSwitchState>, ApiError> {
    let req: KillSwitchBody = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))?;
    let new_state = if req.engaged {
        let reason = req
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .unwrap_or("manual");
        let reason: String = reason.chars().take(200).collect();
        state.trading().engage_kill_switch(&reason).await
    } else {
        state
            .release_guard()
            .authorize_release(session.as_ref(), req.totp.as_deref())
            .await
            .map_err(ApiError::Unauthorized)?;
        state.trading().release_kill_switch().await
    };
    Ok(Json(new_state))
}

// ---------------------------------------------------------------------------
// GET /api/audit
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AuditQuery {
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    before_id: Option<i64>,
}

const AUDIT_DEFAULT_LIMIT: u32 = 100;
const AUDIT_MAX_LIMIT: u32 = 500;

async fn audit_log(
    State(state): State<AppState>,
    Query(q): Query<AuditQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = q
        .limit
        .unwrap_or(AUDIT_DEFAULT_LIMIT)
        .clamp(1, AUDIT_MAX_LIMIT);
    let rows = state
        .audit()
        .recent(limit, q.before_id)
        .await
        .map_err(|e| {
            tracing::error!("reading the audit log failed: {e:#}");
            ApiError::Internal
        })?;
    let entries: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.id,
                "ts": r.ts,
                "session_id": r.session_id,
                "kind": r.kind,
                "payload": r.payload,
            })
        })
        .collect();
    Ok(Json(json!({ "entries": entries })))
}

#[cfg(test)]
mod tests {
    use super::redact_path;

    #[test]
    fn confirmation_tokens_are_redacted_from_logged_paths() {
        assert_eq!(
            redact_path("/api/confirm/abcdef"),
            "/api/confirm/<redacted>"
        );
        assert_eq!(redact_path("/api/cancel/abcdef"), "/api/cancel/<redacted>");
        assert_eq!(
            redact_path("/api/readback/abcdef"),
            "/api/readback/<redacted>"
        );
        assert_eq!(redact_path("/api/status"), "/api/status");
        assert_eq!(
            redact_path("/api/positions/7/close"),
            "/api/positions/7/close"
        );
    }
}
