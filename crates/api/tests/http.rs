mod common;

use axum::http::StatusCode;
use common::*;
use jarvis_api::config::Config;
use jarvis_bridge_client::mock::{MockBridge, MockState};
use jarvis_bridge_client::protocol::TradeMode;
use serde_json::{Value, json};

fn idle_llm() -> std::sync::Arc<ScriptedLlm> {
    ScriptedLlm::new("fake", vec![])
}

// ---------------------------------------------------------------- status

#[tokio::test]
async fn status_with_bridge_up() {
    let mock = MockBridge::start().await;
    let app = build_app(&mock.endpoint, idle_llm(), Options::default()).await;
    let (status, body) = request(&app.router, "GET", "/api/status", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["mode"], "demo");
    assert_eq!(body["mode_mismatch"], false);
    assert_eq!(body["bridge"]["connected"], true);
    assert_eq!(body["bridge"]["trade_mode"], "demo");
    assert_eq!(body["bridge"]["protocol_version"], 1);
    assert_eq!(body["bridge"]["terminal_connected"], true);
    assert!(body["bridge"]["latency_ms"].is_u64());
    assert!(body["bridge"]["error"].is_null());
    assert_eq!(body["llm"]["default"], "fake");
    let providers = body["llm"]["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 2);
    assert_eq!(providers[0]["name"], "fake");
    assert_eq!(providers[0]["kind"], "ollama");
    assert_eq!(providers[0]["model"], "fake-model");
    assert_eq!(providers[0]["available"], true);
    assert_eq!(providers[1]["name"], "claude");
    assert_eq!(providers[1]["kind"], "anthropic");
    assert_eq!(providers[1]["available"], false);
}

#[tokio::test]
async fn status_with_bridge_down_still_returns_200() {
    let app = build_app(DEAD_ENDPOINT, idle_llm(), Options::default()).await;
    let (status, body) = request(&app.router, "GET", "/api/status", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["bridge"]["connected"], false);
    assert!(
        body["bridge"]["error"]
            .as_str()
            .is_some_and(|e| !e.is_empty())
    );
    for field in [
        "latency_ms",
        "ea_version",
        "protocol_version",
        "trade_mode",
        "terminal_connected",
    ] {
        assert!(body["bridge"][field].is_null(), "{field} should be null");
    }
    assert_eq!(body["mode"], "demo");
    assert_eq!(body["llm"]["default"], "fake");
}

// ------------------------------------------------- account / positions

#[tokio::test]
async fn account_and_positions() {
    let mock = MockBridge::start().await;
    let app = build_app(&mock.endpoint, idle_llm(), Options::default()).await;

    let (status, account) = request(&app.router, "GET", "/api/account", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(account["currency"], "USD");
    assert_eq!(account["trade_mode"], "demo");
    for forbidden in ["login", "name", "server"] {
        assert!(
            account.get(forbidden).is_none(),
            "{forbidden} must not be exposed"
        );
    }

    let (status, positions) = request(&app.router, "GET", "/api/positions", None).await;
    assert_eq!(status, StatusCode::OK);
    let list = positions["positions"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["symbol"], "XAUUSD");
    assert_eq!(list[0]["side"], "buy");
    assert!(list[0]["sl"].is_null());
}

#[tokio::test]
async fn account_with_bridge_down_is_503() {
    let app = build_app(DEAD_ENDPOINT, idle_llm(), Options::default()).await;
    let (status, body) = request(&app.router, "GET", "/api/account", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "bridge_unavailable");
    assert!(body["error"]["message"].as_str().is_some());
}

// -------------------------------------------------------------- watchlist

#[tokio::test]
async fn watchlist_lists_partial_errors() {
    let mock = MockBridge::start().await;
    let opts = Options {
        watchlist: &["XAUUSD", "NOSUCHSYM", "US100"],
        ..Options::default()
    };
    let app = build_app(&mock.endpoint, idle_llm(), opts).await;
    let (status, body) = request(&app.router, "GET", "/api/watchlist", None).await;
    assert_eq!(status, StatusCode::OK);
    let prices = body["prices"].as_array().unwrap();
    let symbols: Vec<&str> = prices
        .iter()
        .map(|p| p["symbol"].as_str().unwrap())
        .collect();
    assert_eq!(symbols, ["XAUUSD", "US100"]);
    let errors = body["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["symbol"], "NOSUCHSYM");
    assert!(
        errors[0]["message"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("unknown symbol")
    );
}

#[tokio::test]
async fn watchlist_with_bridge_down_is_503() {
    let app = build_app(DEAD_ENDPOINT, idle_llm(), Options::default()).await;
    let (status, body) = request(&app.router, "GET", "/api/watchlist", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "bridge_unavailable");
}

// ------------------------------------------------------------------- chat

#[tokio::test]
async fn chat_happy_path_calls_tool_and_writes_audit() {
    let mock = MockBridge::start().await;
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            tool_use("call_1", "get_price", json!({ "symbol": "gold" })),
            text("Gold is 2391.12 bid, 2391.40 ask."),
        ],
    );
    let app = build_app(&mock.endpoint, llm.clone(), Options::default()).await;

    let (status, body) = chat(&app, "what's gold at?").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["reply"], "Gold is 2391.12 bid, 2391.40 ask.");
    assert_eq!(body["provider"], "fake");
    let sid = body["session_id"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(sid).is_ok());
    let calls = body["tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["name"], "get_price");
    assert_eq!(calls[0]["arguments"], json!({ "symbol": "gold" }));
    assert_eq!(calls[0]["ok"], true);
    assert!(calls[0]["duration_ms"].is_u64());

    // The bridge received exactly one request for the resolved symbol (the status ping
    // is not involved: chat does not ping).
    assert_eq!(mock.request_count(), 1);

    // The second model call saw the tool result with the XAUUSD price.
    {
        let requests = llm.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let second = serde_json::to_string(&requests[1].messages).unwrap();
        assert!(second.contains("XAUUSD"), "{second}");
        assert!(second.contains("2391.12"), "{second}");
    }

    // Audit rows, in order.
    let rows = app.audit.rows(None).await.unwrap();
    let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "utterance",
            "bridge_request",
            "bridge_reply",
            "tool_call",
            "assistant_reply"
        ]
    );
    assert!(rows.iter().all(|r| r.session_id.as_deref() == Some(sid)));
    assert_eq!(rows[0].payload["text"], "what's gold at?");
    assert_eq!(rows[1].payload["cmd"], "get_price");
    assert_eq!(rows[1].payload["args"]["symbol"], "XAUUSD");
    assert_eq!(rows[2].payload["symbol"], "XAUUSD");
    assert_eq!(rows[3].payload["name"], "get_price");
    assert_eq!(rows[3].payload["ok"], true);
    assert_eq!(rows[4].payload["text"], "Gold is 2391.12 bid, 2391.40 ask.");
}

