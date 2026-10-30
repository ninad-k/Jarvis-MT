//! `/api/personas`, `/api/settings` and `/api/settings/limits` (docs/api.md, "Personas and
//! user settings").
//!
//! Settings are preferences. The limits routes can only make the configured trading limits
//! stricter, and only with a fresh authenticator code when login is on.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use jarvis_gate::Limits;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::audit::AuditKind;
use crate::auth::http::MaybeSession;
use crate::events::Event;
use crate::limit_overrides::{PartialLimits, Refusal, validate_stricter};
use crate::routes::ApiError;
use crate::settings::SettingsPatch;
use crate::state::{AppState, SettingsUpdateError};

// ---------------------------------------------------------------------------
// GET /api/personas
// ---------------------------------------------------------------------------

pub async fn personas(
    State(state): State<AppState>,
    MaybeSession(session): MaybeSession,
) -> Json<Value> {
    let settings = state.user_settings(session.as_ref()).await;
    let active = state.persona_for(&settings).id.clone();
    // The default persona first, the rest by id.
    let default_id = state.personas().default_id();
    let mut packs: Vec<_> = state.personas().list().iter().collect();
    packs.sort_by_key(|p| (p.id != default_id, p.id.as_str()));
    let list: Vec<Value> = packs
        .into_iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "description": p.description,
                "wake_word": p.wake_word,
                "voice_available": state.voice().persona_voice_available(&p.id),
                "theme": p.theme,
            })
        })
        .collect();
    Json(json!({
        "default": state.personas().default_id(),
        "active": active,
        "personas": list,
    }))
}

// ---------------------------------------------------------------------------
// GET / PUT /api/settings
// ---------------------------------------------------------------------------

pub async fn settings_get(
    State(state): State<AppState>,
    MaybeSession(session): MaybeSession,
) -> Json<crate::settings::UserSettings> {
    Json(state.user_settings(session.as_ref()).await)
}

pub async fn settings_put(
    State(state): State<AppState>,
    MaybeSession(session): MaybeSession,
    body: Bytes,
) -> Result<Json<crate::settings::UserSettings>, ApiError> {
    let patch: SettingsPatch = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("invalid settings: {e}")))?;
    let updated = state
        .update_settings(session.as_ref(), patch)
        .await
        .map_err(|e| match e {
            SettingsUpdateError::Invalid(e) => ApiError::BadRequest(e.0),
            SettingsUpdateError::Storage(e) => {
                tracing::error!("saving settings failed: {e:#}");
                ApiError::Internal
            }
        })?;
    state.events().emit(Event::SettingsChanged);
    Ok(Json(updated))
}

// ---------------------------------------------------------------------------
// GET / PUT / DELETE /api/settings/limits
// ---------------------------------------------------------------------------

fn limits_view(state: &AppState) -> Value {
    json!({
        "config": state.trading().config_limits(),
        "overrides": state.trading().limit_overrides(),
        "effective": state.trading().limits(),
    })
}

