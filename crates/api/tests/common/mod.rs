//! Shared test helpers: a scripted fake LLM, state builders and an HTTP helper.
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use jarvis_api::AppState;
use jarvis_api::audit::Audit;
use jarvis_api::auth::crypto::{PasswordHasherCfg, build_totp};
use jarvis_api::auth::{AuthParts, AuthService};
use jarvis_api::clock::{ManualClock, TimeSource};
use jarvis_api::config::Config;
use jarvis_api::state::{Overrides, ProviderSlot};
use jarvis_api::trading::WriteSink;
use jarvis_api::voice::Voice;
use jarvis_bridge_client::{BridgeClient, BridgeConfig};
use jarvis_llm::{ChatRequest, ChatResponse, LlmError, LlmProvider, StopReason, ToolCall};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

/// A fake LLM that replays scripted responses and records every request it receives.
pub struct ScriptedLlm {
    name: String,
    script: Mutex<VecDeque<Result<ChatResponse, LlmError>>>,
    /// Returned once the script is exhausted (when set); otherwise an error.
    repeat: Option<ChatResponse>,
    pub requests: Mutex<Vec<ChatRequest>>,
}

impl ScriptedLlm {
    pub fn new(name: &str, script: Vec<Result<ChatResponse, LlmError>>) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_owned(),
            script: Mutex::new(script.into()),
            repeat: None,
            requests: Mutex::new(Vec::new()),
        })
    }

    /// Always answers with `response`.
    pub fn always(name: &str, response: ChatResponse) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_owned(),
            script: Mutex::new(VecDeque::new()),
            repeat: Some(response),
            requests: Mutex::new(Vec::new()),
        })
    }

    pub fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

#[async_trait]
impl LlmProvider for ScriptedLlm {
    fn name(&self) -> &str {
        &self.name
    }
    fn model(&self) -> &str {
        "scripted-1"
    }
    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        self.requests.lock().unwrap().push(request.clone());
        if let Some(next) = self.script.lock().unwrap().pop_front() {
            return next;
        }
        match &self.repeat {
            Some(r) => Ok(r.clone()),
            None => Err(LlmError::Decode("script exhausted".into())),
        }
    }
}

pub fn text(text: &str) -> Result<ChatResponse, LlmError> {
    Ok(text_response(text))
}

pub fn text_response(text: &str) -> ChatResponse {
    ChatResponse {
        text: Some(text.to_owned()),
        tool_calls: vec![],
        stop_reason: StopReason::EndTurn,
        usage: None,
    }
}

pub fn tool_use_response(id: &str, name: &str, arguments: Value) -> ChatResponse {
    ChatResponse {
        text: None,
        tool_calls: vec![ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments,
        }],
        stop_reason: StopReason::ToolUse,
        usage: None,
    }
}

pub fn tool_use(id: &str, name: &str, arguments: Value) -> Result<ChatResponse, LlmError> {
    Ok(tool_use_response(id, name, arguments))
}

/// Default `[limits]` body for tests: every mock symbol except BTCUSD.
pub const DEFAULT_LIMITS: &str = r#"
max_lot = 1.0
max_positions = 5
max_daily_loss = 500.0
allowed_symbols = ["XAUUSD", "US100", "US30", "EURUSD", "GBPUSD"]
"#;

/// TOML for a test config. `limits`/`trading` are the bodies of the `[limits]` and
/// `[trading]` tables (empty `limits` omits the section).
#[allow(clippy::too_many_arguments)]
pub fn config_toml(
    mode: &str,
    endpoint: &str,
    suffix: &str,
    watchlist: &[&str],
    limits: &str,
    trading: &str,
    server: &str,
    auth: bool,
) -> String {
    let watch = watchlist
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let limits_section = if limits.trim().is_empty() {
        String::new()
    } else {
        format!(
            "[limits]
{limits}
"
        )
    };
    format!(
        r#"
mode = "{mode}"

[server]
bind = "127.0.0.1:0"
{server}

[bridge]
endpoint = "{endpoint}"
timeout_ms = 300

[symbols]
suffix = "{suffix}"
watchlist = [{watch}]

[symbols.aliases]
gold = "XAUUSD"
cold = "XAUUSD"
nasdaq = "US100"
"dow jones" = "US30"

[llm]
default = "fake"

[[llm.providers]]
name = "fake"
kind = "ollama"
model = "fake-model"

[[llm.providers]]
name = "claude"
kind = "anthropic"
model = "claude-test"
api_key_env = "JARVIS_TEST_KEY_THAT_IS_NOT_SET"

{limits_section}

[trading]
{trading}

[auth]
enabled = {auth}
session_hours = 12

[voice]
enabled = false
"#
    )
}

