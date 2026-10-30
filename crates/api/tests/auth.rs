//! Login, sessions, the middleware, kill-switch release and security headers.

mod common;

use axum::http::StatusCode;
use base64::Engine;
use common::*;
use jarvis_api::auth::crypto::PasswordHasherCfg;
use jarvis_api::auth::{AuthParts, AuthService, SESSION_COOKIE};
use jarvis_api::clock::TimeSource;
use jarvis_bridge_client::mock::MockBridge;
use serde_json::{Value, json};
use std::sync::Arc;

fn idle_llm() -> Arc<ScriptedLlm> {
    ScriptedLlm::new("fake", vec![])
}

async fn auth_app() -> (TestApp, TestUser, MockBridge) {
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock,
        idle_llm(),
        Options {
            auth: true,
            ..Options::default()
        },
    )
    .await;
    let user = create_user(&app, "ninad", PASSWORD).await;
    (app, user, mock)
}

const GENERIC: &str = "invalid username, password or code";

// ------------------------------------------------------------------ login

#[tokio::test]
async fn login_sets_a_hardened_cookie_and_opens_the_api() {
    let (app, user, _mock) = auth_app().await;
    let code = app.fresh_code(&user);
    let (status, headers, body) = login_raw(&app, "ninad", PASSWORD, &code).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["username"], "ninad");
    let expires = body["expires_at"].as_str().unwrap();
    assert!(expires.ends_with('Z'), "RFC 3339 UTC: {expires}");

    let set_cookie = headers.get("set-cookie").unwrap().to_str().unwrap();
    for part in [
        "jarvis_session=",
        "Path=/",
        "HttpOnly",
        "SameSite=Strict",
        "Max-Age=43200",
    ] {
        assert!(set_cookie.contains(part), "{set_cookie} lacks {part}");
    }
    // The test app serves plain HTTP on loopback: no Secure attribute.
    assert!(!set_cookie.contains("Secure"), "{set_cookie}");

    let cookie = cookie_from(&headers).unwrap();
    let (status, me) = request_as(&app, &cookie, "GET", "/api/auth/me", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["username"], "ninad");
    let (status, st) = request_as(&app, &cookie, "GET", "/api/status", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(st["auth"], json!({ "enabled": true, "username": "ninad" }));
}