#[tokio::test]
async fn chat_suffix_is_applied_before_the_bridge_call() {
    // Mock only knows XAUUSD, so XAUUSD.m is an unknown symbol: proves the suffix was sent.
    let mock = MockBridge::start().await;
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            tool_use("c1", "get_price", json!({ "symbol": "gold" })),
            text("done"),
        ],
    );
    let opts = Options {
        suffix: ".m",
        ..Options::default()
    };
    let app = build_app(&mock.endpoint, llm, opts).await;
    let (status, body) = chat(&app, "gold?").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tool_calls"][0]["ok"], false);
    let rows = app.audit.rows(None).await.unwrap();
    let request_row = rows.iter().find(|r| r.kind == "bridge_request").unwrap();
    assert_eq!(request_row.payload["args"]["symbol"], "XAUUSD.m");
}

#[tokio::test]
async fn chat_tool_error_is_given_to_the_model_not_the_client() {
    let mock = MockBridge::start().await;
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            tool_use("c1", "get_price", json!({ "symbol": "goldd" })),
            text("I couldn't find that symbol. Could you check its name?"),
        ],
    );
    let app = build_app(&mock.endpoint, llm.clone(), Options::default()).await;
    let (status, body) = chat(&app, "price of goldd").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tool_calls"][0]["ok"], false);
    let requests = llm.requests.lock().unwrap();
    let jarvis_llm::Message::ToolResult {
        content, is_error, ..
    } = &requests[1].messages[2]
    else {
        panic!("expected a tool result, got {:?}", requests[1].messages);
    };
    assert!(*is_error);
    assert!(content.contains("check the symbol name"), "{content}");
}

#[tokio::test]
async fn chat_with_bridge_down_still_answers() {
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            tool_use("c1", "get_account", json!({})),
            text("MetaTrader seems offline."),
        ],
    );
    let app = build_app(DEAD_ENDPOINT, llm, Options::default()).await;
    let (status, body) = chat(&app, "balance?").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tool_calls"][0]["ok"], false);
    assert_eq!(body["reply"], "MetaTrader seems offline.");
}

