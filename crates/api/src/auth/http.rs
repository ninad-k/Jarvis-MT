//! Login routes, the session middleware and the cookie.

use std::net::SocketAddr;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use super::{SESSION_COOKIE, Session};
use crate::routes::ApiError;
use crate::state::AppState;

/// The caller's session, when the request carries one. Present on protected routes whenever
/// login is enabled; always `None` when it is disabled.
#[derive(Debug, Clone)]
pub struct MaybeSession(pub Option<Session>);

impl<S: Send + Sync> FromRequestParts<S> for MaybeSession {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(parts.extensions.get::<Session>().cloned()))
    }
}

/// Peer IP as text; requests without a peer (in-process tests) share one key.
pub fn peer_ip(parts: &Parts) -> String {
    parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map_or_else(|| "local".to_owned(), |ci| ci.0.ip().to_string())
}

/// The value of the session cookie in a `Cookie` header, if any.
pub fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.trim())
}

/// Resolve the session of a request: `Ok(None)` when login is disabled, `Err` (401) when it
/// is enabled and there is no valid session. Enabled with no service (a build error) fails
/// closed.
pub async fn authenticate_headers(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<Session>, ApiError> {
    if !state.config().auth.enabled {
        return Ok(None);
    }
    let unauthorized = || ApiError::Unauthorized("login required".to_owned());
    let auth = state.auth().ok_or_else(|| {
        tracing::error!("auth.enabled is true but no auth service is installed; refusing");
        unauthorized()
    })?;
    let token = session_cookie(headers).ok_or_else(unauthorized)?;
    auth.authenticate(token)
        .await
        .map(Some)
        .ok_or_else(unauthorized)
}

/// Middleware of every protected route.
pub async fn require_session(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    match authenticate_headers(&state, req.headers()).await {
        Ok(Some(session)) => {
            req.extensions_mut().insert(session);
            next.run(req).await
        }
        Ok(None) => next.run(req).await,
        Err(e) => e.into_response(),
    }
}

// ---------------------------------------------------------------------------
// Cookie
// ---------------------------------------------------------------------------

/// `Set-Cookie` for a new session: HttpOnly, SameSite=Strict, `Secure` unless this is plain
/// HTTP on loopback.
pub fn session_set_cookie(token: &str, max_age_secs: i64, secure: bool) -> HeaderValue {
    cookie_value(token, max_age_secs, secure)
}

/// `Set-Cookie` that deletes the session cookie.
pub fn session_clear_cookie(secure: bool) -> HeaderValue {
    cookie_value("", 0, secure)
}

fn cookie_value(value: &str, max_age_secs: i64, secure: bool) -> HeaderValue {
    let mut cookie = format!(
        "{SESSION_COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age_secs}"
    );
    if secure {
        cookie.push_str("; Secure");
    }
    // A JWT is base64url and dots: always a valid header value.
    HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static(""))
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
    totp: String,
}

/// `POST /api/auth/login`
pub async fn login(
    State(state): State<AppState>,
    parts: Parts,
    body: Bytes,
) -> Result<Response, ApiError> {
    let auth = state
        .auth()
        .ok_or_else(|| ApiError::BadRequest("login is disabled on this server".to_owned()))?;
    let ip = peer_ip(&parts);
    let req: LoginBody = serde_json::from_slice(&body).map_err(|_| {
        ApiError::BadRequest("body must be {\"username\", \"password\", \"totp\"}".to_owned())
    })?;
    let outcome = auth
        .login(req.username.trim(), &req.password, req.totp.trim(), &ip)
        .await?;
    let mut response = Json(json!({
        "username": outcome.session.username,
        "expires_at": outcome.session.expires_at_rfc3339(),
    }))
    .into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        session_set_cookie(
            &outcome.token,
            auth.session_ttl_secs(),
            auth.cookie_secure(),
        ),
    );
    Ok(response)
}

/// `POST /api/auth/logout`
pub async fn logout(
    State(state): State<AppState>,
    MaybeSession(session): MaybeSession,
) -> Response {
    let mut secure = state.config().cookie_secure();
    if let (Some(auth), Some(session)) = (state.auth(), session.as_ref()) {
        auth.logout(session).await;
        secure = auth.cookie_secure();
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, session_clear_cookie(secure));
    response
}

/// `GET /api/auth/me`
///
/// 401 without a session (from the middleware). With login disabled there is never a
/// session: `{"username": null, "expires_at": null}`.
pub async fn me(MaybeSession(session): MaybeSession) -> Json<serde_json::Value> {
    Json(match session {
        Some(session) => json!({
            "username": session.username,
            "expires_at": session.expires_at_rfc3339(),
        }),
        None => json!({ "username": null, "expires_at": null }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_session_cookie_among_others() {
        let mut h = HeaderMap::new();
        h.append(
            header::COOKIE,
            HeaderValue::from_static("a=1; jarvis_session=tok.en.x; b=2"),
        );
        assert_eq!(session_cookie(&h), Some("tok.en.x"));
        let mut h = HeaderMap::new();
        h.append(header::COOKIE, HeaderValue::from_static("a=1"));
        h.append(
            header::COOKIE,
            HeaderValue::from_static("jarvis_session=zzz"),
        );
        assert_eq!(session_cookie(&h), Some("zzz"));
        let mut h = HeaderMap::new();
        h.append(
            header::COOKIE,
            HeaderValue::from_static("xjarvis_session=bad"),
        );
        assert_eq!(session_cookie(&h), None);
        assert_eq!(session_cookie(&HeaderMap::new()), None);
    }

    #[test]
    fn cookie_attributes() {
        let secure = session_set_cookie("t", 3600, true);
        let s = secure.to_str().unwrap();
        for part in [
            "jarvis_session=t",
            "Path=/",
            "HttpOnly",
            "SameSite=Strict",
            "Max-Age=3600",
            "Secure",
        ] {
            assert!(s.contains(part), "{s} lacks {part}");
        }
        let plain = session_set_cookie("t", 3600, false);
        assert!(!plain.to_str().unwrap().contains("Secure"));
        let clear = session_clear_cookie(true);
        assert!(clear.to_str().unwrap().contains("Max-Age=0"));
        assert!(clear.to_str().unwrap().starts_with("jarvis_session=;"));
    }
}
