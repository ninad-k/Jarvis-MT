mod common;

use common::*;
use jarvis_llm::openai::OpenAiProvider;
use jarvis_llm::{LlmError, LlmProvider, ProviderKind, StopReason, Usage, build_provider};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn server_with(resp: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(resp)
        .mount(&server)
        .await;
    server
}

fn provider(server: &MockServer, timeout: u64) -> OpenAiProvider {
    // Default OpenAI base ends in /v1; mimic that here.
    let base = format!("{}/v1", server.uri());
    OpenAiProvider::new(&config(ProviderKind::Openai, &base, timeout), "sk-test-key").unwrap()
}

fn completion(message: serde_json::Value, finish: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "chatcmpl-1",
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
        "usage": {"prompt_tokens": 11, "completion_tokens": 3, "total_tokens": 14}
    }))
}

#[tokio::test]
async fn request_shape_multi_turn() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer sk-test-key"))
        .respond_with(completion(
            json!({"role": "assistant", "content": "ok"}),
            "stop",
        ))
        .expect(1)
        .mount(&server)
        .await;
    provider(&server, 5)
        .chat(&multi_turn_request())
        .await
        .unwrap();

    let body = first_body(&server).await;
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["max_completion_tokens"], 777);
    assert!(body.get("max_tokens").is_none());
    assert!((body["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);

    let tool = &body["tools"][0];
    assert_eq!(tool["type"], "function");
    assert_eq!(tool["function"]["name"], "get_price");
    assert_eq!(
        tool["function"]["parameters"]["required"],
        json!(["symbol"])
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
    let tc = &msgs[2]["tool_calls"][0];
    assert_eq!(tc["id"], "call_abc");
    assert_eq!(tc["type"], "function");
    assert_eq!(tc["function"]["name"], "get_price");
    // arguments must be a JSON string
    let args = tc["function"]["arguments"]
        .as_str()
        .expect("string arguments");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(args).unwrap(),
        json!({"symbol": "XAUUSD"})
    );
    assert_eq!(
        msgs[3],
        json!({"role": "tool", "tool_call_id": "call_abc", "content": "{\"bid\":2400.5}"})
    );
    assert_eq!(msgs[4], json!({"role": "user", "content": "and silver?"}));
}

#[tokio::test]
async fn omits_temperature_and_tools_when_unset() {
    let server = server_with(completion(
        json!({"role": "assistant", "content": "ok"}),
        "stop",
    ))
    .await;
    let mut req = simple_request("hi");
    req.tools.clear();
    req.system = None;
    req.max_tokens = Some(42);
    provider(&server, 5).chat(&req).await.unwrap();
    let body = first_body(&server).await;
    assert!(body.get("temperature").is_none());
    assert!(body.get("tools").is_none());
    assert_eq!(body["max_completion_tokens"], 42);
    assert_eq!(body["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn assistant_tool_call_without_text_sends_null_content() {
    let server = server_with(completion(
        json!({"role": "assistant", "content": "ok"}),
        "stop",
    ))
    .await;
    let mut req = multi_turn_request();
    if let jarvis_llm::Message::Assistant { text, .. } = &mut req.messages[1] {
        *text = None;
    }
    provider(&server, 5).chat(&req).await.unwrap();
    let body = first_body(&server).await;
    assert!(body["messages"][2]["content"].is_null());
}

#[tokio::test]
async fn parses_text_only() {
    let server = server_with(completion(
        json!({"role": "assistant", "content": "Gold is up."}),
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
            input_tokens: 11,
            output_tokens: 3
        })
    );
}

#[tokio::test]
async fn parses_parallel_tool_calls() {
    let server = server_with(completion(
        json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [
                {"id": "call_1", "type": "function",
                 "function": {"name": "get_price", "arguments": "{\"symbol\":\"XAUUSD\"}"}},
                {"id": "call_2", "type": "function",
                 "function": {"name": "get_price", "arguments": ""}}
            ]
        }),
        "tool_calls",
    ))
    .await;
    let r = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap();
    assert_eq!(r.text, None);
    assert_eq!(r.stop_reason, StopReason::ToolUse);
    assert_eq!(r.tool_calls.len(), 2);
    assert_eq!(r.tool_calls[0], call("call_1", "XAUUSD"));
    assert_eq!(r.tool_calls[1].arguments, json!({}));
}

#[tokio::test]
async fn parses_length_and_content_filter() {
    let server = server_with(completion(
        json!({"role": "assistant", "content": "cut"}),
        "length",
    ))
    .await;
    let r = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap();
    assert_eq!(r.stop_reason, StopReason::MaxTokens);

    let server = server_with(completion(
        json!({"role": "assistant", "content": null}),
        "content_filter",
    ))
    .await;
    let r = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap();
    assert_eq!(r.stop_reason, StopReason::Other("content_filter".into()));
}

#[tokio::test]
async fn refusal_is_surfaced_as_text() {
    let server = server_with(completion(
        json!({"role": "assistant", "content": null, "refusal": "I can't help with that."}),
        "stop",
    ))
    .await;
    let r = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap();
    assert_eq!(r.text.as_deref(), Some("I can't help with that."));
}

#[tokio::test]
async fn api_error_4xx_extracts_message() {
    let server = server_with(ResponseTemplate::new(401).set_body_json(json!({
        "error": {"message": "Incorrect API key provided", "type": "invalid_request_error", "code": "invalid_api_key"}
    })))
    .await;
    let err = provider(&server, 5)
        .chat(&simple_request("hi"))
        .await
        .unwrap_err();
    match err {
        LlmError::Api { status, message } => {
            assert_eq!(status, 401);
            assert_eq!(message, "Incorrect API key provided");
            assert!(!message.contains("sk-test-key"));
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[tokio::test]
async fn malformed_bodies_are_decode_errors() {
    for resp in [
        ResponseTemplate::new(200).set_body_string("nope"),
        ResponseTemplate::new(200).set_body_json(json!({"choices": []})),
        ResponseTemplate::new(200).set_body_json(json!({"foo": 1})),
        // arguments that are not valid JSON
        completion(
            json!({"role": "assistant", "content": null, "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "get_price", "arguments": "{\"symbol\": "}}
            ]}),
            "tool_calls",
        ),
        // arguments that are valid JSON but not an object
        completion(
            json!({"role": "assistant", "content": null, "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "get_price", "arguments": "[1]"}}
            ]}),
            "tool_calls",
        ),
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
    let server = server_with(completion(
        json!({"role": "assistant", "content": "ok"}),
        "stop",
    ))
    .await;
    let base = format!("{}/v1/", server.uri());
    let p = OpenAiProvider::new(&config(ProviderKind::Openai, &base, 5), "k").unwrap();
    assert_eq!(
        p.chat(&simple_request("hi")).await.unwrap().text.as_deref(),
        Some("ok")
    );
}

#[test]
fn build_provider_missing_key_is_config_error_naming_the_var() {
    let mut cfg = config(ProviderKind::Openai, "http://localhost:1", 5);
    cfg.api_key_env = Some("JARVIS_TEST_OPENAI_KEY_UNSET_1".into());
    match build_provider(&cfg).err().expect("must fail") {
        LlmError::Config(m) => assert!(m.contains("JARVIS_TEST_OPENAI_KEY_UNSET_1"), "{m}"),
        other => panic!("expected Config, got {other:?}"),
    }
}

#[test]
fn build_provider_ok_with_key() {
    // SAFETY: unique variable name, only touched by this test.
    unsafe { std::env::set_var("JARVIS_TEST_OPENAI_KEY_SET_2", "sk-xyz") };
    let mut cfg = config(ProviderKind::Openai, "http://localhost:1", 5);
    cfg.api_key_env = Some("JARVIS_TEST_OPENAI_KEY_SET_2".into());
    let p = build_provider(&cfg).unwrap();
    assert_eq!(p.name(), "test");
    assert_eq!(p.model(), "test-model");
}

#[test]
fn a_key_is_never_sent_over_plain_http_to_a_remote_host() {
    for url in ["http://api.example.com/v1", "http://192.168.1.2:8000/v1"] {
        let err = OpenAiProvider::new(&config(ProviderKind::Openai, url, 5), "sk-key")
            .err()
            .unwrap_or_else(|| panic!("{url} must be refused"));
        assert!(matches!(err, LlmError::Config(_)), "{err:?}");
    }
    for url in ["https://api.openai.com/v1", "http://localhost:11434/v1"] {
        assert!(
            OpenAiProvider::new(&config(ProviderKind::Openai, url, 5), "k").is_ok(),
            "{url}"
        );
    }
}
