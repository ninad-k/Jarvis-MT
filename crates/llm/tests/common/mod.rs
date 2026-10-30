#![allow(dead_code)]

use jarvis_llm::{ChatRequest, Message, ProviderConfig, ProviderKind, ToolCall, ToolSpec};
use serde_json::{Value, json};
use wiremock::MockServer;

pub fn config(kind: ProviderKind, base_url: &str, timeout_secs: u64) -> ProviderConfig {
    ProviderConfig {
        name: "test".into(),
        kind,
        model: "test-model".into(),
        base_url: Some(base_url.into()),
        api_key_env: None,
        timeout_secs,
        max_tokens: 777,
    }
}

pub fn get_price_tool() -> ToolSpec {
    ToolSpec {
        name: "get_price".into(),
        description: "Get the current price of a symbol".into(),
        parameters: json!({
            "type": "object",
            "properties": {"symbol": {"type": "string"}},
            "required": ["symbol"],
        }),
    }
}

pub fn call(id: &str, symbol: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "get_price".into(),
        arguments: json!({"symbol": symbol}),
    }
}

pub fn simple_request(text: &str) -> ChatRequest {
    ChatRequest {
        system: Some("You are Jarvis.".into()),
        messages: vec![Message::User { text: text.into() }],
        tools: vec![get_price_tool()],
        max_tokens: None,
        temperature: None,
    }
}

/// user -> assistant tool call -> tool result -> user follow-up.
pub fn multi_turn_request() -> ChatRequest {
    ChatRequest {
        system: Some("You are Jarvis.".into()),
        messages: vec![
            Message::User {
                text: "price of gold?".into(),
            },
            Message::Assistant {
                text: Some("Checking.".into()),
                tool_calls: vec![call("call_abc", "XAUUSD")],
            },
            Message::ToolResult {
                call_id: "call_abc".into(),
                name: "get_price".into(),
                content: "{\"bid\":2400.5}".into(),
                is_error: false,
            },
            Message::User {
                text: "and silver?".into(),
            },
        ],
        tools: vec![get_price_tool()],
        max_tokens: None,
        temperature: Some(0.2),
    }
}

/// Body of the first request the mock server received.
pub async fn first_body(server: &MockServer) -> Value {
    let reqs = server.received_requests().await.expect("recording enabled");
    assert_eq!(reqs.len(), 1, "expected exactly one request");
    reqs[0].body_json().expect("request body is JSON")
}

pub async fn first_header(server: &MockServer, name: &str) -> Option<String> {
    let reqs = server.received_requests().await.unwrap();
    reqs[0]
        .headers
        .get(name)
        .map(|v| v.to_str().unwrap().to_string())
}