pub async fn limits_get(State(state): State<AppState>) -> Json<Value> {
    Json(limits_view(&state))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitsBody {
    /// A fresh authenticator code; required when login is on.
    #[serde(default)]
    totp: Option<String>,
    #[serde(default)]
    max_lot: Option<f64>,
    #[serde(default)]
    max_positions: Option<u32>,
    #[serde(default)]
    max_daily_loss: Option<f64>,
    #[serde(default)]
    allowed_symbols: Option<Vec<String>>,
    #[serde(default)]
    trading_hours: Option<jarvis_gate::TradingHours>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TotpBody {
    #[serde(default)]
    totp: Option<String>,
}

fn who(session: Option<&crate::auth::Session>) -> &str {
    AppState::settings_key(session)
}

pub async fn limits_put(
    State(state): State<AppState>,
    MaybeSession(session): MaybeSession,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let req: LimitsBody = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("invalid limits: {e}")))?;

    // Symbols are named the way the config names them: canonical.
    let allowed_symbols = match &req.allowed_symbols {
        None => None,
        Some(list) => {
            let mut out: Vec<String> = Vec::with_capacity(list.len());
            for name in list {
                let symbol = state.resolver().canonical(name).ok_or_else(|| {
                    ApiError::BadRequest(format!("'{name}' is not a usable symbol name"))
                })?;
                if !out.contains(&symbol) {
                    out.push(symbol);
                }
            }
            Some(out)
        }
    };
    let wanted = PartialLimits {
        max_lot: req.max_lot,
        max_positions: req.max_positions,
        max_daily_loss: req.max_daily_loss,
        allowed_symbols,
        trading_hours: req.trading_hours,
    };
    if wanted.is_empty() {
        return Err(ApiError::BadRequest(
            "give at least one limit to tighten (max_lot, max_positions, max_daily_loss, \
             allowed_symbols or trading_hours)"
                .to_owned(),
        ));
    }

    // Looser or malformed values are refused before the code is spent.
    match validate_stricter(state.trading().config_limits(), &wanted) {
        Ok(()) => {}
        Err(Refusal::Malformed(m)) => return Err(ApiError::BadRequest(m.0)),
        Err(Refusal::NotStricter(n)) => {
            tracing::warn!(
                field = n.field,
                "a limit override looser than the config was refused"
            );
            return Err(ApiError::LimitNotStricter(n.message));
        }
    }

    state
        .release_guard()
        .authorize_release(session.as_ref(), req.totp.as_deref())
        .await
        .map_err(ApiError::Unauthorized)?;

    let _write = state.settings_store().lock().await;
    let old_overrides = state.trading().limit_overrides();
    let old_effective = state.trading().limits();
    let merged = match &old_overrides {
        Some(old) => old.merged_with(&wanted),
        None => wanted.clone(),
    };
    apply(
        &state,
        session.as_ref(),
        "set",
        Some(merged),
        &wanted,
        old_overrides,
        old_effective,
    )
    .await?;
    Ok(Json(limits_view(&state)))
}

pub async fn limits_delete(
    State(state): State<AppState>,
    MaybeSession(session): MaybeSession,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let req: TotpBody = if body.iter().all(u8::is_ascii_whitespace) {
        TotpBody::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::BadRequest(format!("invalid body: {e}")))?
    };
    state
        .release_guard()
        .authorize_release(session.as_ref(), req.totp.as_deref())
        .await
        .map_err(ApiError::Unauthorized)?;

    let _write = state.settings_store().lock().await;
    let old_overrides = state.trading().limit_overrides();
    let old_effective = state.trading().limits();
    apply(
        &state,
        session.as_ref(),
        "reset",
        None,
        &PartialLimits::default(),
        old_overrides,
        old_effective,
    )
    .await?;
    Ok(Json(limits_view(&state)))
}

/// Store the new overrides, switch the gate over, audit it and tell the clients.
async fn apply(
    state: &AppState,
    session: Option<&crate::auth::Session>,
    action: &str,
    new_overrides: Option<PartialLimits>,
    requested: &PartialLimits,
    old_overrides: Option<PartialLimits>,
    old_effective: Limits,
) -> Result<(), ApiError> {
    let by = who(session).to_owned();
    let store = state.settings_store();
    let saved = match &new_overrides {
        Some(o) => store.save_limit_overrides(o, &by).await,
        None => store.clear_limit_overrides().await,
    };
    if let Err(e) = saved {
        tracing::error!("saving the limit overrides failed: {e:#}");
        return Err(ApiError::Internal);
    }
    let new_effective = state.trading().set_limit_overrides(new_overrides.clone());
    state
        .audit()
        .record(
            None,
            AuditKind::LimitsChanged,
            json!({
                "action": action,
                "by": by,
                "requested": requested,
                "old": { "overrides": old_overrides, "effective": old_effective },
                "new": { "overrides": new_overrides, "effective": new_effective },
            }),
        )
        .await;
    state.events().emit(Event::SettingsChanged);
    Ok(())
}