#[tokio::test]
async fn every_login_failure_looks_the_same() {
    let (app, user, _mock) = auth_app().await;
    let good = user.code_at(app.clock().now_ms());
    let cases = [
        ("unknown user", "ghost", PASSWORD, good.clone()),
        ("wrong password", "ninad", "not the password!", good.clone()),
        ("wrong code", "ninad", PASSWORD, "000001".to_owned()),
        ("non-numeric code", "ninad", PASSWORD, "abcdef".to_owned()),
        ("empty everything", "", "", String::new()),
    ];
    let mut seen = Vec::new();
    for (label, name, password, code) in cases {
        let (status, headers, body) = login_raw(&app, name, password, &code).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{label}");
        assert!(headers.get("set-cookie").is_none(), "{label}");
        assert_eq!(body["error"]["code"], "unauthorized", "{label}");
        assert_eq!(body["error"]["message"], GENERIC, "{label}");
        seen.push(body);
    }
    assert!(seen.windows(2).all(|w| w[0] == w[1]));
    // Malformed JSON is a 400, never a hint about accounts.
    let (status, _, _) = raw_request(
        &app.router,
        "POST",
        "/api/auth/login",
        &[],
        Some("{".into()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // The reasons are in the audit log, the secrets are not.
    let rows = app.audit.rows(None).await.unwrap();
    let reasons: Vec<String> = rows
        .iter()
        .filter(|r| r.kind == "login_failed")
        .map(|r| r.payload["reason"].as_str().unwrap().to_owned())
        .collect();
    assert!(reasons.contains(&"unknown_user".to_owned()), "{reasons:?}");
    assert!(reasons.contains(&"bad_password".to_owned()), "{reasons:?}");
    assert!(reasons.contains(&"bad_code".to_owned()), "{reasons:?}");
    let dump =
        serde_json::to_string(&rows.iter().map(|r| r.payload.clone()).collect::<Vec<_>>()).unwrap();
    for secret in [PASSWORD, "not the password!", good.as_str(), "000001"] {
        assert!(!dump.contains(secret), "audit must not contain {secret}");
    }
}

#[tokio::test]
async fn a_typed_name_that_belongs_to_nobody_is_not_written_to_the_audit_log() {
    // People paste passwords into the user name field; the audit log is readable by every session.
    let (app, _user, _mock) = auth_app().await;
    let typed = "my-real-password-by-mistake-77";
    let (status, _, _) = login_raw(&app, typed, "whatever it was", "123456").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let too_long = "x".repeat(200);
    let (status, _, _) = login_raw(&app, &too_long, "pw", "123456").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let rows = app.audit.rows(None).await.unwrap();
    let dump =
        serde_json::to_string(&rows.iter().map(|r| r.payload.clone()).collect::<Vec<_>>()).unwrap();
    assert!(!dump.contains(typed), "{dump}");
    assert!(!dump.contains(&too_long), "{dump}");
    let reasons: Vec<&str> = rows
        .iter()
        .filter(|r| r.kind == "login_failed")
        .map(|r| r.payload["reason"].as_str().unwrap())
        .collect();
    assert_eq!(reasons, ["unknown_user", "malformed"]);
    // A real account's failures still name the account.
    let (status, _, _) = login_raw(&app, "NINAD", "wrong password!!", "123456").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let rows = app.audit.rows(None).await.unwrap();
    let last = rows.last().unwrap();
    assert_eq!(last.payload["reason"], "bad_password");
    assert_eq!(last.payload["username"], "ninad", "the stored spelling");
}

#[tokio::test]
async fn a_flood_of_refused_logins_writes_one_audit_row_per_minute() {
    // The audit table is append-only: an unauthenticated flood must not be able to fill the disk.
    let (app, _user, _mock) = auth_app().await;
    for _ in 0..200 {
        let (status, _, _) = login_raw(&app, "ninad", "wrong password!!", "000001").await;
        assert!(
            status == StatusCode::UNAUTHORIZED || status == StatusCode::TOO_MANY_REQUESTS,
            "{status}"
        );
    }
    let count = |rows: &[jarvis_api::audit::AuditRow], reason: &str| {
        rows.iter()
            .filter(|r| r.kind == "login_failed" && r.payload["reason"] == reason)
            .count()
    };
    let rows = app.audit.rows(None).await.unwrap();
    assert_eq!(
        count(&rows, "rate_limited"),
        1,
        "one row for the whole flood"
    );
    assert!(
        rows.len() <= 8,
        "{} audit rows for 200 requests",
        rows.len()
    );
    // A minute later the next refusal is recorded again.
    app.clock().advance_ms(61_000);
    for _ in 0..6 {
        login_raw(&app, "ninad", "wrong password!!", "000001").await;
    }
    let rows = app.audit.rows(None).await.unwrap();
    assert_eq!(count(&rows, "rate_limited"), 2);
}

#[tokio::test]
async fn login_is_rate_limited_per_ip() {
    let (app, user, _mock) = auth_app().await;
    for _ in 0..5 {
        let (status, _, _) = login_raw(&app, "ninad", "wrong password!!", "000001").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    // The sixth attempt in a minute is refused before any credential is looked at, even
    // with the right credentials.
    let code = user.code_at(app.clock().now_ms());
    let (status, headers, body) = login_raw(&app, "ninad", PASSWORD, &code).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "rate_limited");
    assert!(headers.get("retry-after").is_some());
    // A minute later it works again.
    app.clock().advance_ms(61_000);
    let code = user.code_at(app.clock().now_ms());
    let (status, _, _) = login_raw(&app, "ninad", PASSWORD, &code).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn ten_failures_lock_the_account_for_fifteen_minutes() {
    let (app, user, _mock) = auth_app().await;
    for batch in 0..2 {
        for _ in 0..5 {
            let (status, _, _) = login_raw(&app, "ninad", PASSWORD, "abcdef").await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        if batch == 0 {
            app.clock().advance_ms(61_000);
        }
    }
    // Locked: the right password and a fresh code are refused with the generic answer.
    app.clock().advance_ms(61_000);
    let code = app.fresh_code(&user);
    let (status, _, body) = login_raw(&app, "ninad", PASSWORD, &code).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["message"], GENERIC);
    let rows = app.audit.rows(None).await.unwrap();
    assert!(
        rows.iter()
            .any(|r| r.kind == "login_failed" && r.payload["locked_now"] == true),
        "the tenth failure is audited as locking the account"
    );
    assert!(
        rows.iter()
            .any(|r| r.kind == "login_failed" && r.payload["reason"] == "locked")
    );
    // Still locked 14 minutes later, open after 15.
    app.clock().advance_ms(12 * 60_000);
    let code = app.fresh_code(&user);
    let (status, _, _) = login_raw(&app, "ninad", PASSWORD, &code).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    app.clock().advance_ms(2 * 60_000);
    let code = app.fresh_code(&user);
    let (status, _, body) = login_raw(&app, "ninad", PASSWORD, &code).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_totp_code_works_once() {
    let (app, user, _mock) = auth_app().await;
    let code = app.fresh_code(&user);
    let (status, _, _) = login_raw(&app, "ninad", PASSWORD, &code).await;
    assert_eq!(status, StatusCode::OK);
    // Same code, same step: replay.
    let (status, _, body) = login_raw(&app, "ninad", PASSWORD, &code).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["message"], GENERIC);
    let rows = app.audit.rows(None).await.unwrap();
    assert!(
        rows.iter()
            .any(|r| r.kind == "login_failed" && r.payload["reason"] == "code_reused")
    );
    // The next step's code is fine.
    let (status, _, _) = login_raw(&app, "ninad", PASSWORD, &app.fresh_code(&user)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn totp_allows_one_step_of_clock_skew_and_no_more() {
    let (app, user, _mock) = auth_app().await;
    let now = app.clock().now_ms();
    // 90 s old and 90 s in the future: refused.
    for offset in [-90_000, 90_000] {
        let (status, _, _) = login_raw(&app, "ninad", PASSWORD, &user.code_at(now + offset)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "offset {offset}");
    }
    // One step old: accepted.
    let (status, _, _) = login_raw(&app, "ninad", PASSWORD, &user.code_at(now - 30_000)).await;
    assert_eq!(status, StatusCode::OK);
}

// ------------------------------------------------------------------- JWT

fn forge(secret: &str, alg: jsonwebtoken::Algorithm, claims: &Value) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(alg),
        claims,
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap()
}

fn claims(sub: &str, now_secs: i64, ttl: i64) -> Value {
    json!({ "sub": sub, "iat": now_secs, "exp": now_secs + ttl, "jti": "forged-1" })
}

async fn status_with_token(app: &TestApp, token: &str) -> StatusCode {
    let cookie = format!("{SESSION_COOKIE}={token}");
    request_as(app, &cookie, "GET", "/api/auth/me", None)
        .await
        .0
}

#[tokio::test]
async fn forged_expired_and_wrongly_signed_tokens_are_rejected() {
    let (app, user, _mock) = auth_app().await;
    let now = app.clock().now_ms() / 1000;
    use jsonwebtoken::Algorithm::{HS256, HS384};

    // Control: a correctly signed token for a real user is accepted (so the rejections
    // below are about the token, not the setup).
    let good = forge(JWT_SECRET, HS256, &claims("ninad", now, 3600));
    assert_eq!(status_with_token(&app, &good).await, StatusCode::OK);

    // Expired by claim.
    let expired = forge(JWT_SECRET, HS256, &claims("ninad", now - 7200, 3600));
    assert_eq!(
        status_with_token(&app, &expired).await,
        StatusCode::UNAUTHORIZED
    );
    // Wrong key, wrong algorithm, no signature.
    let other_key = forge(
        "another-secret-another-secret-123456",
        HS256,
        &claims("ninad", now, 3600),
    );
    assert_eq!(
        status_with_token(&app, &other_key).await,
        StatusCode::UNAUTHORIZED
    );
    let hs384 = forge(JWT_SECRET, HS384, &claims("ninad", now, 3600));
    assert_eq!(
        status_with_token(&app, &hs384).await,
        StatusCode::UNAUTHORIZED
    );
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let none = format!(
        "{}.{}.",
        b64.encode(r#"{"alg":"none","typ":"JWT"}"#),
        b64.encode(claims("ninad", now, 3600).to_string())
    );
    assert_eq!(
        status_with_token(&app, &none).await,
        StatusCode::UNAUTHORIZED
    );
    // A signed token for a user that does not exist.
    let ghost = forge(JWT_SECRET, HS256, &claims("ghost", now, 3600));
    assert_eq!(
        status_with_token(&app, &ghost).await,
        StatusCode::UNAUTHORIZED
    );
    // A token with no expiry.
    let no_exp = forge(
        JWT_SECRET,
        HS256,
        &json!({ "sub": "ninad", "iat": now, "jti": "x" }),
    );
    assert_eq!(
        status_with_token(&app, &no_exp).await,
        StatusCode::UNAUTHORIZED
    );
    // Tampered payload (signature no longer matches).
    let mut parts: Vec<String> = good.split('.').map(str::to_owned).collect();
    parts[1] = b64.encode(claims("ninad", now, 999_999).to_string());
    assert_eq!(
        status_with_token(&app, &parts.join(".")).await,
        StatusCode::UNAUTHORIZED
    );
    // Tampered signature.
    let mut sig = good.clone();
    sig.pop();
    sig.push(if good.ends_with('A') { 'B' } else { 'A' });
    assert_eq!(
        status_with_token(&app, &sig).await,
        StatusCode::UNAUTHORIZED
    );
    // Junk.
    for junk in ["", "abc", "a.b.c", &"x".repeat(5000)] {
        assert_eq!(
            status_with_token(&app, junk).await,
            StatusCode::UNAUTHORIZED
        );
    }

    // A real session expires with the clock.
    let cookie = login(&app, &user).await;
    let (status, _) = request_as(&app, &cookie, "GET", "/api/auth/me", None).await;
    assert_eq!(status, StatusCode::OK);
    app.clock().advance_ms(12 * 3600 * 1000 + 1000);
    let (status, body) = request_as(&app, &cookie, "GET", "/api/auth/me", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn logout_ends_the_session_for_good() {
    let (app, user, _mock) = auth_app().await;
    let cookie = login(&app, &user).await;
    let (status, headers, _) = raw_request(
        &app.router,
        "POST",
        "/api/auth/logout",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let cleared = headers.get("set-cookie").unwrap().to_str().unwrap();
    assert!(cleared.contains("Max-Age=0") && cleared.starts_with("jarvis_session=;"));

    let (status, _) = request_as(&app, &cookie, "GET", "/api/status", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the old cookie is dead");

    // A restarted server (a new service on the same database) still refuses it.
    let token = cookie.strip_prefix("jarvis_session=").unwrap();
    let restarted = AuthService::new(AuthParts {
        secret: JWT_SECRET.to_owned(),
        session_hours: 12,
        cookie_secure: false,
        time: Arc::new(app.clock().clone()) as Arc<dyn TimeSource>,
        audit: app.audit.clone(),
        hasher: PasswordHasherCfg::cheap_for_tests(),
    })
    .await
    .unwrap();
    assert!(restarted.authenticate(token).await.is_none());
    // While a different session of the same user is unaffected.
    let other = login(&app, &user).await;
    let (status, _) = request_as(&app, &other, "GET", "/api/status", None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn resetting_or_removing_a_user_ends_their_sessions() {
    let (app, user, _mock) = auth_app().await;
    let cookie = login(&app, &user).await;
    app.clock().advance_ms(2000);
    app.auth
        .as_ref()
        .unwrap()
        .accounts()
        .reset_totp("ninad")
        .await
        .unwrap();
    let (status, _) = request_as(&app, &cookie, "GET", "/api/auth/me", None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "old sessions die on reset"
    );
    // The old authenticator no longer works either.
    let (status, _, _) = login_raw(&app, "ninad", PASSWORD, &app.fresh_code(&user)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let user2 = create_user(&app, "second", PASSWORD).await;
    let cookie2 = login(&app, &user2).await;
    assert!(
        app.auth
            .as_ref()
            .unwrap()
            .accounts()
            .remove_user("second")
            .await
            .unwrap()
    );
    let (status, _) = request_as(&app, &cookie2, "GET", "/api/auth/me", None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "removed users lose their sessions"
    );
}

#[tokio::test]
async fn weak_or_missing_secrets_are_refused_at_startup() {
    let mock = MockBridge::start().await;
    let app = build_app(&mock, idle_llm(), Options::default()).await;
    let refused = |secret: String| {
        let audit = app.audit.clone();
        async move {
            AuthService::new(AuthParts {
                secret,
                session_hours: 12,
                cookie_secure: false,
                time: Arc::new(jarvis_api::clock::WallClock),
                audit,
                hasher: PasswordHasherCfg::cheap_for_tests(),
            })
            .await
            .err()
            .expect("must refuse")
            .to_string()
        }
    };
    // Too short, and then long enough but guessable (AUTH-2): one byte repeated, a handful of
    // different characters, a block repeated, base64 that decodes to nothing random.
    for secret in [
        String::new(),
        "short".to_owned(),
        "s".repeat(31),
        "a".repeat(32),
        "a".repeat(64),
        "test-secret-test-secret-test-secret-1234".to_owned(),
        "abcdefghijklmnop".repeat(2),
        "A".repeat(48),
    ] {
        let err = refused(secret.clone()).await;
        assert!(err.contains("JARVIS_JWT_SECRET"), "{secret:?}: {err}");
        assert!(err.contains("RandomNumberGenerator"), "{secret:?}: {err}");
    }
    // The generated kind is accepted.
    assert!(
        AuthService::new(AuthParts {
            secret: JWT_SECRET.to_owned(),
            session_hours: 12,
            cookie_secure: false,
            time: Arc::new(jarvis_api::clock::WallClock),
            audit: app.audit.clone(),
            hasher: PasswordHasherCfg::cheap_for_tests(),
        })
        .await
        .is_ok()
    );
}

#[tokio::test]
async fn duplicate_names_and_bad_input_are_refused() {
    let (app, _user, _mock) = auth_app().await;
    let accounts = app.auth.as_ref().unwrap().accounts();
    let err = accounts.add_user("NINAD", PASSWORD).await.unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
    assert!(accounts.add_user("new user", PASSWORD).await.is_err());
    assert!(accounts.add_user("fine", "short").await.is_err());
    let list = accounts.list_users().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].username, "ninad");
}

#[tokio::test]
async fn the_database_never_holds_the_plain_totp_secret_or_password() {
    let (app, _user, _mock) = auth_app().await;
    let enrollment = app
        .auth
        .as_ref()
        .unwrap()
        .accounts()
        .add_user("second", PASSWORD)
        .await
        .unwrap();
    let pool = app.audit.pool().clone();
    let rows = sqlx_rows(&pool).await;
    assert_eq!(rows.len(), 2);
    for (hash, sealed) in rows {
        assert!(hash.starts_with("$argon2id$"), "{hash}");
        assert!(sealed.starts_with("v1:"), "{sealed}");
        assert!(!sealed.contains(&enrollment.secret_base32));
        assert!(!hash.contains(PASSWORD));
    }
}

async fn sqlx_rows(pool: &sqlx::SqlitePool) -> Vec<(String, String)> {
    use sqlx::Row;
    sqlx::query("SELECT password_hash, totp_secret FROM users ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect()
}

// ------------------------------------------------------------- middleware

/// Every protected route, one way or another. A new route added to the protected router is
/// covered by the unknown-path case below even if it is missing here.
const PROTECTED: &[(&str, &str)] = &[
    ("GET", "/api/status"),
    ("GET", "/api/account"),
    ("GET", "/api/positions"),
    ("GET", "/api/orders"),
    ("GET", "/api/watchlist"),
    ("POST", "/api/chat"),
    ("POST", "/api/confirm/abc"),
    ("POST", "/api/cancel/abc"),
    ("POST", "/api/readback/abc"),
    ("POST", "/api/positions/1/close"),
    ("POST", "/api/positions/close"),
    ("POST", "/api/orders/1/cancel"),
    ("GET", "/api/killswitch"),
    ("POST", "/api/killswitch"),
    ("GET", "/api/audit"),
    ("POST", "/api/auth/logout"),
    ("GET", "/api/auth/me"),
    ("GET", "/api/ws"),
    ("POST", "/api/voice/transcribe"),
    ("POST", "/api/voice/speak"),
    ("GET", "/api/anything/else"),
    ("POST", "/api"),
];

#[tokio::test]
async fn the_middleware_protects_every_route() {
    let (app, user, _mock) = auth_app().await;
    for (method, path) in PROTECTED {
        let (status, _, bytes) =
            raw_request(&app.router, method, path, &[], Some("{}".into())).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "unauthorized", "{method} {path}");
        // A garbage cookie is no better than none.
        let (status, _, _) = raw_request(
            &app.router,
            method,
            path,
            &[("cookie", "jarvis_session=nonsense")],
            Some("{}".into()),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} with a bad cookie"
        );
    }
    // With a session the same unknown path is a JSON 404, not a 401.
    let cookie = login(&app, &user).await;
    let (status, body) = request_as(&app, &cookie, "GET", "/api/anything/else", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn health_is_open_and_exactly_ok_true() {
    let (app, _user, _mock) = auth_app().await;
    let (status, headers, bytes) = raw_request(&app.router, "GET", "/api/health", &[], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, br#"{"ok":true}"#);
    assert!(
        headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    // Login is reachable without a session too (400 for an empty body, not 401).
    let (status, _, _) = raw_request(
        &app.router,
        "POST",
        "/api/auth/login",
        &[],
        Some("{}".into()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn with_login_disabled_nothing_needs_a_session() {
    let mock = MockBridge::start().await;
    let app = build_app(&mock, idle_llm(), Options::default()).await;
    let (status, body) = request(&app.router, "GET", "/api/status", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["auth"], json!({ "enabled": false, "username": null }));
    let (status, body) = request(&app.router, "GET", "/api/auth/me", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "username": null, "expires_at": null }));
    let (status, _) = request(
        &app.router,
        "POST",
        "/api/auth/login",
        Some(json!({"username":"a","password":"b","totp":"c"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "no login on a server without it"
    );
}

#[tokio::test]
async fn enabled_login_without_a_service_fails_closed() {
    // Auth on in the config, but no service installed (a wiring mistake): refuse everything.
    let mock = MockBridge::start().await;
    let app = build_app(&mock, idle_llm(), Options::default()).await;
    let mut config = app.state.config().clone();
    config.auth.enabled = true;
    let state = jarvis_api::AppState::new(
        config,
        bridge_client(&mock, 300),
        app.audit.clone(),
        Default::default(),
    );
    let router = jarvis_api::app(state);
    for path in ["/api/status", "/api/killswitch", "/api/nothing"] {
        let (status, _, _) = raw_request(&router, "GET", path, &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
    let (status, _, _) = raw_request(&router, "GET", "/api/health", &[], None).await;
    assert_eq!(status, StatusCode::OK);
}

// --------------------------------------------------- confirm rate limit

#[tokio::test]
async fn confirm_is_limited_per_session_not_per_ip() {
    let (app, user, _mock) = auth_app().await;
    let first = login(&app, &user).await;
    let second = login(&app, &user).await;
    for i in 0..10 {
        let (status, body) = request_as(
            &app,
            &first,
            "POST",
            &format!("/api/confirm/bogus{i}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    }
    let (status, body) = request_as(&app, &first, "POST", "/api/confirm/bogus", None).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "rate_limited");
    // Another session of the same user (same peer address) has its own allowance.
    let (status, _) = request_as(&app, &second, "POST", "/api/confirm/bogus", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------ kill switch

#[tokio::test]
async fn releasing_the_kill_switch_needs_a_fresh_totp_code() {
    let (app, user, _mock) = auth_app().await;
    let cookie = login(&app, &user).await;
    // Engaging is open to any session.
    let (status, body) = request_as(
        &app,
        &cookie,
        "POST",
        "/api/killswitch",
        Some(json!({ "engaged": true, "reason": "test" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["engaged"], true);

    let release = |code: Option<String>| {
        let cookie = cookie.clone();
        let app = &app;
        async move {
            let body = match code {
                Some(c) => json!({ "engaged": false, "totp": c }),
                None => json!({ "engaged": false }),
            };
            request_as(app, &cookie, "POST", "/api/killswitch", Some(body)).await
        }
    };
    for attempt in [None, Some("000000".to_owned())] {
        let (status, body) = release(attempt).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(body["error"]["code"], "unauthorized");
        assert_eq!(body["error"]["message"], "code not accepted");
    }
    let (_, ks) = request_as(&app, &cookie, "GET", "/api/killswitch", None).await;
    assert_eq!(ks["engaged"], true, "still engaged");

    // The code that opened this session is spent; the next one releases.
    let used = user.code_at(app.clock().now_ms());
    let (status, _) = release(Some(used)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a code never works twice");
    let good = app.fresh_code(&user);
    let (status, body) = release(Some(good.clone())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["engaged"], false);

    // Engage again and replay the code that just worked: refused as a reuse.
    request_as(
        &app,
        &cookie,
        "POST",
        "/api/killswitch",
        Some(json!({ "engaged": true })),
    )
    .await;
    let (status, body) = release(Some(good)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let rows = app.audit.rows(None).await.unwrap();
    assert!(
        rows.iter()
            .any(|r| r.kind == "kill_switch" && r.payload["release_refused"] == "code_reused"),
        "the replay is audited as a reuse"
    );
}

#[tokio::test]
async fn an_expired_session_on_release_says_login_required() {
    let (app, user, _mock) = auth_app().await;
    let cookie = login(&app, &user).await;
    app.clock().advance_ms(13 * 3600 * 1000);
    let code = app.fresh_code(&user);
    let (status, body) = request_as(
        &app,
        &cookie,
        "POST",
        "/api/killswitch",
        Some(json!({ "engaged": false, "totp": code })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["message"], "login required");
}

#[tokio::test]
async fn release_attempts_are_rate_limited_and_audited() {
    let (app, user, _mock) = auth_app().await;
    let cookie = login(&app, &user).await;
    for _ in 0..8 {
        let (status, body) = request_as(
            &app,
            &cookie,
            "POST",
            "/api/killswitch",
            Some(json!({ "engaged": false, "totp": "000000" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["message"], "code not accepted");
    }
    let rows = app.audit.rows(None).await.unwrap();
    let refused = rows
        .iter()
        .filter(|r| r.kind == "kill_switch" && r.payload.get("release_refused").is_some())
        .count();
    assert_eq!(
        refused, 5,
        "the sixth and later attempts never reach the code check"
    );
}

// ------------------------------------------------------ security headers

#[tokio::test]
async fn security_headers_are_on_every_response() {
    let (app, user, _mock) = auth_app().await;
    let cookie = login(&app, &user).await;
    let probes = [
        ("GET", "/api/health", vec![]),
        ("GET", "/api/status", vec![("cookie", cookie.as_str())]),
        ("GET", "/api/status", vec![]), // a 401
        ("GET", "/no/such/page", vec![]),
    ];
    for (method, path, headers) in probes {
        let (_, h, _) = raw_request(&app.router, method, path, &headers, None).await;
        let get = |name: &str| h.get(name).map(|v| v.to_str().unwrap().to_owned());
        let csp = get("content-security-policy").unwrap_or_else(|| panic!("CSP on {path}"));
        for part in [
            "default-src 'self'",
            "script-src 'self'",
            "frame-ancestors 'none'",
            "base-uri 'self'",
            "form-action 'self'",
            "media-src 'self' blob:",
            "worker-src 'self'",
        ] {
            assert!(csp.contains(part), "{path}: {csp}");
        }
        assert!(!csp.contains("unsafe-eval"));
        assert_eq!(
            get("x-content-type-options").as_deref(),
            Some("nosniff"),
            "{path}"
        );
        assert_eq!(
            get("referrer-policy").as_deref(),
            Some("no-referrer"),
            "{path}"
        );
        assert_eq!(
            get("permissions-policy").as_deref(),
            Some("microphone=(self), camera=(), geolocation=()"),
            "{path}"
        );
        assert_eq!(
            get("cross-origin-opener-policy").as_deref(),
            Some("same-origin"),
            "{path}"
        );
        assert_eq!(get("x-frame-options").as_deref(), Some("DENY"), "{path}");
        // No TLS in this app: no HSTS.
        assert!(get("strict-transport-security").is_none(), "{path}");
    }
    let (_, h, _) = raw_request(&app.router, "GET", "/api/health", &[], None).await;
    assert_eq!(h.get("cache-control").unwrap(), "no-store");
}

#[tokio::test]
async fn request_bodies_are_capped() {
    let (app, user, _mock) = auth_app().await;
    let cookie = login(&app, &user).await;
    // JSON routes: 64 KB.
    let big = json!({ "message": "x".repeat(70 * 1024) }).to_string();
    let (status, _, bytes) = raw_request(
        &app.router,
        "POST",
        "/api/chat",
        &[("cookie", &cookie)],
        Some(big),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "payload_too_large");
    // The login route too (it is public, so an anonymous client can reach it).
    let big =
        json!({ "username": "x".repeat(70 * 1024), "password": "p", "totp": "1" }).to_string();
    let (status, _, _) = raw_request(&app.router, "POST", "/api/auth/login", &[], Some(big)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    // Just under the cap is read (and fails later, on its content).
    let ok = json!({ "message": "x".repeat(10 * 1024) }).to_string();
    let (status, _, _) = raw_request(
        &app.router,
        "POST",
        "/api/chat",
        &[("cookie", &cookie)],
        Some(ok),
    )
    .await;
    assert_ne!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn wrong_methods_get_the_standard_error_shape() {
    let (app, user, _mock) = auth_app().await;
    let cookie = login(&app, &user).await;
    let (status, body) = request_as(&app, &cookie, "DELETE", "/api/status", None).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(body["error"]["code"], "method_not_allowed");
}

// ------------------------------------------- lockout per client address (AUTH-1)

/// Log in through the service itself, so a test can say which address the attempt came from
/// (the in-process HTTP tests have none).
async fn login_from(
    app: &TestApp,
    user: &TestUser,
    password: &str,
    code: &str,
    ip: &str,
) -> Result<(), jarvis_api::routes::ApiError> {
    app.auth
        .as_ref()
        .unwrap()
        .login(&user.name, password, code, ip)
        .await
        .map(|_| ())
}

fn is_generic_401(e: &jarvis_api::routes::ApiError) -> bool {
    matches!(e, jarvis_api::routes::ApiError::Unauthorized(m) if m == GENERIC)
}

/// Ten bad attempts from one address, spread over enough minutes to stay under the per-address
/// rate limit.
async fn hammer_from(app: &TestApp, user: &TestUser, ip: &str, attempts: usize) {
    for i in 0..attempts {
        let err = login_from(app, user, "not the password!", "000000", ip)
            .await
            .expect_err("a wrong password never gets in");
        assert!(is_generic_401(&err), "{err:?}");
        if i % 4 == 3 {
            app.clock().advance_ms(61_000);
        }
    }
}

#[tokio::test]
async fn someone_who_knows_the_name_cannot_lock_the_owner_out() {
    let (app, user, _mock) = auth_app().await;
    let attacker = "203.0.113.66";
    let home = "198.51.100.10";
    hammer_from(&app, &user, attacker, 10).await;

    // The attacker's address is shut out, even with the right credentials ...
    app.clock().advance_ms(61_000);
    let code = app.fresh_code(&user);
    let err = login_from(&app, &user, PASSWORD, &code, attacker)
        .await
        .expect_err("locked");
    assert!(is_generic_401(&err));
    let rows = app.audit.rows(None).await.unwrap();
    assert!(
        rows.iter()
            .any(|r| r.payload["reason"] == "locked" && r.payload["ip"] == attacker),
        "the refusal is audited as a lock with the address"
    );
    assert!(
        rows.iter().any(|r| r.payload["locked_now"] == true),
        "and the failure that locked it says so"
    );

    // ... and the owner, from elsewhere, is not.
    let code = app.fresh_code(&user);
    login_from(&app, &user, PASSWORD, &code, home)
        .await
        .expect("the owner gets in from another address");

    // The lock is 15 minutes, as before.
    app.clock().advance_ms(10 * 60_000);
    let code = app.fresh_code(&user);
    login_from(&app, &user, PASSWORD, &code, attacker)
        .await
        .expect_err("still locked");
    app.clock().advance_ms(3 * 60_000);
    let code = app.fresh_code(&user);
    login_from(&app, &user, PASSWORD, &code, attacker)
        .await
        .expect("open again");
}

#[tokio::test]
async fn a_lock_does_not_leak_a_difference_in_what_the_client_sees() {
    // The locked address gets the same answer as any other failure.
    let (app, user, _mock) = auth_app().await;
    hammer_from(&app, &user, "203.0.113.66", 10).await;
    app.clock().advance_ms(61_000);
    let (status, _, body) = login_raw(&app, "ninad", "not the password!", "000000").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["message"], GENERIC);
    let code = app.fresh_code(&user);
    let err = login_from(&app, &user, PASSWORD, &code, "203.0.113.66")
        .await
        .expect_err("locked");
    assert!(is_generic_401(&err), "{err:?}");
}

#[tokio::test]
async fn failures_from_many_addresses_slow_the_account_down_but_never_lock_it() {
    let (app, user, _mock) = auth_app().await;
    let auth = app.auth.as_ref().unwrap();
    auth.set_slow_down_delay(std::time::Duration::from_millis(400));

    // Before the mark, an attempt is not delayed.
    let started = std::time::Instant::now();
    let code = app.fresh_code(&user);
    login_from(&app, &user, PASSWORD, &code, "198.51.100.1")
        .await
        .expect("a normal login");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(400),
        "no delay yet: {:?}",
        started.elapsed()
    );

    // 50 addresses, one bad attempt each: nobody is locked, so nobody is refused for it.
    for n in 0..50 {
        let ip = format!("203.0.113.{n}");
        login_from(&app, &user, "not the password!", "000000", &ip)
            .await
            .expect_err("wrong");
    }
    // Now every attempt on the account waits, the owner's too, and the owner still gets in.
    let code = app.fresh_code(&user);
    let started = std::time::Instant::now();
    login_from(&app, &user, PASSWORD, &code, "198.51.100.1")
        .await
        .expect("the owner is slowed down, not locked out");
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(400),
        "delayed: {:?}",
        started.elapsed()
    );

    // The pressure fades with time.
    app.clock().advance_ms(16 * 60_000);
    let code = app.fresh_code(&user);
    let started = std::time::Instant::now();
    login_from(&app, &user, PASSWORD, &code, "198.51.100.1")
        .await
        .expect("login");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(400),
        "no longer delayed: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_good_login_clears_the_failures_of_that_address() {
    let (app, user, _mock) = auth_app().await;
    let ip = "198.51.100.10";
    // Nine failures, then a success, then nine more: never ten in a row.
    hammer_from(&app, &user, ip, 9).await;
    app.clock().advance_ms(61_000);
    let code = app.fresh_code(&user);
    login_from(&app, &user, PASSWORD, &code, ip)
        .await
        .expect("login");
    app.clock().advance_ms(61_000);
    hammer_from(&app, &user, ip, 9).await;
    app.clock().advance_ms(61_000);
    let code = app.fresh_code(&user);
    login_from(&app, &user, PASSWORD, &code, ip)
        .await
        .expect("still not locked");
}
