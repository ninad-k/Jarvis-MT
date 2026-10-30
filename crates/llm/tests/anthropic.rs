mod common;

use common::*;
use jarvis_llm::anthropic::AnthropicProvider;
use jarvis_llm::{
    ChatRequest, LlmError, LlmProvider, Message, ProviderKind, StopReason, Usage, build_provider,
};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn server_with(resp: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(resp)
        .mount(&server)
        .await;
    server
}

fn provider(server: &MockServer, timeout: u64) -> AnthropicProvider {
    AnthropicProvider::new(
        &config(ProviderKind::Anthropic, &server.uri(), timeout),
        "sk-ant-test-key",
    )
    .unwrap()
}

#[tokio::test]
async fn request_shape_multi_turn() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-ant-test-key"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
        })))
        .expect(1)
        .mount(&server)
        .await;
    let p = provider(&server, 5);
    p.chat(&multi_turn_request()).await.unwrap();

    let body = first_body(&server).await;
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["max_tokens"], 777);
    assert_eq!(body["system"], "You are Jarvis.");
    // Current models reject sampling params: temperature must not be sent.
    assert!(body.get("temperature").is_none());
    assert!(body.get("tool_choice").is_none());
    assert!(body.get("thinking").is_none());

    let tool = &body["tools"][0];
    assert_eq!(tool["name"], "get_price");
    assert_eq!(tool["description"], "Get the current price of a symbol");
    assert_eq!(tool["input_schema"]["required"], json!(["symbol"]));

    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(
        msgs.len(),
        3,
        "tool result + follow-up share one user message"
    );
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(
        msgs[0]["content"][0],
        json!({"type": "text", "text": "price of gold?"})
    );
    assert_eq!(msgs[1]["role"], "assistant");
    assert_eq!(
        msgs[1]["content"][0],
        json!({"type": "text", "text": "Checking."})
    );
    assert_eq!(
        msgs[1]["content"][1],
        json!({"type": "tool_use", "id": "call_abc", "name": "get_price", "input": {"symbol": "XAUUSD"}})
    );
    assert_eq!(msgs[2]["role"], "user");
    // tool_result comes first, then the text.
    assert_eq!(
        msgs[2]["content"][0],
        json!({"type": "tool_result", "tool_use_id": "call_abc", "content": "{\"bid\":2400.5}"})
    );
    assert_eq!(
        msgs[2]["content"][1],
        json!({"type": "text", "text": "and silver?"})
    );
}

#[tokio::test]
async fn request_max_tokens_override_and_no_system_no_tools() {
    let server = server_with(ResponseTemplate::new(200).set_body_json(json!({
        "content": [{"type": "text", "text": "hi"}], "stop_reason": "end_turn"
    })))
    .await;
    let req = ChatRequest {
        system: None,
        messages: vec![Message::User {
            text: "hello".into(),
        }],
        tools: vec![],
        max_tokens: Some(50),
        temperature: None,
    };
    provider(&server, 5).chat(&req).await.unwrap();
    let body = first_body(&server).await;
    assert_eq!(body["max_tokens"], 50);
    assert!(body.get("system").is_none());
    assert!(body.get("tools").is_none());
}

#[tokio::test]
async fn parallel_tool_results_share_one_user_message_and_error_flag() {
    let server = server_with(ResponseTemplate::new(200).set_body_json(json!({
        "content": [{"type": "text", "text": "done"}], "stop_reason": "end_turn"
    })))
    .await;
    let req = ChatRequest {
        system: None,
        messages: vec![
            Message::User {
                text: "prices".into(),
            },
            Message::Assistant {
                text: None,
                tool_calls: vec![call("toolu_1", "XAUUSD"), call("toolu_2", "EURUSD")],
            },
            Message::ToolResult {
                call_id: "toolu_1".into(),
                name: "get_price".into(),
                content: "1".into(),
                is_error: false,
            },
            Message::ToolResult {
                call_id: "toolu_2".into(),
                name: "get_price".into(),
                content: "unknown symbol".into(),
                is_error: true,
            },
        ],
        tools: vec![get_price_tool()],
        max_tokens: None,
        temperature: None,
    };
    provider(&server, 5).chat(&req).await.unwrap();
    let body = first_body(&server).await;
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3);
    let results = msgs[2]["content"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["tool_use_id"], "toolu_1");
    assert!(results[0].get("is_error").is_none());
    assert_eq!(results[1]["tool_use_id"], "toolu_2");
    assert_eq!(results[1]["is_error"], true);
}

#[tokio::test]
async fn parses_text_only() {
    let server = server_with(ResponseTemplate::new(200).set_body_json(json!({
        "id": "msg_1", "type": "message", "role": "assistant", "model": "test-model",
        "content": [{"type": "text", "text": "Gold is "}, {"type": "text", "text": "up."}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 10, "output_tokens": 4}
    })))
    .await;
    let r = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap();
    assert_eq!(r.text.as_deref(), Some("Gold is up."));
    assert!(r.tool_calls.is_empty());
    assert_eq!(r.stop_reason, StopReason::EndTurn);
    assert_eq!(
        r.usage,
        Some(Usage {
            input_tokens: 10,
            output_tokens: 4
        })
    );
}

