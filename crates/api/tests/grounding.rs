//! The grounding check in the agent loop: a reply whose figures no tool reported is sent back
//! once, and replaced if the model does it again. Identical for every persona.

mod common;

use axum::http::StatusCode;
use common::*;
use jarvis_api::agent::{MAX_ROUNDS, ROUND_LIMIT_REPLY};
use jarvis_api::grounding::UNVERIFIED_REPLY;
use jarvis_bridge_client::mock::MockBridge;
use jarvis_llm::Message;
use serde_json::{Value, json};

/// What the model said in the failure found in live testing.
const INVENTED: &str = "Gold is currently trading at 2654.80 USD per ounce.";
const GROUNDED: &str = "Gold is trading at 2391.12 bid and 2391.40 ask.";

async fn set_persona(app: &TestApp, id: &str) {
    let (status, body) = request(
        &app.router,
        "PUT",
        "/api/settings",
        Some(json!({ "persona": id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn rows_of(app: &TestApp, kind: &str) -> Vec<jarvis_api::audit::AuditRow> {
    app.audit
        .rows(None)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == kind)
        .collect()
}

fn last_message(llm: &ScriptedLlm, request: usize) -> Message {
    llm.requests.lock().unwrap()[request]
        .messages
        .last()
        .cloned()
        .expect("a message")
}

#[tokio::test]
async fn a_price_stated_without_a_tool_is_corrected_and_then_answered_from_the_tool() {
    let mock = MockBridge::start().await;
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            text(INVENTED),
            tool_use("t1", "get_price", json!({ "symbol": "gold" })),
            text(GROUNDED),
        ],
    );
    let app = build_app(&mock, llm.clone(), Options::default()).await;
    let (status, body) = chat(&app, "what is gold trading at?").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["reply"], GROUNDED);
    assert!(!body.to_string().contains("2654"), "{body}");
    assert_eq!(body["tool_calls"].as_array().unwrap().len(), 1);
    assert_eq!(body["tool_calls"][0]["name"], "get_price");
    assert_eq!(mock.request_count(), 1, "the price came from the bridge");

    // The correction was sent, once, after the rejected reply.
    assert_eq!(llm.request_count(), 3);
    match last_message(&llm, 1) {
        Message::User { text } => {
            assert!(text.contains("did not come from a tool"), "{text}");
            assert!(text.contains("2654.80"), "{text}");
            assert!(text.contains("Call the appropriate read tool"), "{text}");
        }
        other => panic!("expected the correction as a user message, got {other:?}"),
    }
    // The third request still carries it, plus the tool result.
    let third = llm.requests.lock().unwrap()[2].messages.clone();
    assert!(
        third
            .iter()
            .any(|m| matches!(m, Message::ToolResult { name, .. } if name == "get_price"))
    );

    let rows = rows_of(&app, "ungrounded_reply").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].payload["reply"], INVENTED);
    assert_eq!(rows[0].payload["figures"], json!(["2654.80"]));
    assert_eq!(rows[0].payload["persona"], "jarvis");
    assert_eq!(rows[0].persona.as_deref(), Some("jarvis"));
    assert!(rows_of(&app, "ungrounded_reply_blocked").await.is_empty());
    // The reply the user got is the one audited as the answer.
    let replies = rows_of(&app, "assistant_reply").await;
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].payload["text"], GROUNDED);

    // The stored conversation has neither the rejected reply nor the correction.
    let sid = body["session_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(app.state.agent().history_len(sid), 4);
}

#[tokio::test]
async fn staying_ungrounded_twice_gives_the_safe_reply_and_no_invented_figure() {
    let mock = MockBridge::start().await;
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            text(INVENTED),
            text("Sorry. Gold is 2655.10 right now, I am sure."),
        ],
    );
    let app = build_app(&mock, llm.clone(), Options::default()).await;
    let (status, body) = chat(&app, "what is gold trading at?").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["reply"], UNVERIFIED_REPLY);
    let shown = body.to_string();
    assert!(
        !shown.contains("2654") && !shown.contains("2655"),
        "{shown}"
    );
    assert!(body["tool_calls"].as_array().unwrap().is_empty());
    assert_eq!(llm.request_count(), 2, "exactly one retry");
    assert_eq!(mock.request_count(), 0);

    assert_eq!(rows_of(&app, "ungrounded_reply").await.len(), 1);
    let blocked = rows_of(&app, "ungrounded_reply_blocked").await;
    assert_eq!(blocked.len(), 1);
    assert_eq!(
        blocked[0].payload["reply"],
        "Sorry. Gold is 2655.10 right now, I am sure."
    );
    assert_eq!(blocked[0].payload["figures"], json!(["2655.10"]));
    assert_eq!(blocked[0].payload["persona"], "jarvis");
    // What was audited as the answer is the safe reply, never the invented one.
    let replies = rows_of(&app, "assistant_reply").await;
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].payload["text"], UNVERIFIED_REPLY);

    // History: the user's message and the safe reply only.
    let sid = body["session_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(app.state.agent().history_len(sid), 2);
}