pub fn temp_db_url() -> String {
    let path = std::env::temp_dir()
        .join(format!("jarvis-test-{}", Uuid::new_v4()))
        .join("audit.db");
    format!("sqlite://{}", path.to_string_lossy().replace('\\', "/"))
}

/// The secret the test auth service signs and encrypts with.
pub const JWT_SECRET: &str = "test-secret-test-secret-test-secret-1234";

pub struct TestApp {
    pub state: AppState,
    /// Present when the app was built with `Options::auth`.
    pub auth: Option<Arc<AuthService>>,
    pub router: Router,
    pub audit: Audit,
    pub llm: Arc<ScriptedLlm>,
    /// Present when the app was built with `Options::clock`.
    pub clock: Option<ManualClock>,
}

pub struct Options<'a> {
    pub mode: &'a str,
    pub suffix: &'a str,
    pub watchlist: &'a [&'a str],
    /// Body of the `[limits]` table; empty omits the section.
    pub limits: &'a str,
    /// Body of the `[trading]` table.
    pub trading: &'a str,
    /// Time source for token windows and rate limits.
    pub clock: Option<ManualClock>,
    /// Replaces the bridge as the destination of confirmed writes.
    pub sink: Option<Arc<dyn WriteSink>>,
    /// Turn login on (with a manual clock and cheap password hashing).
    pub auth: bool,
    /// Extra lines for the `[server]` table.
    pub server: &'a str,
    /// Speech engines.
    pub voice: Option<Voice>,
    /// Use this audit database instead of a fresh one (a "restart" over the same file).
    pub audit: Option<Audit>,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self {
            mode: "demo",
            suffix: "",
            watchlist: &["XAUUSD", "US100", "EURUSD"],
            limits: DEFAULT_LIMITS,
            trading: "",
            clock: None,
            sink: None,
            auth: false,
            server: "",
            voice: None,
            audit: None,
        }
    }
}

/// App wired to the bridge at `endpoint` and the given fake LLM (registered as `fake`;
/// the `claude` provider is left unavailable).
pub async fn build_app(endpoint: &str, llm: Arc<ScriptedLlm>, opts: Options<'_>) -> TestApp {
    let config = Config::from_toml_str(&config_toml(
        opts.mode,
        endpoint,
        opts.suffix,
        opts.watchlist,
        opts.limits,
        opts.trading,
        opts.server,
        opts.auth,
    ))
    .expect("test config");
    let bridge = BridgeClient::new(BridgeConfig {
        endpoint: endpoint.to_owned(),
        timeout: Duration::from_millis(config.bridge.timeout_ms),
    })
    .expect("bridge client");
    let audit = match opts.audit {
        Some(audit) => audit,
        None => Audit::connect(&temp_db_url()).await.expect("audit db"),
    };
    let mut slots = HashMap::new();
    slots.insert("fake".to_owned(), ProviderSlot::Available(llm.clone()));
    slots.insert(
        "claude".to_owned(),
        ProviderSlot::Unavailable("ANTHROPIC key missing".into()),
    );
    // Login needs a clock the tests can move (TOTP steps, lockouts, expiry).
    let clock = match (opts.clock, opts.auth) {
        (None, true) => Some(ManualClock::starting_now()),
        (clock, _) => clock,
    };
    let auth = if opts.auth {
        Some(
            AuthService::new(AuthParts {
                secret: JWT_SECRET.to_owned(),
                session_hours: 12,
                cookie_secure: config.cookie_secure(),
                time: Arc::new(clock.clone().expect("clock")) as Arc<dyn TimeSource>,
                audit: audit.clone(),
                hasher: PasswordHasherCfg::cheap_for_tests(),
            })
            .await
            .expect("auth service"),
        )
    } else {
        None
    };
    let overrides = Overrides {
        time: clock.clone().map(|c| Arc::new(c) as _),
        write_sink: opts.sink,
        auth: auth.clone(),
        voice: opts.voice,
    };
    let state = AppState::new_with(config, bridge, audit.clone(), slots, overrides);
    TestApp {
        router: jarvis_api::app(state.clone()),
        state,
        auth,
        audit,
        llm,
        clock,
    }
}

