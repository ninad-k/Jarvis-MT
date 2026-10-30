mod common;

use common::*;
use jarvis_llm::ollama::OllamaProvider;
use jarvis_llm::{LlmError, LlmProvider, ProviderKind, StopReason, Usage, build_provider};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn server_with(resp: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(resp)
        .mount(&server)
        .await;
    server
}

fn provider(server: &MockServer, timeout: u64) -> OllamaProvider {
    OllamaProvider::new(&config(ProviderKind::Ollama, &server.uri(), timeout)).unwrap()
}

fn reply(message: serde_json::Value, done_reason: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "model": "test-model",
        "message": message,
        "done": true,
        "done_reason": done_reason,
        "prompt_eval_count": 21,
        "eval_count": 8
    }))
}

#[tokio::test]
async fn request_shape_multi_turn() {
    let server = server_with(reply(json!({"role": "assistant", "content": "ok"}), "stop")).await;
    provider(&server, 5)
        .chat(&multi_turn_request())
        .await
        .unwrap();

    let body = first_body(&server).await;
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["stream"], false);
    assert_eq!(body["think"], false);
    assert_eq!(body["options"]["num_predict"], 777);
    assert!((body["options"]["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);

    let tool = &body["tools"][0];
    assert_eq!(tool["type"], "function");
    assert_eq!(tool["function"]["name"], "get_price");
    assert_eq!(
        tool["function"]["parameters"]["properties"]["symbol"]["type"],
        "string"
    );

    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 5);
    assert_eq!(
        msgs[0],
        json!({"role": "system", "content": "You are Jarvis."})
    );
    assert_eq!(
        msgs[1],
        json!({"role": "user", "content": "price of gold?"})
    );
    assert_eq!(msgs[2]["role"], "assistant");
    assert_eq!(msgs[2]["content"], "Checking.");
    // Ollama wants arguments as an OBJECT.
    assert_eq!(
        msgs[2]["tool_calls"][0],
        json!({"function": {"name": "get_price", "arguments": {"symbol": "XAUUSD"}}})
    );
    assert_eq!(
        msgs[3],
        json!({"role": "tool", "tool_name": "get_price", "content": "{\"bid\":2400.5}"})
    );
    assert_eq!(msgs[4], json!({"role": "user", "content": "and silver?"}));
}

#[tokio::test]
async fn think_flag_is_configurable_and_temperature_optional() {
    let server = server_with(reply(json!({"role": "assistant", "content": "ok"}), "stop")).await;
    let p = provider(&server, 5).with_think(true);
    let mut req = simple_request("hi");
    req.max_tokens = Some(64);
    p.chat(&req).await.unwrap();
    let body = first_body(&server).await;
    assert_eq!(body["think"], true);
    assert_eq!(body["options"]["num_predict"], 64);
    assert!(body["options"].get("temperature").is_none());
}

#[tokio::test]
async fn parses_text_only_with_usage() {
    let server = server_with(reply(
        json!({"role": "assistant", "content": "Gold is up.", "thinking": "hmm"}),
        "stop",
    ))
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
            input_tokens: 21,
            output_tokens: 8
        })
    );
}

#[tokio::test]
async fn parses_tool_calls_and_generates_ids() {
    let server = server_with(reply(
        json!({"role": "assistant", "content": "", "tool_calls": [
            {"function": {"name": "get_price", "arguments": {"symbol": "XAUUSD"}}},
            {"function": {"name": "get_price", "arguments": {}}},
            {"function": {"name": "get_price"}}
        ]}),
        "stop",
    ))
    .await;
    let r = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap();
    assert_eq!(r.text, None);
    assert_eq!(r.stop_reason, StopReason::ToolUse);
    assert_eq!(r.tool_calls.len(), 3);
    assert_eq!(r.tool_calls[0], call("call_0", "XAUUSD"));
    assert_eq!(r.tool_calls[1].id, "call_1");
    assert_eq!(r.tool_calls[1].arguments, json!({}));
    assert_eq!(r.tool_calls[2].id, "call_2");
    assert_eq!(r.tool_calls[2].arguments, json!({}));
}

#[tokio::test]
async fn string_arguments_are_tolerated_and_bad_ones_rejected() {
    let server = server_with(reply(
        json!({"role": "assistant", "content": "", "tool_calls": [
            {"function": {"name": "get_price", "arguments": "{\"symbol\":\"EURUSD\"}"}}
        ]}),
        "stop",
    ))
    .await;
    let r = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap();
    assert_eq!(r.tool_calls[0].arguments, json!({"symbol": "EURUSD"}));

    let server = server_with(reply(
        json!({"role": "assistant", "content": "", "tool_calls": [
            {"function": {"name": "get_price", "arguments": "{oops"}}
        ]}),
        "stop",
    ))
    .await;
    let err = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::Decode(_)), "got {err:?}");
}

#[tokio::test]
async fn parses_max_tokens() {
    let server = server_with(reply(
        json!({"role": "assistant", "content": "cut off"}),
        "length",
    ))
    .await;
    let r = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap();
    assert_eq!(r.stop_reason, StopReason::MaxTokens);
    assert_eq!(r.text.as_deref(), Some("cut off"));
}

#[tokio::test]
async fn api_error_extracts_message() {
    let server = server_with(
        ResponseTemplate::new(404).set_body_json(json!({"error": "model 'nope' not found"})),
    )
    .await;
    let err = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap_err();
    match err {
        LlmError::Api { status, message } => {
            assert_eq!(status, 404);
            assert_eq!(message, "model 'nope' not found");
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[tokio::test]
async fn malformed_bodies_are_decode_errors() {
    for resp in [
        ResponseTemplate::new(200).set_body_string("not json"),
        ResponseTemplate::new(200).set_body_json(json!({"done": true})),
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
async fn trailing_slash_in_base_url() {
    let server = server_with(reply(json!({"role": "assistant", "content": "ok"}), "stop")).await;
    let base = format!("{}/", server.uri());
    let p = OllamaProvider::new(&config(ProviderKind::Ollama, &base, 5)).unwrap();
    assert_eq!(
        p.chat(&simple_request("hi")).await.unwrap().text.as_deref(),
        Some("ok")
    );
}

#[test]
fn build_provider_needs_no_key_and_defaults_base_url() {
    let mut cfg = config(ProviderKind::Ollama, "", 5);
    cfg.base_url = None;
    let p = build_provider(&cfg).unwrap();
    assert_eq!(p.name(), "test");
    assert_eq!(p.model(), "test-model");
}

#[tokio::test]
async fn inline_think_tags_are_stripped() {
    let server = server_with(reply(
        json!({"role": "assistant", "content": "Okay, reasoning here.\n</think>\n\nThe bid is 2412."}),
        "stop",
    ))
    .await;
    let r = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap();
    assert_eq!(r.text.as_deref(), Some("The bid is 2412."));

    let server = server_with(reply(
        json!({"role": "assistant", "content": "<think>still thinking when cut off"}),
        "length",
    ))
    .await;
    let r = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap();
    assert_eq!(r.text, None);
    assert_eq!(r.stop_reason, StopReason::MaxTokens);
}
