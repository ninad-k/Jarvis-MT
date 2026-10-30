//! Live smoke tests against real providers. All `#[ignore]`d:
//!
//! ```text
//! cargo test -p jarvis-llm --test live -- --ignored ollama --nocapture
//! ANTHROPIC_API_KEY=... cargo test -p jarvis-llm --test live -- --ignored anthropic --nocapture
//! OPENAI_API_KEY=...    cargo test -p jarvis-llm --test live -- --ignored openai --nocapture
//! ```
//!
//! Optional overrides: `JARVIS_OLLAMA_MODEL` (default `qwen3.5:9b`), `JARVIS_ANTHROPIC_MODEL`
//! (default `claude-sonnet-5-5`), `JARVIS_OPENAI_MODEL` (default `gpt-5`).

mod common;

use std::time::Instant;

use jarvis_llm::{
    ChatRequest, LlmProvider, Message, ProviderConfig, ProviderKind, StopReason, ToolSpec,
    build_provider,
};
use serde_json::json;

fn env_or(var: &str, default: &str) -> String {
    std::env::var(var)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn key_present(var: &str) -> bool {
    std::env::var(var)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

fn tool() -> ToolSpec {
    ToolSpec {
        name: "get_price".into(),
        description: "Get the current bid/ask price of a trading symbol.".into(),
        parameters: json!({
            "type": "object",
            "properties": {"symbol": {"type": "string", "description": "Broker symbol, e.g. EURUSD"}},
            "required": ["symbol"],
        }),
    }
}

/// Ask for gold's price, expect a `get_price(XAUUSD)` call, return a result, expect text
/// that mentions the price.
async fn gold_price_flow(p: &dyn LlmProvider) {
    let mut req = ChatRequest {
        system: Some(
            "You are Jarvis, a concise trading assistant. Use the tools to get market data; never guess prices."
                .into(),
        ),
        messages: vec![Message::User {
            text: "what's the price of gold? Gold's symbol is XAUUSD".into(),
        }],
        tools: vec![tool()],
        max_tokens: Some(512),
        temperature: None,
    };

    let t0 = Instant::now();
    let first = p.chat(&req).await.expect("first turn failed");
    let d1 = t0.elapsed();
    println!(
        "[{} / {}] turn 1: {:?} stop={:?} text={:?} calls={:?} usage={:?}",
        p.name(),
        p.model(),
        d1,
        first.stop_reason,
        first.text,
        first.tool_calls,
        first.usage
    );
    assert_eq!(first.stop_reason, StopReason::ToolUse);
    assert_eq!(first.tool_calls.len(), 1, "expected exactly one tool call");
    let call = &first.tool_calls[0];
    assert_eq!(call.name, "get_price");
    assert_eq!(
        call.arguments["symbol"]
            .as_str()
            .map(str::to_uppercase)
            .as_deref(),
        Some("XAUUSD")
    );

    req.messages.push(Message::Assistant {
        text: first.text.clone(),
        tool_calls: first.tool_calls.clone(),
    });
    req.messages.push(Message::ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: json!({"symbol": "XAUUSD", "bid": 2412.35, "ask": 2412.65}).to_string(),
        is_error: false,
    });

    let t1 = Instant::now();
    let second = p.chat(&req).await.expect("second turn failed");
    let d2 = t1.elapsed();
    println!(
        "[{} / {}] turn 2: {:?} stop={:?} text={:?} usage={:?}",
        p.name(),
        p.model(),
        d2,
        second.stop_reason,
        second.text,
        second.usage
    );
    println!(
        "[{} / {}] total latency: {:?}",
        p.name(),
        p.model(),
        d1 + d2
    );
    assert!(second.tool_calls.is_empty());
    assert_eq!(second.stop_reason, StopReason::EndTurn);
    let text = second.text.expect("expected a text answer");
    assert!(
        text.contains("2412") || text.contains("2,412"),
        "answer should mention the price, got: {text}"
    );
}

#[tokio::test]
#[ignore = "needs a local Ollama at 127.0.0.1:11434"]
async fn ollama_gold_price_tool_roundtrip() {
    let cfg = ProviderConfig {
        name: "local".into(),
        kind: ProviderKind::Ollama,
        model: env_or("JARVIS_OLLAMA_MODEL", "qwen3.5:9b"),
        base_url: Some(env_or("JARVIS_OLLAMA_URL", "http://127.0.0.1:11434")),
        api_key_env: None,
        timeout_secs: 180,
        max_tokens: 512,
    };
    let p = build_provider(&cfg).unwrap();
    gold_price_flow(p.as_ref()).await;
}

#[tokio::test]
#[ignore = "needs ANTHROPIC_API_KEY"]
async fn anthropic_gold_price_tool_roundtrip() {
    if !key_present("ANTHROPIC_API_KEY") {
        println!("ANTHROPIC_API_KEY not set; skipping");
        return;
    }
    let cfg = ProviderConfig {
        name: "claude".into(),
        kind: ProviderKind::Anthropic,
        model: env_or("JARVIS_ANTHROPIC_MODEL", "claude-sonnet-5-5"),
        base_url: None,
        api_key_env: Some("ANTHROPIC_API_KEY".into()),
        timeout_secs: 60,
        max_tokens: 1024,
    };
    // The environment may point ANTHROPIC_BASE_URL at a proxy; honour it only if set explicitly.
    let cfg = ProviderConfig {
        base_url: std::env::var("JARVIS_ANTHROPIC_URL")
            .ok()
            .filter(|s| !s.is_empty()),
        ..cfg
    };
    let p = build_provider(&cfg).unwrap();
    gold_price_flow(p.as_ref()).await;
}

#[tokio::test]
#[ignore = "needs OPENAI_API_KEY"]
async fn openai_gold_price_tool_roundtrip() {
    if !key_present("OPENAI_API_KEY") {
        println!("OPENAI_API_KEY not set; skipping");
        return;
    }
    let cfg = ProviderConfig {
        name: "openai".into(),
        kind: ProviderKind::Openai,
        model: env_or("JARVIS_OPENAI_MODEL", "gpt-5"),
        base_url: std::env::var("JARVIS_OPENAI_URL")
            .ok()
            .filter(|s| !s.is_empty()),
        api_key_env: Some("OPENAI_API_KEY".into()),
        timeout_secs: 90,
        max_tokens: 2048,
    };
    let p = build_provider(&cfg).unwrap();
    gold_price_flow(p.as_ref()).await;
}
