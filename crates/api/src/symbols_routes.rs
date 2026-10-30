//! `/api/symbols*` (docs/api.md, "Broker symbol mapping").
//!
//! Reading is open to every logged-in user. Changing a mapping needs a fresh authenticator
//! code when login is on: a wrong mapping could send an order to the wrong instrument.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use crate::auth::http::MaybeSession;
use crate::broker::{BrokerError, slug};
use crate::routes::ApiError;
use crate::state::AppState;

impl From<BrokerError> for ApiError {
    fn from(e: BrokerError) -> Self {
        match e {
            BrokerError::Bridge(b) => b.into(),
            BrokerError::NoBroker => ApiError::BridgeUnavailable(e.to_string()),
            BrokerError::NoCatalog => ApiError::SymbolsUnavailable(e.to_string()),
            BrokerError::ChangedDuringDetection | BrokerError::AccountChanged => {
                ApiError::BrokerChanged
            }
            BrokerError::UnknownBrokerSymbol(m) => ApiError::UnknownBrokerSymbol(m),
            BrokerError::UnknownInstrument(m) => ApiError::BadRequest(m),
            BrokerError::Save(m) => {
                tracing::error!("saving the symbol map failed: {m}");
                ApiError::Internal
            }
        }
    }
}

// ---------------------------------------------------------------------------
// GET /api/symbols
// ---------------------------------------------------------------------------

pub async fn symbols_get(State(state): State<AppState>) -> Json<Value> {
    Json(state.broker().book().view())
}

// ---------------------------------------------------------------------------
// PUT /api/symbols/map
// ---------------------------------------------------------------------------

/// Tells "field absent" from "field is null".
fn present<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MapBody {
    canonical: String,
    /// A broker symbol sets the override; `null` removes it.
    #[serde(default, deserialize_with = "present")]
    broker_symbol: Option<Option<String>>,
    /// A fresh authenticator code; required when login is on.
    #[serde(default)]
    totp: Option<String>,
}

pub async fn map_put(
    State(state): State<AppState>,
    MaybeSession(session): MaybeSession,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let req: MapBody = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("invalid body: {e}")))?;
    let Some(broker_symbol) = req.broker_symbol else {
        return Err(ApiError::BadRequest(
            "broker_symbol is required: a symbol of the broker, or null to remove the override"
                .to_owned(),
        ));
    };

    // Refused before the code is spent: unknown instrument or a symbol the broker lacks.
    let prepared = state
        .broker()
        .prepare_override(&req.canonical, broker_symbol.as_deref())?;

    state
        .release_guard()
        .authorize_release(session.as_ref(), req.totp.as_deref())
        .await
        .map_err(ApiError::Unauthorized)?;

    let by = AppState::settings_key(session.as_ref());
    state.broker().apply_override(prepared, by).await?;
    Ok(Json(state.broker().book().view()))
}

// ---------------------------------------------------------------------------
// POST /api/symbols/detect
// ---------------------------------------------------------------------------

pub async fn detect_post(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    state.broker().detect().await?;
    Ok(Json(state.broker().book().view()))
}

// ---------------------------------------------------------------------------
// GET /api/symbols/catalog
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CatalogQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

const CATALOG_DEFAULT_LIMIT: usize = 50;
const CATALOG_MAX_LIMIT: usize = 500;

pub async fn catalog_get(
    State(state): State<AppState>,
    Query(query): Query<CatalogQuery>,
) -> Json<Value> {
    let limit = query
        .limit
        .unwrap_or(CATALOG_DEFAULT_LIMIT)
        .clamp(1, CATALOG_MAX_LIMIT);
    let Some(snapshot) = state.broker().book().snapshot() else {
        return Json(json!({ "symbols": [], "total": 0 }));
    };
    let (found, total) = snapshot.search(query.q.as_deref().unwrap_or(""), limit);
    let symbols: Vec<Value> = found
        .into_iter()
        .map(|s| {
            json!({
                "name": s.name,
                "description": s.description,
                "path": s.path,
                "trade_mode": s.trade_mode,
                "visible": s.visible,
            })
        })
        .collect();
    Json(json!({ "symbols": symbols, "total": total }))
}

// ---------------------------------------------------------------------------
// GET /api/symbols/export
// ---------------------------------------------------------------------------

pub async fn export_get(State(state): State<AppState>) -> Result<Response, ApiError> {
    let Some(export) = state.broker().book().export_view() else {
        return Err(ApiError::BridgeUnavailable(
            "no broker is known yet; is MetaTrader 5 running with the EA?".to_owned(),
        ));
    };
    let company = export["broker"].as_str().unwrap_or("broker");
    let filename = format!("arc-mt-symbols-{}.json", slug(company));
    let text = serde_json::to_string_pretty(&export).map_err(|e| {
        tracing::error!("encoding the symbol export failed: {e}");
        ApiError::Internal
    })?;
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
        .map_err(|_| ApiError::Internal)?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            ),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        text,
    )
        .into_response())
}