#[tokio::test]
async fn chat_validation() {
    let app = build_app(DEAD_ENDPOINT, idle_llm(), Options::default()).await;
    let bad = |msg: Value| request(&app.router, "POST", "/api/chat", Some(msg));

    for body in [
        json!({ "message": "" }),
        json!({ "message": "   " }),
        json!({ "message": "x".repeat(2001) }),
        json!({}),
        json!({ "message": "hi", "session_id": "not-a-uuid" }),
        json!({ "message": "hi", "provider": "nope" }),
        json!({ "message": "hi", "provider": "claude" }), // configured but unavailable
    ] {
        let (status, resp) = bad(body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(resp["error"]["code"], "bad_request", "{body}");
        assert!(
            resp["error"]["message"]
                .as_str()
                .is_some_and(|m| !m.is_empty())
        );
    }
    // Not JSON at all.
    let (status, _, bytes) =
        raw_request(&app.router, "POST", "/api/chat", &[], Some("hello".into())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["error"]["code"], "bad_request");

    // Exactly 2000 characters is fine (and counts characters, not bytes).
    let llm_ok = ScriptedLlm::new("fake", vec![text("ok"), text("ok")]);
    let ok_app = build_app(DEAD_ENDPOINT, llm_ok, Options::default()).await;
    let (status, _) = chat(&ok_app, &"x".repeat(2000)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = chat(&ok_app, &"é".repeat(2000)).await;
    assert_eq!(status, StatusCode::OK);

    // Nothing invalid reached the model or the audit log.
    assert_eq!(app.llm.request_count(), 0);
    assert!(app.audit.rows(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn chat_can_select_a_provider_by_name() {
    let app = build_app(
        DEAD_ENDPOINT,
        ScriptedLlm::new("fake", vec![text("hello")]),
        Options::default(),
    )
    .await;
    let (status, body) = request(
        &app.router,
        "POST",
        "/api/chat",
        Some(json!({ "message": "hi", "provider": "fake" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["provider"], "fake");
}

#[tokio::test]
async fn chat_stops_after_max_rounds() {
    let mock = MockBridge::start().await;
    let llm = ScriptedLlm::always("fake", tool_use_response("c", "get_account", json!({})));
    let app = build_app(&mock.endpoint, llm.clone(), Options::default()).await;
    let (status, body) = chat(&app, "loop forever").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(llm.request_count(), jarvis_api::agent::MAX_ROUNDS);
    assert_eq!(body["reply"], jarvis_api::agent::ROUND_LIMIT_REPLY);
    // 5 model rounds; the last one's tool call is not executed.
    assert_eq!(
        body["tool_calls"].as_array().unwrap().len(),
        jarvis_api::agent::MAX_ROUNDS - 1
    );
    assert_eq!(mock.request_count(), jarvis_api::agent::MAX_ROUNDS - 1);
    let kinds = audit_kinds(&app).await;
    assert_eq!(kinds.first().map(String::as_str), Some("utterance"));
    assert_eq!(kinds.last().map(String::as_str), Some("assistant_reply"));
}

#[tokio::test]
async fn chat_llm_failure_is_502_and_audited() {
    let llm = ScriptedLlm::new("fake", vec![Err(jarvis_llm::LlmError::Timeout)]);
    let app = build_app(DEAD_ENDPOINT, llm, Options::default()).await;
    let (status, body) = chat(&app, "hello").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["error"]["code"], "llm_error");
    assert_eq!(audit_kinds(&app).await, ["utterance", "error"]);
}

#[tokio::test]
async fn chat_session_keeps_history_and_caps_it() {
    let llm = ScriptedLlm::always("fake", text_response("ok"));
    let app = build_app(DEAD_ENDPOINT, llm.clone(), Options::default()).await;
    let (_, first) = chat(&app, "one").await;
    let sid = first["session_id"].as_str().unwrap().to_owned();

    let (status, second) = request(
        &app.router,
        "POST",
        "/api/chat",
        Some(json!({ "session_id": sid, "message": "two" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["session_id"], sid);
    {
        let requests = llm.requests.lock().unwrap();
        assert_eq!(requests[1].messages.len(), 3); // user, assistant, user
    }

    for i in 0..30 {
        request(
            &app.router,
            "POST",
            "/api/chat",
            Some(json!({ "session_id": sid, "message": format!("m{i}") })),
        )
        .await;
    }
    let id = uuid::Uuid::parse_str(&sid).unwrap();
    let len = app.state.agent().history_len(id);
    assert!(
        (30..=jarvis_api::agent::MAX_HISTORY_MESSAGES).contains(&len),
        "history len {len}"
    );
    let last = llm.requests.lock().unwrap().last().cloned().unwrap();
    assert!(last.messages.len() <= jarvis_api::agent::MAX_HISTORY_MESSAGES + 1);
    assert!(matches!(last.messages[0], jarvis_llm::Message::User { .. }));
}

#[tokio::test]
async fn system_prompt_and_tools_are_sent_to_the_model() {
    let llm = ScriptedLlm::new("fake", vec![text("hi")]);
    let app = build_app(DEAD_ENDPOINT, llm.clone(), Options::default()).await;
    chat(&app, "hello").await;
    let requests = llm.requests.lock().unwrap();
    let system = requests[0].system.as_deref().unwrap();
    assert!(system.contains("gold = XAUUSD"));
    assert!(system.contains("dow jones = US30"));
    let mut names: Vec<&str> = requests[0].tools.iter().map(|t| t.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "get_account",
            "get_history",
            "get_orders",
            "get_positions",
            "get_price",
            "propose_cancel",
            "propose_close",
            "propose_modify",
            "propose_order",
        ]
    );
}

// -------------------------------------------------------- rule 7 (modes)

fn real_account_state() -> MockState {
    let mut state = MockState::default();
    state.ping.trade_mode = TradeMode::Real;
    state.account.trade_mode = TradeMode::Real;
    state
}

#[tokio::test]
async fn demo_config_against_real_account_blocks_chat() {
    let mock = MockBridge::start_with(real_account_state()).await;
    let llm = ScriptedLlm::new("fake", vec![text("should never be used")]);
    let app = build_app(&mock.endpoint, llm.clone(), Options::default()).await;

    // Before any successful ping the flag is not set yet.
    assert!(!app.state.mode_mismatch());

    // A status call pings the bridge, detects the mismatch and reports it.
    let (status, body) = request(&app.router, "GET", "/api/status", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mode_mismatch"], true);
    assert_eq!(body["bridge"]["trade_mode"], "real");

    let (status, body) = chat(&app, "hello").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "mode_mismatch");
    assert_eq!(llm.request_count(), 0);
}

#[tokio::test]
async fn mismatch_is_also_detected_through_the_account_endpoint() {
    let mock = MockBridge::start_with(real_account_state()).await;
    let app = build_app(&mock.endpoint, idle_llm(), Options::default()).await;
    let (status, _) = request(&app.router, "GET", "/api/account", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(app.state.mode_mismatch());
    let (status, body) = chat(&app, "hi").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "mode_mismatch");
}

#[tokio::test]
async fn startup_refuses_demo_config_against_real_account() {
    let mock = MockBridge::start_with(real_account_state()).await;
    let app = build_app(&mock.endpoint, idle_llm(), Options::default()).await;
    let err = app.state.startup_check().await.unwrap_err().to_string();
    assert!(err.contains("refusing to start"), "{err}");
}

#[tokio::test]
async fn startup_accepts_live_config_against_real_account_and_demo_against_demo() {
    let real = MockBridge::start_with(real_account_state()).await;
    let live = build_app(
        &real.endpoint,
        idle_llm(),
        Options {
            mode: "live",
            ..Options::default()
        },
    )
    .await;
    live.state.startup_check().await.unwrap();
    assert!(!live.state.mode_mismatch());

    let demo_mock = MockBridge::start().await;
    let demo = build_app(&demo_mock.endpoint, idle_llm(), Options::default()).await;
    demo.state.startup_check().await.unwrap();
    assert!(!demo.state.mode_mismatch());
}

#[tokio::test]
async fn startup_with_bridge_down_is_not_fatal() {
    let app = build_app(DEAD_ENDPOINT, idle_llm(), Options::default()).await;
    app.state.startup_check().await.unwrap();
    assert!(!app.state.mode_mismatch());
}

// ------------------------------------------------------------------- CORS

async fn preflight(app: &TestApp, origin: &str) -> axum::http::HeaderMap {
    let (_, headers, _) = raw_request(
        &app.router,
        "OPTIONS",
        "/api/chat",
        &[
            ("origin", origin),
            ("access-control-request-method", "POST"),
            ("access-control-request-headers", "content-type"),
        ],
        None,
    )
    .await;
    headers
}

#[tokio::test]
async fn cors_is_off_by_default() {
    // Nothing is allowed unless `[server] dev_origins` says so, not even the Vite dev server.
    let app = build_app(DEAD_ENDPOINT, idle_llm(), Options::default()).await;
    for origin in [
        "http://localhost:5173",
        "http://127.0.0.1:5173",
        "http://evil.example",
    ] {
        let headers = preflight(&app, origin).await;
        assert!(
            headers.get("access-control-allow-origin").is_none(),
            "{origin} must not get CORS headers by default"
        );
    }
}

#[tokio::test]
async fn cors_allows_only_the_configured_dev_origins() {
    let app = build_app(
        DEAD_ENDPOINT,
        idle_llm(),
        Options {
            server: r#"dev_origins = ["http://localhost:5173", "http://127.0.0.1:5173"]"#,
            ..Options::default()
        },
    )
    .await;
    for origin in ["http://localhost:5173", "http://127.0.0.1:5173"] {
        let headers = preflight(&app, origin).await;
        assert_eq!(headers.get("access-control-allow-origin").unwrap(), origin);
        assert_eq!(
            headers.get("access-control-allow-credentials").unwrap(),
            "true"
        );
    }
    let headers = preflight(&app, "http://evil.example").await;
    assert!(headers.get("access-control-allow-origin").is_none());
    let headers = preflight(&app, "http://localhost:5174").await;
    assert!(headers.get("access-control-allow-origin").is_none());
}

#[tokio::test]
async fn bad_dev_origins_are_refused_at_startup() {
    for bad in [
        r#"dev_origins = ["*"]"#,
        r#"dev_origins = ["localhost:5173"]"#,
        r#"dev_origins = ["http://localhost:5173/app"]"#,
        r#"dev_origins = ["http://user:pw@localhost:5173"]"#,
        r#"dev_origins = [""]"#,
    ] {
        let toml = common::config_toml(
            "demo",
            DEAD_ENDPOINT,
            "",
            &[],
            common::DEFAULT_LIMITS,
            "",
            bad,
            false,
        );
        let err = Config::from_toml_str(&toml).unwrap_err();
        assert!(format!("{err:#}").contains("dev_origins"), "{bad}: {err:#}");
    }
}

// ------------------------------------------- cross-origin writes and DNS rebinding

#[tokio::test]
async fn state_changing_requests_from_a_foreign_origin_are_refused() {
    // SameSite=Strict stops other sites; this stops other origins of the *same* site, such as
    // another port on localhost (SECURITY.md: no web page may drive the trade endpoints).
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock.endpoint,
        ScriptedLlm::always("fake", text_response("hello")),
        Options::default(),
    )
    .await;
    let body = json!({ "message": "hi" }).to_string();
    for origin in [
        "http://evil.example",
        "http://localhost:5173", // same site as the API, different origin
        "http://127.0.0.1:9999",
        "null",
    ] {
        for (method, uri) in [
            ("POST", "/api/chat"),
            ("POST", "/api/confirm/abc"),
            ("POST", "/api/cancel/abc"),
            ("POST", "/api/killswitch"),
            ("POST", "/api/positions/close"),
            ("POST", "/api/auth/logout"),
        ] {
            let (status, headers, bytes) = raw_request(
                &app.router,
                method,
                uri,
                &[("origin", origin), ("host", "127.0.0.1:8787")],
                Some(body.clone()),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {uri} from {origin}"
            );
            let v: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(v["error"]["code"], "forbidden");
            assert!(headers.get("access-control-allow-origin").is_none());
        }
    }
    assert_eq!(mock.executed_writes(), 0);
    // Nothing reached the chat handler: no model call, no audit row.
    assert_eq!(app.llm.request_count(), 0);
    assert!(app.audit.rows(None).await.unwrap().is_empty());

    // The server's own origin and a non-browser client (no Origin) pass.
    let (status, _, _) = raw_request(
        &app.router,
        "POST",
        "/api/chat",
        &[
            ("origin", "http://127.0.0.1:8787"),
            ("host", "127.0.0.1:8787"),
        ],
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = raw_request(
        &app.router,
        "POST",
        "/api/chat",
        &[("host", "127.0.0.1:8787")],
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A configured dev origin passes.
    let dev = build_app(
        &mock.endpoint,
        ScriptedLlm::always("fake", text_response("hello")),
        Options {
            server: r#"dev_origins = ["http://localhost:5173"]"#,
            ..Options::default()
        },
    )
    .await;
    let (status, _, _) = raw_request(
        &dev.router,
        "POST",
        "/api/chat",
        &[
            ("origin", "http://localhost:5173"),
            ("host", "127.0.0.1:8787"),
        ],
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn reads_from_a_foreign_origin_get_no_cors_headers() {
    let mock = MockBridge::start().await;
    let app = build_app(&mock.endpoint, idle_llm(), Options::default()).await;
    let (status, headers, _) = raw_request(
        &app.router,
        "GET",
        "/api/status",
        &[
            ("origin", "http://evil.example"),
            ("host", "127.0.0.1:8787"),
        ],
        None,
    )
    .await;
    // The request is served, but a browser cannot read the answer without these headers.
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("access-control-allow-origin").is_none());
}

#[tokio::test]
async fn dns_rebinding_is_refused_while_login_is_off() {
    // With no session cookie in play, a web page whose DNS name is switched to 127.0.0.1 is
    // same-origin with the API. The Host header gives it away.
    let mock = MockBridge::start().await;
    let app = build_app(&mock.endpoint, idle_llm(), Options::default()).await;
    for host in [
        "evil.example:8787",
        "evil.example",
        "127.0.0.1.evil.example:8787",
        "192.168.1.5:8787",
    ] {
        for (method, uri) in [
            ("GET", "/api/status"),
            ("GET", "/api/positions"),
            ("POST", "/api/chat"),
            ("POST", "/api/killswitch"),
            ("OPTIONS", "/api/chat"),
        ] {
            let origin = format!("http://{host}");
            let (status, _, bytes) = raw_request(
                &app.router,
                method,
                uri,
                // Same-origin from the attacker page's point of view: Origin == Host.
                &[("host", host), ("origin", &origin)],
                Some("{}".to_owned()),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri} Host {host}");
            let v: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(v["error"]["code"], "forbidden");
        }
    }
    // Loopback names work, including with an explicit port.
    for host in [
        "localhost:8787",
        "127.0.0.1:8787",
        "[::1]:8787",
        "x.localhost",
    ] {
        let (status, _, _) =
            raw_request(&app.router, "GET", "/api/status", &[("host", host)], None).await;
        assert_eq!(status, StatusCode::OK, "Host {host}");
    }
}

#[tokio::test]
async fn host_names_are_not_restricted_when_login_is_on() {
    // With login on, the session cookie is bound to the real host name, so a rebinding page
    // has no cookie to send; deployments behind a reverse proxy keep working.
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock.endpoint,
        idle_llm(),
        Options {
            auth: true,
            ..Options::default()
        },
    )
    .await;
    let (status, _, _) = raw_request(
        &app.router,
        "GET",
        "/api/health",
        &[("host", "jarvis.example.com")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // ...but a foreign Origin is still refused on a write, logged in or not.
    let (status, _, _) = raw_request(
        &app.router,
        "POST",
        "/api/auth/login",
        &[
            ("host", "jarvis.example.com"),
            ("origin", "https://evil.example"),
        ],
        Some("{}".to_owned()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ------------------------------------------------------- server clock

#[tokio::test]
async fn history_ranges_use_the_learned_server_time_offset() {
    let mut state = MockState::default();
    state.ping.server_time += 3 * 3600; // broker is UTC+3
    let mock = MockBridge::start_with(state).await;
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            tool_use(
                "h1",
                "get_history",
                json!({ "from": "2026-10-30", "to": "2026-10-30T12:00:00Z" }),
            ),
            text("done"),
        ],
    );
    let app = build_app(&mock.endpoint, llm.clone(), Options::default()).await;

    // A status call pings the bridge and teaches the clock the offset.
    request(&app.router, "GET", "/api/status", None).await;
    assert_eq!(app.state.clock().offset_secs(), 3 * 3600);

    let (status, body) = chat(&app, "what did I trade today?").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["tool_calls"][0]["ok"], true);
    let rows = app.audit.rows(None).await.unwrap();
    let sent = &rows
        .iter()
        .find(|r| r.kind == "bridge_request")
        .unwrap()
        .payload["args"];
    // Date: already server-local. Instant: shifted by the offset.
    assert_eq!(sent["from"], 1_793_318_400);
    assert_eq!(sent["to"], 1_793_318_400 + 43_200 + 3 * 3600);

    let requests = llm.requests.lock().unwrap();
    assert!(
        requests[0]
            .system
            .as_deref()
            .unwrap()
            .contains("broker server time")
    );
}