#[tokio::test]
async fn parses_tool_calls_with_text() {
    let server = server_with(ResponseTemplate::new(200).set_body_json(json!({
        "content": [
            {"type": "text", "text": "Let me check."},
            {"type": "tool_use", "id": "toolu_1", "name": "get_price", "input": {"symbol": "XAUUSD"}},
            {"type": "tool_use", "id": "toolu_2", "name": "get_price", "input": {}}
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 5, "cache_read_input_tokens": 100, "output_tokens": 9}
    })))
    .await;
    let r = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap();
    assert_eq!(r.text.as_deref(), Some("Let me check."));
    assert_eq!(r.stop_reason, StopReason::ToolUse);
    assert_eq!(r.tool_calls.len(), 2);
    assert_eq!(r.tool_calls[0], call("toolu_1", "XAUUSD"));
    assert_eq!(r.tool_calls[1].arguments, json!({}));
    // cache reads count toward total input tokens
    assert_eq!(
        r.usage,
        Some(Usage {
            input_tokens: 105,
            output_tokens: 9
        })
    );
}

#[tokio::test]
async fn parses_max_tokens_and_other_stop_reasons() {
    let server = server_with(ResponseTemplate::new(200).set_body_json(json!({
        "content": [{"type": "text", "text": "cut"}], "stop_reason": "max_tokens"
    })))
    .await;
    let r = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap();
    assert_eq!(r.stop_reason, StopReason::MaxTokens);
    assert_eq!(r.usage, None);

    let server = server_with(ResponseTemplate::new(200).set_body_json(json!({
        "content": [], "stop_reason": "refusal"
    })))
    .await;
    let r = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap();
    assert_eq!(r.stop_reason, StopReason::Other("refusal".into()));
    assert_eq!(r.text, None);
}

#[tokio::test]
async fn api_error_4xx_extracts_message() {
    let server = server_with(ResponseTemplate::new(400).set_body_json(json!({
        "type": "error",
        "error": {"type": "invalid_request_error", "message": "max_tokens: must be positive"}
    })))
    .await;
    let err = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap_err();
    match err {
        LlmError::Api { status, message } => {
            assert_eq!(status, 400);
            assert_eq!(message, "max_tokens: must be positive");
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[tokio::test]
async fn api_error_5xx_non_json_body() {
    let server =
        server_with(ResponseTemplate::new(529).set_body_string("overloaded upstream")).await;
    let err = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap_err();
    match err {
        LlmError::Api { status, message } => {
            assert_eq!(status, 529);
            assert_eq!(message, "overloaded upstream");
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[tokio::test]
async fn api_error_never_leaks_key() {
    let server = server_with(ResponseTemplate::new(401).set_body_json(json!({
        "type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key"}
    })))
    .await;
    let err = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap_err();
    assert!(!format!("{err} {err:?}").contains("sk-ant-test-key"));
}

#[tokio::test]
async fn malformed_bodies_are_decode_errors() {
    for resp in [
        ResponseTemplate::new(200).set_body_string("<html>not json</html>"),
        ResponseTemplate::new(200).set_body_json(json!({"unexpected": true})),
        ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "tool_use", "name": "get_price", "input": {}}],
            "stop_reason": "tool_use"
        })),
        ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "tool_use", "id": "x", "name": "get_price", "input": "[1,2]"}],
            "stop_reason": "tool_use"
        })),
    ] {
        let server = server_with(resp).await;
        let err = provider(&server, 5)
            .chat(&simple_request("hi"))
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Decode(_)), "got {err:?}");
    }
}

#[tokio::test]
async fn slow_response_times_out() {
    let server =
        server_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(4))).await;
    let err = provider(&server, 1)
        .chat(&simple_request("hi"))
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::Timeout), "got {err:?}");
}

#[tokio::test]
async fn connection_refused_is_transport_error() {
    let p = AnthropicProvider::new(
        &config(ProviderKind::Anthropic, "http://127.0.0.1:1", 5),
        "k",
    )
    .unwrap();
    let err = p.chat(&simple_request("hi")).await.unwrap_err();
    assert!(matches!(err, LlmError::Transport(_)), "got {err:?}");
}

#[tokio::test]
async fn trailing_slash_and_v1_suffix_in_base_url() {
    for suffix in ["/", "//", "/v1", "/v1/"] {
        let server = server_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn"
        })))
        .await;
        let base = format!("{}{suffix}", server.uri());
        let p = AnthropicProvider::new(&config(ProviderKind::Anthropic, &base, 5), "k").unwrap();
        let r = p.chat(&simple_request("hi")).await.unwrap();
        assert_eq!(r.text.as_deref(), Some("ok"), "base suffix {suffix:?}");
    }
}

