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
use jarvis_api::config::Config;
use jarvis_api::state::ProviderSlot;
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

/// TOML for a test config. `extra_top` goes before any table (top-level keys only).
pub fn config_toml(mode: &str, endpoint: &str, suffix: &str, watchlist: &[&str]) -> String {
    let watch = watchlist
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"
mode = "{mode}"

[server]
bind = "127.0.0.1:0"

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
"#
    )
}

pub fn temp_db_url() -> String {
    let path = std::env::temp_dir()
        .join(format!("jarvis-test-{}", Uuid::new_v4()))
        .join("audit.db");
    format!("sqlite://{}", path.to_string_lossy().replace('\\', "/"))
}

pub struct TestApp {
    pub state: AppState,
    pub router: Router,
    pub audit: Audit,
    pub llm: Arc<ScriptedLlm>,
}

pub struct Options<'a> {
    pub mode: &'a str,
    pub suffix: &'a str,
    pub watchlist: &'a [&'a str],
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self {
            mode: "demo",
            suffix: "",
            watchlist: &["XAUUSD", "US100", "EURUSD"],
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
    ))
    .expect("test config");
    let bridge = BridgeClient::new(BridgeConfig {
        endpoint: endpoint.to_owned(),
        timeout: Duration::from_millis(config.bridge.timeout_ms),
    })
    .expect("bridge client");
    let audit = Audit::connect(&temp_db_url()).await.expect("audit db");
    let mut slots = HashMap::new();
    slots.insert("fake".to_owned(), ProviderSlot::Available(llm.clone()));
    slots.insert(
        "claude".to_owned(),
        ProviderSlot::Unavailable("ANTHROPIC key missing".into()),
    );
    let state = AppState::new(config, bridge, audit.clone(), slots);
    TestApp {
        router: jarvis_api::app(state.clone()),
        state,
        audit,
        llm,
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