/// An endpoint nobody listens on.
pub const DEAD_ENDPOINT: &str = "tcp://127.0.0.1:1";

pub async fn request(
    router: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, _, bytes) =
        raw_request(router, method, uri, &[], body.map(|b| b.to_string())).await;
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

pub async fn raw_request(
    router: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<String>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let req = builder.body(Body::from(body.unwrap_or_default())).unwrap();
    let response = router.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, headers, bytes)
}

pub async fn chat(app: &TestApp, message: &str) -> (StatusCode, Value) {
    request(
        &app.router,
        "POST",
        "/api/chat",
        Some(json!({ "message": message })),
    )
    .await
}

pub async fn audit_kinds(app: &TestApp) -> Vec<String> {
    app.audit
        .rows(None)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.kind)
        .collect()
}

// ---------------------------------------------------------------------------
// Login helpers
// ---------------------------------------------------------------------------

/// A user created through the real account code, with the authenticator it was enrolled on.
pub struct TestUser {
    pub name: String,
    pub password: String,
    totp: totp_rs::Totp,
}

impl TestUser {
    /// The code the authenticator shows at `now_ms`.
    pub fn code_at(&self, now_ms: i64) -> String {
        self.totp
            .generate(u64::try_from(now_ms / 1000).unwrap())
            .to_string()
    }
}

pub async fn create_user(app: &TestApp, name: &str, password: &str) -> TestUser {
    let auth = app.auth.as_ref().expect("app built with Options::auth");
    let enrollment = auth
        .accounts()
        .add_user(name, password)
        .await
        .expect("add user");
    let secret = totp_rs::Secret::try_from_base32(&enrollment.secret_base32).expect("base32");
    TestUser {
        name: name.to_owned(),
        password: password.to_owned(),
        totp: build_totp(secret.as_bytes().to_vec(), name).expect("totp"),
    }
}

pub const PASSWORD: &str = "correct horse battery";

impl TestApp {
    pub fn clock(&self) -> &ManualClock {
        self.clock.as_ref().expect("manual clock")
    }

    /// A code for the *next* TOTP step: moves the clock on 30 s first, so a code is never
    /// presented twice and the rate-limit window also moves.
    pub fn fresh_code(&self, user: &TestUser) -> String {
        self.clock().advance_ms(30_000);
        user.code_at(self.clock().now_ms())
    }
}

/// `POST /api/auth/login`; the headers carry `Set-Cookie`.
pub async fn login_raw(
    app: &TestApp,
    username: &str,
    password: &str,
    totp: &str,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let body = json!({ "username": username, "password": password, "totp": totp });
    let (status, headers, bytes) = raw_request(
        &app.router,
        "POST",
        "/api/auth/login",
        &[],
        Some(body.to_string()),
    )
    .await;
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Log `user` in with a fresh code and return the `Cookie` request header value.
pub async fn login(app: &TestApp, user: &TestUser) -> String {
    let code = app.fresh_code(user);
    let (status, headers, body) = login_raw(app, &user.name, &user.password, &code).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    cookie_from(&headers).expect("Set-Cookie")
}

/// `jarvis_session=<token>` from a response's `Set-Cookie`.
pub fn cookie_from(headers: &axum::http::HeaderMap) -> Option<String> {
    let set = headers.get("set-cookie")?.to_str().ok()?;
    Some(set.split(';').next()?.to_owned())
}

/// A request that carries a session cookie.
pub async fn request_as(
    app: &TestApp,
    cookie: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, _, bytes) = raw_request(
        &app.router,
        method,
        uri,
        &[("cookie", cookie)],
        body.map(|b| b.to_string()),
    )
    .await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