#[tokio::test]
async fn a_tool_that_does_not_help_does_not_save_an_invented_figure() {
    // The model calls a tool after the correction but still states a made-up price.
    let mock = MockBridge::start().await;
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            text(INVENTED),
            tool_use("t1", "get_price", json!({ "symbol": "gold" })),
            text("Gold is 2654.80, as I said."),
        ],
    );
    let app = build_app(&mock, llm.clone(), Options::default()).await;
    let (_, body) = chat(&app, "gold?").await;
    assert_eq!(body["reply"], UNVERIFIED_REPLY);
    assert_eq!(llm.request_count(), 3, "no second retry");
    assert_eq!(rows_of(&app, "ungrounded_reply_blocked").await.len(), 1);
}

#[tokio::test]
async fn the_retry_counts_toward_the_round_limit() {
    let mock = MockBridge::start().await;
    let mut script = vec![text(INVENTED)];
    for i in 0..MAX_ROUNDS {
        script.push(tool_use(
            &format!("t{i}"),
            "get_price",
            json!({ "symbol": "gold" }),
        ));
    }
    let llm = ScriptedLlm::new("fake", script);
    let app = build_app(&mock, llm.clone(), Options::default()).await;
    let (status, body) = chat(&app, "what is gold trading at?").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["reply"], ROUND_LIMIT_REPLY);
    assert_eq!(llm.request_count(), MAX_ROUNDS);
}

#[tokio::test]
async fn grounded_answers_pass_untouched_with_no_extra_model_call() {
    let mock = MockBridge::start().await;

    // A tool-backed answer: one model call for the tool, one for the answer.
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            tool_use("t1", "get_price", json!({ "symbol": "gold" })),
            text(GROUNDED),
        ],
    );
    let app = build_app(&mock, llm.clone(), Options::default()).await;
    let (_, body) = chat(&app, "what is gold trading at?").await;
    assert_eq!(body["reply"], GROUNDED);
    assert_eq!(llm.request_count(), 2);
    assert!(rows_of(&app, "ungrounded_reply").await.is_empty());
    assert!(rows_of(&app, "ungrounded_reply_blocked").await.is_empty());

    // The user's own numbers are quoted back.
    let llm = ScriptedLlm::new(
        "fake",
        vec![text("Do you want to buy or sell 0.01 lots of gold?")],
    );
    let app = build_app(&mock, llm.clone(), Options::default()).await;
    let (_, body) = chat(&app, "0.01 gold").await;
    assert_eq!(
        body["reply"],
        "Do you want to buy or sell 0.01 lots of gold?"
    );
    assert_eq!(llm.request_count(), 1);

    // A question without numbers, and a refusal without any.
    for reply in [
        "Which symbol do you mean?",
        "I can't do that, the kill switch is on.",
        "I'm sorry, I don't know.",
    ] {
        let llm = ScriptedLlm::new("fake", vec![text(reply)]);
        let app = build_app(&mock, llm.clone(), Options::default()).await;
        let (_, body) = chat(&app, "buy").await;
        assert_eq!(body["reply"], reply);
        assert_eq!(llm.request_count(), 1);
    }
}

#[tokio::test]
async fn the_clock_and_earlier_tool_results_of_the_session_are_grounded() {
    let mock = MockBridge::start().await;
    let year = chrono::Utc::now().format("%Y").to_string();
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            tool_use("t1", "get_account", json!({})),
            text("Your balance is 10,000.00 USD."),
            // Next turn, no tool: repeats the earlier balance and the year from the clock.
            text(&format!("It was 10000 USD, and it is {year} now.")),
        ],
    );
    let app = build_app(&mock, llm.clone(), Options::default()).await;
    let (_, first) = chat(&app, "balance?").await;
    assert_eq!(first["reply"], "Your balance is 10,000.00 USD.");
    let (_, second) = request(
        &app.router,
        "POST",
        "/api/chat",
        Some(json!({ "message": "again?", "session_id": first["session_id"] })),
    )
    .await;
    assert_eq!(
        second["reply"],
        format!("It was 10000 USD, and it is {year} now.")
    );
    assert_eq!(llm.request_count(), 3);
    assert!(rows_of(&app, "ungrounded_reply").await.is_empty());
}