#[tokio::test]
async fn thinking_blocks_are_replayed_with_the_tool_call() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [
                {"type": "thinking", "thinking": "", "signature": "sig123"},
                {"type": "tool_use", "id": "toolu_9", "name": "get_price", "input": {"symbol": "XAUUSD"}}
            ],
            "stop_reason": "tool_use"
        })))
        .mount(&server)
        .await;
    let p = provider(&server, 5);
    let first = p.chat(&simple_request("price of gold?")).await.unwrap();
    assert_eq!(first.tool_calls.len(), 1);

    let mut req = simple_request("price of gold?");
    req.messages.push(Message::Assistant {
        text: first.text.clone(),
        tool_calls: first.tool_calls.clone(),
    });
    req.messages.push(Message::ToolResult {
        call_id: "toolu_9".into(),
        name: "get_price".into(),
        content: "2400".into(),
        is_error: false,
    });
    p.chat(&req).await.unwrap();

    let reqs = server.received_requests().await.unwrap();
    let body: Value = reqs[1].body_json().unwrap();
    let assistant = &body["messages"][1]["content"];
    assert_eq!(
        assistant[0],
        json!({"type": "thinking", "thinking": "", "signature": "sig123"})
    );
    assert_eq!(assistant[1]["type"], "tool_use");
}

#[tokio::test]
async fn optional_thinking_and_effort_are_sent_only_when_configured() {
    let server = server_with(ResponseTemplate::new(200).set_body_json(json!({
        "content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn"
    })))
    .await;
    let p = provider(&server, 5)
        .with_thinking(json!({"type": "between_tools"}))
        .with_effort("low");
    p.chat(&simple_request("hi")).await.unwrap();
    let body = first_body(&server).await;
    assert_eq!(body["thinking"], json!({"type": "between_tools"}));
    assert_eq!(body["output_config"], json!({"effort": "low"}));
}

#[test]
fn build_provider_missing_key_is_config_error_naming_the_var() {
    let mut cfg = config(ProviderKind::Anthropic, "http://localhost:1", 5);
    cfg.api_key_env = Some("JARVIS_TEST_ANTHROPIC_KEY_UNSET_1".into());
    let err = build_provider(&cfg).err().expect("must fail");
    match err {
        LlmError::Config(m) => assert!(m.contains("JARVIS_TEST_ANTHROPIC_KEY_UNSET_1"), "{m}"),
        other => panic!("expected Config, got {other:?}"),
    }
}

#[test]
fn build_provider_without_api_key_env_is_config_error() {
    let cfg = config(ProviderKind::Anthropic, "http://localhost:1", 5);
    assert!(matches!(
        build_provider(&cfg).err(),
        Some(LlmError::Config(_))
    ));
}

#[test]
fn build_provider_empty_key_is_config_error() {
    // SAFETY: unique variable name, only touched by this test.
    unsafe { std::env::set_var("JARVIS_TEST_ANTHROPIC_KEY_EMPTY_2", "  ") };
    let mut cfg = config(ProviderKind::Anthropic, "http://localhost:1", 5);
    cfg.api_key_env = Some("JARVIS_TEST_ANTHROPIC_KEY_EMPTY_2".into());
    assert!(matches!(
        build_provider(&cfg).err(),
        Some(LlmError::Config(_))
    ));
}

#[test]
fn build_provider_ok_with_key_and_reports_name_and_model() {
    // SAFETY: unique variable name, only touched by this test.
    unsafe { std::env::set_var("JARVIS_TEST_ANTHROPIC_KEY_SET_3", "sk-ant-xyz") };
    let mut cfg = config(ProviderKind::Anthropic, "http://localhost:1", 5);
    cfg.api_key_env = Some("JARVIS_TEST_ANTHROPIC_KEY_SET_3".into());
    let p = build_provider(&cfg).unwrap();
    assert_eq!(p.name(), "test");
    assert_eq!(p.model(), "test-model");
}

#[test]
fn invalid_base_url_is_config_error() {
    let cfg = config(ProviderKind::Anthropic, "not a url", 5);
    assert!(matches!(
        AnthropicProvider::new(&cfg, "k").err(),
        Some(LlmError::Config(_))
    ));
}

#[test]
fn a_key_is_never_sent_over_plain_http_to_a_remote_host() {
    for url in [
        "http://api.example.com",
        "http://10.0.0.5:8080",
        "http://127.0.0.1.example.com",
        "ftp://api.example.com",
    ] {
        let err = AnthropicProvider::new(&config(ProviderKind::Anthropic, url, 5), "sk-ant-key")
            .err()
            .unwrap_or_else(|| panic!("{url} must be refused"));
        match err {
            LlmError::Config(m) => {
                assert!(m.contains("https"), "{m}");
                assert!(!m.contains("sk-ant-key"), "the key must not appear: {m}");
            }
            other => panic!("expected Config, got {other:?}"),
        }
    }
    for url in [
        "https://api.anthropic.com",
        "http://localhost:8080",
        "http://127.0.0.1:1",
        "http://[::1]:8080",
    ] {
        assert!(
            AnthropicProvider::new(&config(ProviderKind::Anthropic, url, 5), "k").is_ok(),
            "{url}"
        );
    }
}
