//! HTTP routes. The contract is `docs/api.md`.

use std::time::Instant;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use jarvis_bridge_client::BridgeError;
use jarvis_bridge_client::protocol::{Account, Positions, Price};
use jarvis_llm::LlmError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::task::JoinSet;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use crate::agent::ToolCallSummary;
use crate::state::{AppState, ProviderSlot};

/// Longest accepted chat message, in characters.
pub const MAX_MESSAGE_CHARS: usize = 2000;
/// Origins allowed to call the API from a browser (the Vite dev server).
pub const DEV_ORIGINS: [&str; 2] = ["http://localhost:5173", "http://127.0.0.1:5173"];

/// The whole API as a router. Tests call this with a hand-built [`AppState`].
pub fn app(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(DEV_ORIGINS.map(HeaderValue::from_static).to_vec())
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE]);
    Router::new()
        .route("/api/status", get(status))
        .route("/api/account", get(account))
        .route("/api/positions", get(positions))
        .route("/api/watchlist", get(watchlist))
        .route("/api/chat", post(chat))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
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

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = self.parts();
        (
            status,
            Json(json!({ "error": { "code": code, "message": message } })),
        )
            .into_response()
    }
}

// ---------------------------------------------------------------------------
// GET /api/status
// ---------------------------------------------------------------------------

async fn status(State(state): State<AppState>) -> Json<Value> {
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
}

#[derive(Serialize)]
struct ChatResponseBody {
    session_id: Uuid,
    reply: String,
    provider: String,
    tool_calls: Vec<ToolCallSummary>,
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
        .run_turn(provider.as_ref(), session_id, message)
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
    }))
}