#[tokio::test]
async fn proposals_and_refusals_are_unaffected() {
    let mock = MockBridge::start().await;

    // The preview description comes from the tool result, so its figures are grounded.
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            tool_use(
                "t1",
                "propose_order",
                json!({ "symbol": "gold", "side": "buy", "volume": 0.1 }),
            ),
            text("Buy 0.10 XAUUSD at market. Nothing has happened yet. Please confirm."),
        ],
    );
    let app = build_app(&mock, llm.clone(), Options::default()).await;
    let (status, body) = chat(&app, "buy 0.1 gold").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["reply"],
        "Buy 0.10 XAUUSD at market. Nothing has happened yet. Please confirm."
    );
    assert!(body["preview"]["token"].is_string(), "{body}");
    assert_eq!(llm.request_count(), 2);
    assert!(rows_of(&app, "ungrounded_reply").await.is_empty());

    // A refusal names the limit and its value, as the shared rules ask. The value comes
    // from the tool result.
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            tool_use(
                "t1",
                "propose_order",
                json!({ "symbol": "gold", "side": "buy", "volume": 5 }),
            ),
            text("I can't propose that: the max_lot limit is 1.0 lots and you asked for 5."),
        ],
    );
    let app = build_app(&mock, llm.clone(), Options::default()).await;
    let (_, body) = chat(&app, "buy 5 gold").await;
    assert!(body["preview"].is_null());
    assert_eq!(
        body["reply"],
        "I can't propose that: the max_lot limit is 1.0 lots and you asked for 5."
    );
    assert_eq!(llm.request_count(), 2);
    assert!(rows_of(&app, "ungrounded_reply").await.is_empty());
}

// ------------------------------------------------------------------ parity

/// Everything the user and the audit trail see for one persona, without ids and times.
async fn run_for(persona: &str, script: Vec<Value>) -> Value {
    let mock = MockBridge::start().await;
    let responses = script
        .into_iter()
        .map(|step| match step["tool"].as_str() {
            Some(tool) => tool_use("t1", tool, step["args"].clone()),
            None => text(step["text"].as_str().unwrap()),
        })
        .collect();
    let llm = ScriptedLlm::new("fake", responses);
    let app = build_app(&mock, llm.clone(), Options::default()).await;
    set_persona(&app, persona).await;
    let (status, body) = chat(&app, "what is gold trading at?").await;
    let rows = app.audit.rows(None).await.unwrap();
    for row in &rows {
        if row.session_id.is_some() {
            assert_eq!(row.persona.as_deref(), Some(persona), "{}", row.kind);
        }
    }
    let requests: Vec<Value> = llm
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| serde_json::to_value(&r.messages).unwrap())
        .collect();
    json!({
        "status": status.as_u16(),
        "reply": body["reply"],
        "tool_calls": body["tool_calls"].as_array().unwrap().iter()
            .map(|c| c["name"].clone()).collect::<Vec<_>>(),
        "model_calls": llm.request_count(),
        "bridge_requests": mock.request_count(),
        "messages_sent": requests,
        "audit": rows.iter().map(|r| json!({
            "kind": r.kind,
            "payload": if r.kind.starts_with("ungrounded") { r.payload.clone() } else { Value::Null },
        })).collect::<Vec<_>>(),
    })
}

fn drop_persona_names(mut value: Value, persona: &str) -> Value {
    // The payload of an ungrounded_reply row names the persona; compare the rest.
    if let Some(rows) = value["audit"].as_array_mut() {
        for row in rows {
            if row["payload"].is_object() {
                assert_eq!(row["payload"]["persona"], persona);
                row["payload"]["persona"] = Value::Null;
            }
        }
    }
    value
}

#[tokio::test]
async fn grounding_behaves_identically_for_every_persona() {
    let corrected = || {
        vec![
            json!({ "text": INVENTED }),
            json!({ "tool": "get_price", "args": { "symbol": "gold" } }),
            json!({ "text": GROUNDED }),
        ]
    };
    let blocked = || {
        vec![
            json!({ "text": INVENTED }),
            json!({ "text": "Gold is 2655.10 for sure." }),
        ]
    };
    for (name, script) in [
        ("corrected", corrected as fn() -> Vec<Value>),
        ("blocked", blocked),
    ] {
        let jarvis = drop_persona_names(run_for("jarvis", script()).await, "jarvis");
        let friday = drop_persona_names(run_for("friday", script()).await, "friday");
        assert_eq!(jarvis, friday, "{name}");
    }
    // And the two runs did what they claim.
    let blocked_run = run_for("friday", blocked()).await;
    assert_eq!(blocked_run["reply"], UNVERIFIED_REPLY);
    let kinds: Vec<&str> = blocked_run["audit"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"ungrounded_reply"));
    assert!(kinds.contains(&"ungrounded_reply_blocked"));
    let corrected_run = run_for("friday", corrected()).await;
    assert_eq!(corrected_run["reply"], GROUNDED);
    assert_eq!(corrected_run["model_calls"], 3);
}
