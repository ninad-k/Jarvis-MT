//! Personas, user settings and tighten-only limits (docs/api.md, "Personas and user settings").
//!
//! The central claim is in `persona_parity_*`: the persona changes how the assistant sounds
//! and looks, never what happens. The same scripted conversation runs once with Jarvis and
//! once with Friday and must produce identical outcomes everywhere except the persona name.

mod common;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::http::StatusCode;
use common::*;
use jarvis_api::AppState;
use jarvis_api::clock::{ManualClock, TimeSource};
use jarvis_api::events::Event;
use jarvis_api::trading::WriteSink;
use jarvis_api::voice::Voice;
use jarvis_bridge_client::mock::MockBridge;
use jarvis_bridge_client::protocol::{Command, TradeResult};
use jarvis_bridge_client::{BridgeClient, BridgeError};
use jarvis_llm::Message;
use jarvis_voice::{SpeechToText, TextToSpeech, Transcript, VoiceError};
use serde_json::{Value, json};

// ------------------------------------------------------------------ helpers

async fn put(app: &TestApp, uri: &str, body: Value) -> (StatusCode, Value) {
    request(&app.router, "PUT", uri, Some(body)).await
}

async fn get(app: &TestApp, uri: &str) -> (StatusCode, Value) {
    request(&app.router, "GET", uri, None).await
}

async fn post(app: &TestApp, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    request(&app.router, "POST", uri, body).await
}

async fn delete(app: &TestApp, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    request(&app.router, "DELETE", uri, body).await
}

async fn set_persona(app: &TestApp, id: &str) {
    let (status, body) = put(app, "/api/settings", json!({ "persona": id })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["persona"], id);
}

fn buy_gold() -> Value {
    json!({ "symbol": "gold", "side": "buy", "volume": 0.1 })
}

fn llm_script(steps: Vec<(&str, Value)>) -> Arc<ScriptedLlm> {
    let mut script = Vec::new();
    for (i, (tool, args)) in steps.into_iter().enumerate() {
        script.push(tool_use(&format!("t{i}"), tool, args));
        script.push(text("Here is the preview. Please confirm."));
    }
    ScriptedLlm::new("fake", script)
}

/// Records every write command on its way to the bridge.
struct RecordingSink {
    inner: BridgeClient,
    commands: Mutex<Vec<Value>>,
}

#[async_trait]
impl WriteSink for RecordingSink {
    async fn send_write(&self, command: &Command) -> Result<TradeResult, BridgeError> {
        self.commands
            .lock()
            .unwrap()
            .push(serde_json::to_value(command).unwrap());
        self.inner.send_write(command).await
    }
}

fn token_of(body: &Value) -> String {
    body["preview"]["token"]
        .as_str()
        .unwrap_or_else(|| panic!("no preview token in {body}"))
        .to_owned()
}

fn system_of(llm: &ScriptedLlm, request: usize) -> String {
    llm.requests.lock().unwrap()[request]
        .system
        .clone()
        .expect("a system prompt")
}

// ------------------------------------------------------------------ parity

/// Everything that must not depend on the persona.
#[derive(Debug, PartialEq)]
struct Outcome {
    /// Status and normalised body of every step.
    steps: Vec<Value>,
    /// Write commands that reached the bridge.
    writes: Vec<Value>,
    /// Every request the bridge received, reads included.
    bridge_requests: usize,
    executed_writes: usize,
    positions_left: usize,
    /// `(kind, row carries the expected persona)` of every audit row, in order.
    audit: Vec<(String, Option<bool>)>,
}

/// Drop what is unique per run (ids, tokens, absolute times) and the persona itself.
fn normalise(value: &Value, now_ms: i64) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| {
                    !matches!(
                        k.as_str(),
                        "id" | "session_id" | "preview_id" | "token" | "persona" | "duration_ms"
                    )
                })
                .map(|(k, v)| {
                    if k == "comment" {
                        // "<tag>:<first characters of the preview id>".
                        let tag = v.as_str().and_then(|c| c.split(':').next()).unwrap_or("");
                        (k.clone(), json!(tag))
                    } else if k.ends_with("_at_ms") {
                        // Absolute times differ per run; the distance from "now" must not.
                        (
                            format!("{k}_minus_now"),
                            json!(v.as_i64().unwrap_or(0) - now_ms),
                        )
                    } else {
                        (k.clone(), normalise(v, now_ms))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|v| normalise(v, now_ms)).collect()),
        other => other.clone(),
    }
}

async fn run_scenario(persona: &str) -> Outcome {
    let mock = MockBridge::start().await;
    let clock = ManualClock::starting_now();
    let sink = Arc::new(RecordingSink {
        inner: bridge_client(&mock, 300),
        commands: Mutex::new(Vec::new()),
    });
    let llm = llm_script(vec![
        ("propose_order", buy_gold()),
        ("propose_close", json!({ "all": true })),
        ("propose_order", buy_gold()),
        ("propose_order", buy_gold()),
        ("propose_order", buy_gold()),
        (
            "propose_order",
            json!({ "symbol": "gold", "side": "buy", "volume": 5 }),
        ),
    ]);
    let app = build_app(
        &mock,
        llm,
        Options {
            clock: Some(clock.clone()),
            sink: Some(sink.clone()),
            ..Options::default()
        },
    )
    .await;
    set_persona(&app, persona).await;

    let mut steps = Vec::new();
    let mut record = |name: &str, status: StatusCode, body: &Value| {
        steps.push(json!({
            "step": name,
            "status": status.as_u16(),
            "body": normalise(body, clock.now_ms()),
        }));
    };

    // 1. Propose a buy and confirm it.
    let (status, body) = chat(&app, "buy 0.1 gold").await;
    record("chat: propose buy", status, &body);
    assert_eq!(body["persona"], persona);
    let (status, done) = post(&app, &format!("/api/confirm/{}", token_of(&body)), None).await;
    record("confirm: buy", status, &done);

    // 2. A batch close needs a second confirmation.
    let (status, body) = chat(&app, "close all my positions").await;
    record("chat: propose close all", status, &body);
    let (status, second) = post(&app, &format!("/api/confirm/{}", token_of(&body)), None).await;
    record("confirm: first", status, &second);
    let second_token = second["token"].as_str().expect("second token").to_owned();
    let (status, done) = post(&app, &format!("/api/confirm/{second_token}"), None).await;
    record("confirm: second", status, &done);

    // 3. An expired token.
    let (status, body) = chat(&app, "buy 0.1 gold").await;
    record("chat: propose buy (will expire)", status, &body);
    clock.advance_ms(16_000);
    let (status, gone) = post(&app, &format!("/api/confirm/{}", token_of(&body)), None).await;
    record("confirm: expired", status, &gone);

    // 4. The kill switch goes on between preview and confirm, and refuses new proposals.
    let (status, body) = chat(&app, "buy 0.1 gold").await;
    record("chat: propose buy (kill switch next)", status, &body);
    let (status, ks) = post(
        &app,
        "/api/killswitch",
        Some(json!({ "engaged": true, "reason": "parity test" })),
    )
    .await;
    record("kill switch: engage", status, &ks);
    let (status, locked) = post(&app, &format!("/api/confirm/{}", token_of(&body)), None).await;
    record("confirm: kill switch", status, &locked);
    let (status, refused) = chat(&app, "buy 0.1 gold").await;
    record("chat: propose while engaged", status, &refused);
    let (status, ks) = post(&app, "/api/killswitch", Some(json!({ "engaged": false }))).await;
    record("kill switch: release", status, &ks);

    // 5. A limit rejection.
    let (status, over) = chat(&app, "buy 5 gold").await;
    record("chat: over max_lot", status, &over);
    assert!(over["preview"].is_null());

    let rows = app.audit.rows(None).await.unwrap();
    let audit = rows
        .iter()
        .map(|r| (r.kind.clone(), r.persona.as_ref().map(|p| p == persona)))
        .collect();
    // Every step of a conversation names the persona that was active (rule 10).
    for row in &rows {
        if row.session_id.is_some() {
            assert_eq!(
                row.persona.as_deref(),
                Some(persona),
                "row {} ({}) lacks the persona",
                row.id,
                row.kind
            );
        }
    }
    let writes = normalise(&json!(sink.commands.lock().unwrap().clone()), 0)
        .as_array()
        .cloned()
        .unwrap_or_default();
    Outcome {
        steps,
        writes,
        bridge_requests: mock.request_count(),
        executed_writes: mock.executed_writes(),
        positions_left: mock.state().positions.len(),
        audit,
    }
}

#[tokio::test]
async fn persona_parity_same_conversation_same_outcomes() {
    let jarvis = run_scenario("jarvis").await;
    let friday = run_scenario("friday").await;

    // Sanity: the scenario really exercised the paths it claims to.
    let statuses: Vec<u64> = jarvis
        .steps
        .iter()
        .map(|s| s["status"].as_u64().unwrap())
        .collect();
    assert_eq!(
        statuses,
        [
            200, 200, 200, 200, 200, 200, 410, 200, 200, 423, 200, 200, 200
        ]
    );
    assert_eq!(jarvis.executed_writes, 1 + 3, "one buy, then three closes");
    assert_eq!(jarvis.writes.len(), 4);
    assert_eq!(jarvis.positions_left, 0);
    let kinds: BTreeSet<&str> = jarvis.audit.iter().map(|(k, _)| k.as_str()).collect();
    for kind in [
        "utterance",
        "intent",
        "tool_call",
        "preview",
        "confirm",
        "confirm_second",
        "bridge_request",
        "bridge_reply",
        "trade_result",
        "gate_rejected",
        "kill_switch",
        "assistant_reply",
    ] {
        assert!(kinds.contains(kind), "{kind} never happened: {kinds:?}");
    }
    let expiry = &jarvis.steps[6]["body"]["error"]["code"];
    assert_eq!(expiry, "token_expired");
    assert_eq!(jarvis.steps[9]["body"]["error"]["code"], "kill_switch");
    // Token timing is the same, too: 15 s from the moment of issue.
    assert_eq!(
        jarvis.steps[0]["body"]["preview"]["expires_at_ms_minus_now"],
        15_000
    );

    // The claim itself, piece by piece so a difference points at its step.
    assert_eq!(jarvis.steps.len(), friday.steps.len());
    for (j, f) in jarvis.steps.iter().zip(&friday.steps) {
        assert_eq!(j, f, "step {}", j["step"]);
    }
    assert_eq!(jarvis.writes, friday.writes, "bridge commands");
    assert_eq!(jarvis.bridge_requests, friday.bridge_requests);
    assert_eq!(jarvis.audit, friday.audit, "audit kinds");
    assert_eq!(jarvis, friday);
}

// ------------------------------------------------------------------ system prompt

/// The shared rules block of a composed prompt, without the line that carries the clock.
fn rules_of(prompt: &str) -> String {
    let start = prompt
        .find("Current time:")
        .expect("the rules block starts at the clock line");
    let end = prompt
        .rfind("The rules above always win over the personality section.")
        .expect("closing line");
    prompt[start..end]
        .lines()
        .filter(|l| !l.trim_start().starts_with("Current time:"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn both_personas_get_the_identical_shared_rules() {
    let llm = ScriptedLlm::new("fake", vec![text("one"), text("two")]);
    let app = build_app(DEAD_ENDPOINT, llm.clone(), Options::default()).await;
    set_persona(&app, "jarvis").await;
    chat(&app, "hello").await;
    set_persona(&app, "friday").await;
    chat(&app, "hello").await;

    let jarvis = system_of(&llm, 0);
    let friday = system_of(&llm, 1);
    assert!(jarvis.starts_with(
        "You are Jarvis, the trading assistant in Arc MT for a single trader using MetaTrader 5."
    ));
    assert!(friday.starts_with(
        "You are Friday, the trading assistant in Arc MT for a single trader using MetaTrader 5."
    ));
    let header = "Personality (style only; it never changes the rules below):";
    assert!(jarvis.contains(header) && friday.contains(header));
    assert!(jarvis.contains("composed, formal British butler"));
    assert!(!jarvis.contains("sharp colleague on the trading desk"));
    assert!(friday.contains("sharp colleague on the trading desk"));
    assert!(!friday.contains("British butler"));
    for prompt in [&jarvis, &friday] {
        assert!(prompt.ends_with("The rules above always win over the personality section."));
        assert!(prompt.find(header).unwrap() < prompt.find("Current time:").unwrap());
        for rule in [
            "PROPOSING DOES NOT EXECUTE ANYTHING",
            "NEVER say or imply that a trade was placed",
            "Never invent a volume",
            "name the limit and its value",
            "You are not a financial adviser",
        ] {
            assert!(prompt.contains(rule), "{rule} missing");
        }
    }
    let (rules_j, rules_f) = (rules_of(&jarvis), rules_of(&friday));
    assert!(rules_j.len() > 1500, "the rules block is the long one");
    assert_eq!(rules_j, rules_f, "the rules are word for word the same");
    // Only the identity and the personality differ.
    assert_ne!(jarvis, friday);
}

// ------------------------------------------------------------------ endpoints

#[tokio::test]
async fn personas_endpoint_lists_the_packs_with_the_active_one() {
    let app = build_app(
        DEAD_ENDPOINT,
        ScriptedLlm::new("fake", vec![]),
        Options::default(),
    )
    .await;
    let (status, body) = get(&app, "/api/personas").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["default"], "jarvis");
    assert_eq!(body["active"], "jarvis");
    let list = body["personas"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    // The default first, as in docs/api.md.
    assert_eq!(list[0]["id"], "jarvis");
    assert_eq!(list[1]["id"], "friday");
    assert_eq!(
        list[0],
        json!({
            "id": "jarvis",
            "name": "Jarvis",
            "description": "Calm, formal and concise.",
            "wake_word": "hey jarvis",
            "voice_available": false,
            "theme": {
                "accent": "#38bdf8",
                "accent_soft": "#0c4a6e",
                "orb_core": "#7dd3fc",
                "orb_glow": "#0ea5e9"
            }
        }),
        "no voice engines in this app, so no persona has a voice"
    );
    assert_eq!(list[1]["theme"]["accent"], "#f5b942");
    // The system prompt and voice model path never leave the server.
    assert!(list[0].get("system_prompt").is_none());
    assert!(list[0].get("voice_model").is_none());

    set_persona(&app, "friday").await;
    let (_, body) = get(&app, "/api/personas").await;
    assert_eq!(body["active"], "friday");
    assert_eq!(body["default"], "jarvis");
}

#[tokio::test]
async fn status_reports_the_active_persona_and_effective_limits() {
    let mock = MockBridge::start().await;
    let app = build_app(&mock, ScriptedLlm::new("fake", vec![]), Options::default()).await;
    let (_, status) = get(&app, "/api/status").await;
    assert_eq!(
        status["persona"],
        json!({ "id": "jarvis", "name": "Jarvis" })
    );
    set_persona(&app, "friday").await;
    let (_, status) = get(&app, "/api/status").await;
    assert_eq!(
        status["persona"],
        json!({ "id": "friday", "name": "Friday" })
    );
    assert_eq!(status["limits"]["max_lot"], 1.0);
}

#[tokio::test]
async fn persona_switch_via_settings_applies_from_the_next_turn_and_keeps_history() {
    let mock = MockBridge::start().await;
    let llm = ScriptedLlm::new("fake", vec![text("one"), text("two"), text("three")]);
    let app = build_app(&mock, llm.clone(), Options::default()).await;
    let mut events = app.state.events().subscribe();

    let (status, body) = chat(&app, "hello").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["persona"], "jarvis");
    let session = body["session_id"].as_str().unwrap().to_owned();
    let sid: uuid::Uuid = session.parse().unwrap();
    assert_eq!(
        app.state.agent().session_persona(sid).as_deref(),
        Some("jarvis")
    );

    set_persona(&app, "friday").await;
    assert_eq!(events.try_recv().unwrap(), Event::SettingsChanged);

    let (status, body) = post(
        &app,
        "/api/chat",
        Some(json!({ "message": "and now?", "session_id": session })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["persona"], "friday");
    assert_eq!(body["session_id"], session.as_str());
    assert_eq!(
        app.state.agent().session_persona(sid).as_deref(),
        Some("friday")
    );

    // The session kept its history across the switch; only the system prompt changed.
    let requests = llm.requests.lock().unwrap().clone();
    assert_eq!(requests[1].messages.len(), 3);
    assert!(matches!(&requests[1].messages[0], Message::User { text } if text == "hello"));
    assert!(
        requests[0]
            .system
            .as_deref()
            .unwrap()
            .starts_with("You are Jarvis,")
    );
    assert!(
        requests[1]
            .system
            .as_deref()
            .unwrap()
            .starts_with("You are Friday,")
    );

    // The audit rows of each turn name the persona that spoke.
    let rows = app.audit.rows(Some(sid)).await.unwrap();
    let replies: Vec<_> = rows
        .iter()
        .filter(|r| r.kind == "assistant_reply")
        .map(|r| r.persona.clone().unwrap())
        .collect();
    assert_eq!(replies, ["jarvis", "friday"]);
}

// ------------------------------------------------------------------ settings

#[tokio::test]
async fn settings_have_documented_defaults_and_update_partially() {
    let app = build_app(
        DEAD_ENDPOINT,
        ScriptedLlm::new("fake", vec![]),
        Options::default(),
    )
    .await;
    let (status, body) = get(&app, "/api/settings").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({
            "persona": "jarvis",
            "llm_provider": null,
            "voice": { "read_aloud": true, "input_mode": "hold", "language": "en" },
            "watchlist": ["XAUUSD", "US100", "EURUSD"],
            "aliases": {}
        })
    );

    let (status, body) = put(
        &app,
        "/api/settings",
        json!({ "voice": { "input_mode": "wake_word" }, "watchlist": ["gold", "eur/usd", "gold"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["voice"],
        json!({ "read_aloud": true, "input_mode": "wake_word", "language": "en" })
    );
    assert_eq!(body["watchlist"], json!(["XAUUSD", "EURUSD"]));
    assert_eq!(body["persona"], "jarvis");

    let (_, again) = put(
        &app,
        "/api/settings",
        json!({ "voice": { "read_aloud": false } }),
    )
    .await;
    assert_eq!(again["voice"]["read_aloud"], false);
    assert_eq!(
        again["voice"]["input_mode"], "wake_word",
        "unsent fields stay"
    );
    assert_eq!(again["watchlist"], json!(["XAUUSD", "EURUSD"]));
    let (_, stored) = get(&app, "/api/settings").await;
    assert_eq!(stored, again);

    // An empty update is fine and changes nothing.
    let (status, same) = put(&app, "/api/settings", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(same, again);
}

#[tokio::test]
async fn settings_validation_is_a_400_with_the_standard_error_shape() {
    let app = build_app(
        DEAD_ENDPOINT,
        ScriptedLlm::new("fake", vec![]),
        Options::default(),
    )
    .await;
    let many: Vec<String> = (0..31).map(|i| format!("SYM{i}")).collect();
    let many_aliases: serde_json::Map<String, Value> = (0..101)
        .map(|i| (format!("alias {i}"), json!("XAUUSD")))
        .collect();
    let cases = [
        ("unknown field", json!({ "theme": "dark" })),
        ("unknown voice field", json!({ "voice": { "volume": 3 } })),
        ("unknown persona", json!({ "persona": "ultron" })),
        ("unknown provider", json!({ "llm_provider": "gpt-nine" })),
        (
            "bad input mode",
            json!({ "voice": { "input_mode": "shout" } }),
        ),
        ("bad language", json!({ "voice": { "language": "fr" } })),
        ("too many symbols", json!({ "watchlist": many })),
        ("unusable symbol", json!({ "watchlist": ["///"] })),
        ("too many aliases", json!({ "aliases": many_aliases })),
        (
            "bad alias target",
            json!({ "aliases": { "oil": "US OIL" } }),
        ),
        ("empty alias", json!({ "aliases": { " ": "XAUUSD" } })),
        ("wrong type", json!({ "persona": 7 })),
    ];
    for (what, body) in cases {
        let (status, response) = put(&app, "/api/settings", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{what}: {response}");
        assert_eq!(response["error"]["code"], "bad_request", "{what}");
        assert!(response["error"]["message"].is_string());
    }
    let (status, _, _) = raw_request(
        &app.router,
        "PUT",
        "/api/settings",
        &[],
        Some("{not json".to_owned()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Nothing was stored by any of those.
    let (_, stored) = get(&app, "/api/settings").await;
    assert_eq!(stored["persona"], "jarvis");
    assert_eq!(stored["aliases"], json!({}));
}

#[tokio::test]
async fn a_persona_that_disappeared_falls_back_to_the_default() {
    let app = build_app(
        DEAD_ENDPOINT,
        ScriptedLlm::new("fake", vec![]),
        Options::default(),
    )
    .await;
    let mut stored = app.state.user_settings(None).await;
    stored.persona = "ghost".to_owned();
    stored.llm_provider = Some("gone".to_owned());
    app.state
        .settings_store()
        .put("local", &stored)
        .await
        .unwrap();
    let (_, body) = get(&app, "/api/settings").await;
    assert_eq!(body["persona"], "jarvis");
    assert_eq!(body["llm_provider"], Value::Null);
}

#[tokio::test]
async fn the_llm_provider_preference_is_used_when_the_request_names_none() {
    let mock = MockBridge::start().await;
    let fake = ScriptedLlm::new("fake", vec![text("from the default"), text("from fake")]);
    let claude = ScriptedLlm::new(
        "claude",
        vec![text("from claude"), text("from claude again")],
    );
    let app = build_app(
        &mock,
        fake.clone(),
        Options {
            claude_llm: Some(claude.clone()),
            ..Options::default()
        },
    )
    .await;

    let (_, body) = chat(&app, "hi").await;
    assert_eq!(body["provider"], "fake");

    let (status, body) = put(&app, "/api/settings", json!({ "llm_provider": "claude" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["llm_provider"], "claude");
    let (_, body) = chat(&app, "hi").await;
    assert_eq!(body["provider"], "claude");
    assert_eq!(body["reply"], "from claude");

    // An explicit provider in the request still wins.
    let (_, body) = post(
        &app,
        "/api/chat",
        Some(json!({ "message": "hi", "provider": "fake" })),
    )
    .await;
    assert_eq!(body["provider"], "fake", "{body}");
    assert_eq!(claude.request_count(), 1);

    // null puts it back to the server default.
    let (_, body) = put(&app, "/api/settings", json!({ "llm_provider": null })).await;
    assert_eq!(body["llm_provider"], Value::Null);
    assert_eq!(fake.request_count(), 2);
}

#[tokio::test]
async fn an_unusable_preferred_provider_falls_back_to_the_default() {
    let app = build_app(
        DEAD_ENDPOINT,
        ScriptedLlm::new("fake", vec![text("ok")]),
        Options::default(),
    )
    .await;
    // `claude` is configured but its key is missing in this app: allowed as a preference...
    let (status, _) = put(&app, "/api/settings", json!({ "llm_provider": "claude" })).await;
    assert_eq!(status, StatusCode::OK);
    // ...but chat keeps working on the default.
    let (status, body) = chat(&app, "hi").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["provider"], "fake");
}

#[tokio::test]
async fn user_aliases_resolve_in_the_tools_of_that_users_turns() {
    let mock = MockBridge::start().await;
    let llm = llm_script(vec![
        ("get_price", json!({ "symbol": "yellow metal" })),
        ("get_price", json!({ "symbol": "yellow metal" })),
        (
            "propose_order",
            json!({ "symbol": "yellow metal", "side": "buy", "volume": 0.1 }),
        ),
        ("get_price", json!({ "symbol": "gold" })),
    ]);
    let app = build_app(&mock, llm.clone(), Options::default()).await;

    // Before: nobody knows "yellow metal".
    let (_, body) = chat(&app, "what is yellow metal at").await;
    assert_eq!(body["tool_calls"][0]["ok"], false, "{body}");

    let (status, settings) = put(
        &app,
        "/api/settings",
        json!({ "aliases": { "Yellow  Metal": "xauusd" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["aliases"], json!({ "yellow metal": "xauusd" }));

    let (_, body) = chat(&app, "what is yellow metal at").await;
    assert_eq!(body["tool_calls"][0]["ok"], true, "{body}");
    let rows = app.audit.rows(None).await.unwrap();
    let asked: Vec<&Value> = rows
        .iter()
        .filter(|r| r.kind == "bridge_request")
        .map(|r| &r.payload["args"]["symbol"])
        .collect();
    assert_eq!(asked.last().unwrap().as_str(), Some("XAUUSD"));

    // The propose tools and the gate see the same resolved symbol.
    let (_, body) = chat(&app, "buy 0.1 yellow metal").await;
    assert_eq!(body["preview"]["symbols"], json!(["XAUUSD"]), "{body}");

    // The model is told the user's aliases, and a user entry wins over the configured one.
    assert!(system_of(&llm, 2).contains("yellow metal = XAUUSD"));
    put(
        &app,
        "/api/settings",
        json!({ "aliases": { "gold": "US30" } }),
    )
    .await;
    let (_, body) = chat(&app, "price of gold").await;
    assert_eq!(body["tool_calls"][0]["ok"], true, "{body}");
    let rows = app.audit.rows(None).await.unwrap();
    let last = rows
        .iter()
        .rev()
        .find(|r| r.kind == "bridge_request")
        .unwrap();
    assert_eq!(last.payload["args"]["symbol"], "US30");
}

#[tokio::test]
async fn the_users_watchlist_drives_the_watchlist_endpoint() {
    let mock = MockBridge::start().await;
    let app = build_app(&mock, ScriptedLlm::new("fake", vec![]), Options::default()).await;
    let (_, body) = get(&app, "/api/watchlist").await;
    assert_eq!(body["prices"].as_array().unwrap().len(), 3);
    put(
        &app,
        "/api/settings",
        json!({ "aliases": { "yellow metal": "XAUUSD" }, "watchlist": ["yellow metal", "eurusd"] }),
    )
    .await;
    let (status, body) = get(&app, "/api/watchlist").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let symbols: Vec<&str> = body["prices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["symbol"].as_str().unwrap())
        .collect();
    assert_eq!(symbols, ["XAUUSD", "EURUSD"]);
}

// ------------------------------------------------------------------ audit

#[tokio::test]
async fn confirm_rows_carry_the_persona_that_created_the_preview() {
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock,
        llm_script(vec![("propose_order", buy_gold())]),
        Options::default(),
    )
    .await;
    set_persona(&app, "jarvis").await;
    let (_, body) = chat(&app, "buy 0.1 gold").await;
    set_persona(&app, "friday").await;
    let (status, done) = post(&app, &format!("/api/confirm/{}", token_of(&body)), None).await;
    assert_eq!(status, StatusCode::OK, "{done}");

    let rows = app.audit.rows(None).await.unwrap();
    for kind in [
        "preview",
        "confirm",
        "bridge_request",
        "bridge_reply",
        "trade_result",
    ] {
        let row = rows
            .iter()
            .find(|r| r.kind == kind)
            .unwrap_or_else(|| panic!("{kind}"));
        assert_eq!(row.persona.as_deref(), Some("jarvis"), "{kind}");
    }
}

#[tokio::test]
async fn ui_buttons_record_the_users_active_persona() {
    let mock = MockBridge::start().await;
    let app = build_app(&mock, ScriptedLlm::new("fake", vec![]), Options::default()).await;
    set_persona(&app, "friday").await;
    let (status, preview) = post(&app, "/api/positions/1001001/close", None).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let (status, _) = post(
        &app,
        &format!("/api/confirm/{}", preview["token"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows = app.audit.rows(None).await.unwrap();
    let preview_row = rows.iter().find(|r| r.kind == "preview").unwrap();
    assert_eq!(preview_row.persona.as_deref(), Some("friday"));
    let confirm_row = rows.iter().find(|r| r.kind == "confirm").unwrap();
    assert_eq!(confirm_row.persona.as_deref(), Some("friday"));
    // The audit endpoint shows the column.
    let (_, page) = get(&app, "/api/audit?limit=50").await;
    assert!(
        page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "preview" && e["persona"] == "friday")
    );
}

// ------------------------------------------------------------------ voice

struct FixedStt(Mutex<String>);

impl FixedStt {
    fn new(text: &str) -> Arc<Self> {
        Arc::new(Self(Mutex::new(text.to_owned())))
    }
    fn say(&self, text: &str) {
        *self.0.lock().unwrap() = text.to_owned();
    }
}

#[async_trait]
impl SpeechToText for FixedStt {
    async fn transcribe(&self, samples: Vec<f32>) -> Result<Transcript, VoiceError> {
        let text = self.0.lock().unwrap().clone();
        Ok(Transcript {
            raw: text.clone(),
            text,
            audio_ms: (samples.len() / 16) as u64,
            elapsed_ms: 1,
        })
    }
}

/// Speaks "<label>:<text>" so a test can tell the voices apart.
struct LabelTts(&'static str);

#[async_trait]
impl TextToSpeech for LabelTts {
    async fn synthesize(&self, text: &str) -> Result<Vec<u8>, VoiceError> {
        Ok(format!("RIFFxxxxWAVE{}:{text}", self.0).into_bytes())
    }
}

fn silence_wav(seconds: f32) -> Vec<u8> {
    let samples = (16_000.0 * seconds) as u32;
    let data_len = samples * 2;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&16_000u32.to_le_bytes());
    out.extend_from_slice(&32_000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.resize(out.len() + data_len as usize, 0);
    out
}

async fn transcribe(app: &TestApp, query: &str, cookie: Option<&str>) -> (StatusCode, Value) {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let mut req = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/api/voice/transcribe{query}"))
        .header("content-type", "audio/wav");
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    let response = app
        .router
        .clone()
        .oneshot(req.body(Body::from(silence_wav(0.5))).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn speak(app: &TestApp, body: Value) -> (StatusCode, String) {
    let (status, _, bytes) = raw_request(
        &app.router,
        "POST",
        "/api/voice/speak",
        &[],
        Some(body.to_string()),
    )
    .await;
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn a_wake_phrase_switches_the_persona_and_returns_the_rest_as_the_command() {
    let stt = FixedStt::new("Hey, Friday! what's gold at?");
    let voice = Voice::new(Some(stt.clone()), Some(Arc::new(LabelTts("D"))), 30.0);
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock,
        ScriptedLlm::new("fake", vec![]),
        Options {
            voice: Some(voice),
            ..Options::default()
        },
    )
    .await;
    let mut events = app.state.events().subscribe();

    let (status, body) = transcribe(&app, "?wake=1", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["text"], "Hey, Friday! what's gold at?");
    assert_eq!(
        body["wake"],
        json!({ "detected": true, "persona": "friday", "command": "what's gold at?" })
    );
    let (_, settings) = get(&app, "/api/settings").await;
    assert_eq!(
        settings["persona"], "friday",
        "the wake phrase switched the persona"
    );
    assert_eq!(events.try_recv().unwrap(), Event::SettingsChanged);
    let (_, status_body) = get(&app, "/api/status").await;
    assert_eq!(status_body["persona"]["id"], "friday");

    // Calling the other one switches back.
    stt.say("okay jarvis");
    let (_, body) = transcribe(&app, "?wake=1", None).await;
    assert_eq!(
        body["wake"],
        json!({ "detected": true, "persona": "jarvis", "command": "" })
    );
    let (_, settings) = get(&app, "/api/settings").await;
    assert_eq!(settings["persona"], "jarvis");

    // No wake phrase: not detected, nothing changes, the UI ignores the segment.
    stt.say("so anyway hey friday is what I call her");
    let (_, body) = transcribe(&app, "?wake=1", None).await;
    assert_eq!(
        body["wake"],
        json!({ "detected": false, "persona": null, "command": "" })
    );
    let (_, settings) = get(&app, "/api/settings").await;
    assert_eq!(settings["persona"], "jarvis");

    // Wake mode sees through to the command: "hey friday confirm" is a confirm.
    stt.say("hey friday, confirm");
    let (_, body) = transcribe(&app, "?wake=1", None).await;
    assert_eq!(body["intent"], "confirm");
    assert_eq!(body["wake"]["command"], "confirm");
}

#[tokio::test]
async fn a_wake_phrase_does_not_switch_persona_while_a_confirmation_is_open() {
    let stt = FixedStt::new("hey friday, cancel");
    let voice = Voice::new(Some(stt.clone()), Some(Arc::new(LabelTts("D"))), 30.0);
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock,
        ScriptedLlm::new("fake", vec![]),
        Options {
            voice: Some(voice),
            ..Options::default()
        },
    )
    .await;

    // A confirmation is open (a close preview from the UI), with Jarvis active.
    let (status, preview) = post(&app, "/api/positions/1001001/close", None).await;
    assert_eq!(status, StatusCode::OK, "{preview}");

    // Friday's wake phrase is still detected and its command still read...
    let (status, body) = transcribe(&app, "?wake=1", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["wake"]["detected"], true);
    assert_eq!(body["wake"]["command"], "cancel");
    assert_eq!(body["intent"], "cancel");
    // ...but the persona stays the one the confirmation started with.
    let (_, settings) = get(&app, "/api/settings").await;
    assert_eq!(settings["persona"], "jarvis");

    // Once the confirmation is resolved, the same phrase switches persona again.
    let token = preview["token"].as_str().expect("token").to_owned();
    let (status, _) = post(&app, &format!("/api/cancel/{token}"), None).await;
    assert!(status.is_success(), "{status}");
    stt.say("hey friday");
    let (_, _) = transcribe(&app, "?wake=1", None).await;
    let (_, settings) = get(&app, "/api/settings").await;
    assert_eq!(settings["persona"], "friday");
}

#[tokio::test]
async fn without_wake_the_transcribe_response_is_unchanged() {
    let stt = FixedStt::new("hey friday close all");
    let voice = Voice::new(Some(stt), Some(Arc::new(LabelTts("D"))), 30.0);
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock,
        ScriptedLlm::new("fake", vec![]),
        Options {
            voice: Some(voice),
            ..Options::default()
        },
    )
    .await;
    for query in ["", "?wake=0", "?wake=false"] {
        let (status, body) = transcribe(&app, query, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.get("wake").is_none(), "{query}: {body}");
        assert_eq!(body["text"], "hey friday close all");
        let keys: BTreeSet<&str> = body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from(["text", "raw", "audio_ms", "elapsed_ms", "intent"])
        );
    }
    let (_, settings) = get(&app, "/api/settings").await;
    assert_eq!(settings["persona"], "jarvis", "no wake flag, no switch");
}

#[tokio::test]
async fn each_persona_speaks_with_its_own_voice() {
    let voice = Voice::new(None, Some(Arc::new(LabelTts("D"))), 30.0)
        .with_persona_tts("friday", Arc::new(LabelTts("F")));
    let app = build_app(
        DEAD_ENDPOINT,
        ScriptedLlm::new("fake", vec![]),
        Options {
            voice: Some(voice),
            ..Options::default()
        },
    )
    .await;

    let (_, personas) = get(&app, "/api/personas").await;
    let by_id = |id: &str| {
        personas["personas"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(by_id("friday")["voice_available"], true);
    assert_eq!(
        by_id("jarvis")["voice_available"],
        false,
        "no model of its own"
    );

    // The active persona (Jarvis, no voice of its own) falls back to the shared voice.
    let (status, wav) = speak(&app, json!({ "text": "hello" })).await;
    assert_eq!(status, StatusCode::OK);
    assert!(wav.ends_with("D:hello"), "{wav}");
    // Previewing another persona's voice.
    let (status, wav) = speak(&app, json!({ "text": "hello", "persona": "friday" })).await;
    assert_eq!(status, StatusCode::OK);
    assert!(wav.ends_with("F:hello"), "{wav}");
    // After switching, the active persona's voice is used.
    set_persona(&app, "friday").await;
    let (_, wav) = speak(&app, json!({ "text": "hello" })).await;
    assert!(wav.ends_with("F:hello"), "{wav}");
    let (status, wav) = speak(&app, json!({ "text": "hello", "persona": "ultron" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{wav}");

    let (_, status_body) = get(&app, "/api/status").await;
    assert_eq!(status_body["voice"], json!({ "stt": false, "tts": true }));
    let rows = app.audit.rows(None).await.unwrap();
    let tts_rows: Vec<_> = rows.iter().filter(|r| r.kind == "voice_tts").collect();
    assert_eq!(tts_rows.len(), 3);
    assert_eq!(tts_rows[1].persona.as_deref(), Some("friday"));
    assert_eq!(tts_rows[0].persona.as_deref(), Some("jarvis"));
}

// ------------------------------------------------------------------ isolation (login on)

struct Accounts {
    app: TestApp,
    alice: TestUser,
    bob: TestUser,
    _mock: MockBridge,
}

async fn accounts(llm: Arc<ScriptedLlm>) -> Accounts {
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock,
        llm,
        Options {
            auth: true,
            ..Options::default()
        },
    )
    .await;
    let alice = create_user(&app, "alice", PASSWORD).await;
    let bob = create_user(&app, "bob", PASSWORD).await;
    Accounts {
        app,
        alice,
        bob,
        _mock: mock,
    }
}

#[tokio::test]
async fn settings_are_private_to_each_user() {
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            tool_use("a1", "get_price", json!({ "symbol": "yellow metal" })),
            text("alice done"),
            tool_use("b1", "get_price", json!({ "symbol": "yellow metal" })),
            text("bob done"),
        ],
    );
    let Accounts {
        app,
        alice,
        bob,
        _mock,
    } = accounts(llm.clone()).await;
    let alice_cookie = login(&app, &alice).await;
    let bob_cookie = login(&app, &bob).await;

    let (status, body) = request_as(
        &app,
        &alice_cookie,
        "PUT",
        "/api/settings",
        Some(json!({
            "persona": "friday",
            "aliases": { "yellow metal": "XAUUSD" },
            "watchlist": ["XAUUSD"],
            "voice": { "read_aloud": false }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Bob still has the defaults.
    let (_, bobs) = request_as(&app, &bob_cookie, "GET", "/api/settings", None).await;
    assert_eq!(bobs["persona"], "jarvis");
    assert_eq!(bobs["aliases"], json!({}));
    assert_eq!(bobs["voice"]["read_aloud"], true);
    assert_eq!(bobs["watchlist"], json!(["XAUUSD", "US100", "EURUSD"]));
    let (_, alices) = request_as(&app, &alice_cookie, "GET", "/api/settings", None).await;
    assert_eq!(alices["persona"], "friday");

    let (_, p) = request_as(&app, &bob_cookie, "GET", "/api/personas", None).await;
    assert_eq!(p["active"], "jarvis");
    let (_, p) = request_as(&app, &alice_cookie, "GET", "/api/personas", None).await;
    assert_eq!(p["active"], "friday");
    let (_, s) = request_as(&app, &bob_cookie, "GET", "/api/status", None).await;
    assert_eq!(s["persona"]["id"], "jarvis");

    // Alice's turn resolves her alias and is answered by Friday; Bob's does neither.
    let (status, a) = request_as(
        &app,
        &alice_cookie,
        "POST",
        "/api/chat",
        Some(json!({ "message": "yellow metal?" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{a}");
    assert_eq!(a["persona"], "friday");
    assert_eq!(a["tool_calls"][0]["ok"], true, "{a}");
    let (status, b) = request_as(
        &app,
        &bob_cookie,
        "POST",
        "/api/chat",
        Some(json!({ "message": "yellow metal?" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{b}");
    assert_eq!(b["persona"], "jarvis");
    assert_eq!(
        b["tool_calls"][0]["ok"], false,
        "bob has no such alias: {b}"
    );
    let requests = llm.requests.lock().unwrap().clone();
    assert!(
        requests[0]
            .system
            .as_deref()
            .unwrap()
            .starts_with("You are Friday,")
    );
    assert!(
        requests[2]
            .system
            .as_deref()
            .unwrap()
            .starts_with("You are Jarvis,")
    );
    assert!(
        requests[0]
            .system
            .as_deref()
            .unwrap()
            .contains("yellow metal = XAUUSD")
    );
    assert!(
        !requests[2]
            .system
            .as_deref()
            .unwrap()
            .contains("yellow metal")
    );

    // Bob changing his own settings leaves Alice's alone.
    request_as(
        &app,
        &bob_cookie,
        "PUT",
        "/api/settings",
        Some(json!({ "persona": "friday" })),
    )
    .await;
    request_as(
        &app,
        &bob_cookie,
        "PUT",
        "/api/settings",
        Some(json!({ "persona": "jarvis", "aliases": { "x": "US30" } })),
    )
    .await;
    let (_, alices) = request_as(&app, &alice_cookie, "GET", "/api/settings", None).await;
    assert_eq!(alices["persona"], "friday");
    assert_eq!(alices["aliases"], json!({ "yellow metal": "XAUUSD" }));
}

#[tokio::test]
async fn a_wake_phrase_switches_only_the_speakers_persona() {
    let stt = FixedStt::new("hey friday close all");
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock,
        ScriptedLlm::new("fake", vec![]),
        Options {
            auth: true,
            voice: Some(Voice::new(Some(stt), Some(Arc::new(LabelTts("D"))), 30.0)),
            ..Options::default()
        },
    )
    .await;
    let alice = create_user(&app, "alice", PASSWORD).await;
    let bob = create_user(&app, "bob", PASSWORD).await;
    let (ac, bc) = (login(&app, &alice).await, login(&app, &bob).await);
    let (status, body) = transcribe(&app, "?wake=1", Some(&ac)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["wake"]["persona"], "friday");
    let (_, a) = request_as(&app, &ac, "GET", "/api/settings", None).await;
    let (_, b) = request_as(&app, &bc, "GET", "/api/settings", None).await;
    assert_eq!(
        (a["persona"].as_str(), b["persona"].as_str()),
        (Some("friday"), Some("jarvis"))
    );
}

// ------------------------------------------------------------------ tighten-only limits

#[tokio::test]
async fn limits_start_at_the_config_and_can_be_tightened_and_reset() {
    let mock = MockBridge::start().await;
    let llm = llm_script(vec![
        (
            "propose_order",
            json!({ "symbol": "gold", "side": "buy", "volume": 0.8 }),
        ),
        (
            "propose_order",
            json!({ "symbol": "gold", "side": "buy", "volume": 0.4 }),
        ),
        (
            "propose_order",
            json!({ "symbol": "gold", "side": "buy", "volume": 0.8 }),
        ),
    ]);
    let app = build_app(&mock, llm, Options::default()).await;
    let mut events = app.state.events().subscribe();

    let (status, view) = get(&app, "/api/settings/limits").await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["overrides"], Value::Null);
    assert_eq!(view["config"], view["effective"]);
    assert_eq!(view["config"]["max_lot"], 1.0);
    assert_eq!(view["config"]["max_positions"], 5);
    assert_eq!(
        view["config"]["allowed_symbols"],
        json!(["XAUUSD", "US100", "US30", "EURUSD", "GBPUSD"])
    );

    // Tighten: stricter and equal values are accepted.
    let (status, view) = put(
        &app,
        "/api/settings/limits",
        json!({
            "max_lot": 0.5,
            "max_positions": 5,
            "max_daily_loss": 100.0,
            "allowed_symbols": ["gold", "eurusd"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["overrides"]["max_lot"], 0.5);
    assert_eq!(view["effective"]["max_lot"], 0.5);
    assert_eq!(view["effective"]["max_daily_loss"], 100.0);
    assert_eq!(view["effective"]["max_positions"], 5);
    assert_eq!(
        view["effective"]["allowed_symbols"],
        json!(["XAUUSD", "EURUSD"])
    );
    assert_eq!(
        view["config"]["max_lot"], 1.0,
        "the config view never changes"
    );
    assert_eq!(events.try_recv().unwrap(), Event::SettingsChanged);
    let (_, status_body) = get(&app, "/api/status").await;
    assert_eq!(
        status_body["limits"]["max_lot"], 0.5,
        "status reports the effective limits"
    );

    // The gate enforces them: 0.8 lots is now over the limit, 0.4 passes.
    let (_, body) = chat(&app, "buy 0.8 gold").await;
    assert!(body["preview"].is_null(), "{body}");
    assert_eq!(body["tool_calls"][0]["ok"], false);
    let rows = app.audit.rows(None).await.unwrap();
    assert!(
        rows.iter()
            .any(|r| r.kind == "gate_rejected" && r.payload["reason"] == "limit:max_lot")
    );
    let (_, body) = chat(&app, "buy 0.4 gold").await;
    assert!(body["preview"].is_object(), "{body}");

    // Reset: back to the config, and the gate follows.
    let (status, view) = delete(&app, "/api/settings/limits", None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["overrides"], Value::Null);
    assert_eq!(view["effective"], view["config"]);
    let (_, body) = chat(&app, "buy 0.8 gold").await;
    assert!(
        body["preview"].is_object(),
        "back at the configured 1.0: {body}"
    );

    let rows = app.audit.rows(None).await.unwrap();
    let changes: Vec<_> = rows.iter().filter(|r| r.kind == "limits_changed").collect();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].payload["action"], "set");
    assert_eq!(changes[0].payload["old"]["overrides"], Value::Null);
    assert_eq!(changes[0].payload["old"]["effective"]["max_lot"], 1.0);
    assert_eq!(changes[0].payload["new"]["overrides"]["max_lot"], 0.5);
    assert_eq!(changes[0].payload["new"]["effective"]["max_lot"], 0.5);
    assert_eq!(changes[1].payload["action"], "reset");
    assert_eq!(changes[1].payload["old"]["overrides"]["max_lot"], 0.5);
    assert_eq!(changes[1].payload["new"]["overrides"], Value::Null);
    assert_eq!(changes[1].payload["new"]["effective"]["max_lot"], 1.0);
    assert_eq!(changes[0].payload["by"], "local");
}

#[tokio::test]
async fn looser_limits_are_422_naming_the_field() {
    let mock = MockBridge::start().await;
    let app = build_app(&mock, ScriptedLlm::new("fake", vec![]), Options::default()).await;
    let looser = [
        ("max_lot", json!({ "max_lot": 1.5 })),
        ("max_positions", json!({ "max_positions": 6 })),
        ("max_daily_loss", json!({ "max_daily_loss": 501.0 })),
        (
            "allowed_symbols",
            json!({ "allowed_symbols": ["XAUUSD", "BTCUSD"] }),
        ),
    ];
    for (field, body) in looser {
        let (status, response) = put(&app, "/api/settings/limits", body).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{field}: {response}"
        );
        assert_eq!(response["error"]["code"], "limit_not_stricter", "{field}");
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains(field),
            "{field}: {response}"
        );
    }
    // A mix with one looser value is refused as a whole.
    let (status, response) = put(
        &app,
        "/api/settings/limits",
        json!({ "max_lot": 0.1, "max_positions": 50 }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");
    let (_, view) = get(&app, "/api/settings/limits").await;
    assert_eq!(view["overrides"], Value::Null, "nothing was stored");
    assert_eq!(view["effective"], view["config"]);
    let rows = app.audit.rows(None).await.unwrap();
    assert!(rows.iter().all(|r| r.kind != "limits_changed"));
}

#[tokio::test]
async fn malformed_limits_are_400() {
    let mock = MockBridge::start().await;
    let app = build_app(&mock, ScriptedLlm::new("fake", vec![]), Options::default()).await;
    for body in [
        json!({}),
        json!({ "totp": "123456" }),
        json!({ "max_lot": 0 }),
        json!({ "max_lot": -1 }),
        json!({ "max_lot": "half" }),
        json!({ "max_positions": 0 }),
        json!({ "max_positions": 2.5 }),
        json!({ "max_daily_loss": 0 }),
        json!({ "allowed_symbols": ["///"] }),
        json!({ "trading_hours": { "days": [], "start": "09:00", "end": "17:00" } }),
        json!({ "trading_hours": { "days": ["mon"], "start": "9:00", "end": "17:00" } }),
        json!({ "trading_hours": { "days": ["funday"], "start": "09:00", "end": "17:00" } }),
        json!({ "max_lots": 1 }),
    ] {
        let (status, response) = put(&app, "/api/settings/limits", body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {response}");
        assert_eq!(response["error"]["code"], "bad_request", "{body}");
    }
}

#[tokio::test]
async fn trading_hours_can_be_narrowed_inside_the_configured_window() {
    let mock = MockBridge::start().await;
    let limits = r#"
max_lot = 1.0
max_positions = 5
max_daily_loss = 500.0
allowed_symbols = ["XAUUSD", "US100", "EURUSD"]

[limits.trading_hours]
days = ["mon", "tue", "wed", "thu", "fri"]
start = "00:05"
end = "23:55"
"#;
    let app = build_app(
        &mock,
        ScriptedLlm::new("fake", vec![]),
        Options {
            limits: limits.trim_start_matches('\n'),
            ..Options::default()
        },
    )
    .await;
    let (status, view) = put(
        &app,
        "/api/settings/limits",
        json!({ "trading_hours": { "days": ["tue", "wed"], "start": "08:00", "end": "17:00" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(
        view["effective"]["trading_hours"],
        json!({ "days": ["tue", "wed"], "start": "08:00", "end": "17:00" })
    );
    assert_eq!(view["config"]["trading_hours"]["start"], "00:05");
    for (what, hours) in [
        (
            "a day the config lacks",
            json!({ "days": ["sat"], "start": "08:00", "end": "09:00" }),
        ),
        (
            "a window that starts earlier",
            json!({ "days": ["mon"], "start": "00:00", "end": "09:00" }),
        ),
        (
            "a window that ends later",
            json!({ "days": ["mon"], "start": "08:00", "end": "23:59" }),
        ),
        (
            "a window across midnight",
            json!({ "days": ["mon"], "start": "22:00", "end": "02:00" }),
        ),
    ] {
        let (status, response) = put(
            &app,
            "/api/settings/limits",
            json!({ "trading_hours": hours }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{what}: {response}"
        );
        assert_eq!(response["error"]["code"], "limit_not_stricter");
    }
}

#[tokio::test]
async fn later_puts_merge_into_the_stored_overrides() {
    let mock = MockBridge::start().await;
    let app = build_app(&mock, ScriptedLlm::new("fake", vec![]), Options::default()).await;
    put(&app, "/api/settings/limits", json!({ "max_lot": 0.5 })).await;
    let (status, view) = put(&app, "/api/settings/limits", json!({ "max_positions": 2 })).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(
        view["overrides"],
        json!({ "max_lot": 0.5, "max_positions": 2 })
    );
    assert_eq!(view["effective"]["max_lot"], 0.5);
    assert_eq!(view["effective"]["max_positions"], 2);
}

#[tokio::test]
async fn a_preview_in_flight_is_rechecked_against_the_tightened_limits_at_confirm() {
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock,
        llm_script(vec![(
            "propose_order",
            json!({ "symbol": "gold", "side": "buy", "volume": 0.4 }),
        )]),
        Options::default(),
    )
    .await;
    let (_, body) = chat(&app, "buy 0.4 gold").await;
    let token = token_of(&body);
    // The user tightens the limit while the preview is waiting.
    let (status, _) = put(&app, "/api/settings/limits", json!({ "max_lot": 0.3 })).await;
    assert_eq!(status, StatusCode::OK);
    let (status, refused) = post(&app, &format!("/api/confirm/{token}"), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "gate_rejected");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("max_lot"),
        "{refused}"
    );
    assert_eq!(mock.executed_writes(), 0, "nothing reached the bridge");
    let rows = app.audit.rows(None).await.unwrap();
    assert!(
        rows.iter()
            .any(|r| r.kind == "gate_rejected" && r.payload["reason"] == "re-check at confirm"),
        "the re-check is audited"
    );
}

#[tokio::test]
async fn the_allowed_symbol_override_blocks_other_symbols_at_the_gate() {
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock,
        llm_script(vec![(
            "propose_order",
            json!({ "symbol": "eurusd", "side": "buy", "volume": 0.1 }),
        )]),
        Options::default(),
    )
    .await;
    put(
        &app,
        "/api/settings/limits",
        json!({ "allowed_symbols": ["gold"] }),
    )
    .await;
    let (_, body) = chat(&app, "buy 0.1 eurusd").await;
    assert!(body["preview"].is_null(), "{body}");
    let rows = app.audit.rows(None).await.unwrap();
    assert!(
        rows.iter()
            .any(|r| r.kind == "gate_rejected" && r.payload["reason"] == "limit:allowed_symbols")
    );
    // The UI buttons only ever close, which no symbol limit touches.
    let (status, _) = post(&app, "/api/positions/1001001/close", None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn tightened_limits_survive_a_restart() {
    let mock = MockBridge::start().await;
    let app = build_app(&mock, ScriptedLlm::new("fake", vec![]), Options::default()).await;
    put(
        &app,
        "/api/settings/limits",
        json!({ "max_lot": 0.25, "allowed_symbols": ["XAUUSD"] }),
    )
    .await;

    // A "restart": a new state over the same database file.
    let again = build_app(
        &mock,
        ScriptedLlm::new("fake", vec![]),
        Options {
            audit: Some(app.audit.clone()),
            ..Options::default()
        },
    )
    .await;
    again.state.restore_limit_overrides().await;
    let (_, view) = get(&again, "/api/settings/limits").await;
    assert_eq!(view["overrides"]["max_lot"], 0.25);
    assert_eq!(view["effective"]["max_lot"], 0.25);
    assert_eq!(view["effective"]["allowed_symbols"], json!(["XAUUSD"]));

    // A stored override can never loosen a config that was made stricter in the meantime.
    let mut config = again.state.config().clone();
    config.limits.as_mut().unwrap().max_lot = 0.1;
    let stricter = AppState::new_with(
        config,
        bridge_client(&mock, 300),
        again.audit.clone(),
        Default::default(),
        jarvis_api::state::Overrides {
            personas: Some(real_personas()),
            ..Default::default()
        },
    );
    stricter.restore_limit_overrides().await;
    assert_eq!(stricter.trading().limits().max_lot, 0.1);
    assert_eq!(stricter.trading().limits().allowed_symbols, ["XAUUSD"]);
}

#[tokio::test]
async fn limits_need_a_fresh_authenticator_code_when_login_is_on() {
    let Accounts {
        app,
        alice,
        bob,
        _mock,
    } = accounts(ScriptedLlm::new("fake", vec![])).await;
    let ac = login(&app, &alice).await;
    let bc = login(&app, &bob).await;

    // Reading needs no code.
    let (status, view) = request_as(&app, &ac, "GET", "/api/settings/limits", None).await;
    assert_eq!(status, StatusCode::OK, "{view}");

    // No code, a wrong code: 401 "code not accepted"; nothing changes.
    for body in [
        json!({ "max_lot": 0.5 }),
        json!({ "max_lot": 0.5, "totp": "000000" }),
    ] {
        let (status, response) =
            request_as(&app, &ac, "PUT", "/api/settings/limits", Some(body)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{response}");
        assert_eq!(response["error"]["code"], "unauthorized");
        assert_eq!(response["error"]["message"], "code not accepted");
    }
    let (_, view) = request_as(&app, &ac, "GET", "/api/settings/limits", None).await;
    assert_eq!(view["overrides"], Value::Null);

    // A looser value is refused before the code is looked at.
    let (status, response) = request_as(
        &app,
        &ac,
        "PUT",
        "/api/settings/limits",
        Some(json!({ "max_lot": 9.0 })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");

    // A fresh code works once.
    let code = app.fresh_code(&alice);
    let (status, view) = request_as(
        &app,
        &ac,
        "PUT",
        "/api/settings/limits",
        Some(json!({ "max_lot": 0.5, "totp": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["effective"]["max_lot"], 0.5);
    let (status, response) = request_as(
        &app,
        &ac,
        "PUT",
        "/api/settings/limits",
        Some(json!({ "max_positions": 2, "totp": code })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the same code never works twice: {response}"
    );

    // The limits are global: Bob sees Alice's, and so does his status.
    let (_, view) = request_as(&app, &bc, "GET", "/api/settings/limits", None).await;
    assert_eq!(view["effective"]["max_lot"], 0.5);
    let (_, st) = request_as(&app, &bc, "GET", "/api/status", None).await;
    assert_eq!(st["limits"]["max_lot"], 0.5);

    // Resetting needs a code too.
    let (status, response) = request_as(&app, &bc, "DELETE", "/api/settings/limits", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{response}");
    let (status, response) = request_as(
        &app,
        &bc,
        "DELETE",
        "/api/settings/limits",
        Some(json!({ "totp": "123456" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{response}");
    let code = app.fresh_code(&bob);
    let (status, view) = request_as(
        &app,
        &bc,
        "DELETE",
        "/api/settings/limits",
        Some(json!({ "totp": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["overrides"], Value::Null);
    assert_eq!(view["effective"]["max_lot"], 1.0);

    let rows = app.audit.rows(None).await.unwrap();
    let changes: Vec<_> = rows.iter().filter(|r| r.kind == "limits_changed").collect();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].payload["by"], "alice");
    assert_eq!(changes[1].payload["by"], "bob");
}

// ------------------------------------------------------------------ startup

fn write_key(dir: &std::path::Path) -> std::path::PathBuf {
    let key = jarvis_bridge_client::BridgeKey::generate().unwrap();
    let path = dir.join("bridge.key");
    std::fs::write(&path, format!("{}\n", key.expose_hex())).unwrap();
    path
}

fn startup_config(dir: &std::path::Path, personas: &str) -> jarvis_api::config::Config {
    let mut toml = config_toml("demo", "tcp://127.0.0.1:5999", "", &[], "", "", "", false);
    // Replace the test default `[personas]` section.
    let at = toml.find("[personas]").unwrap();
    toml.truncate(at);
    toml.push_str(personas);
    toml = toml.replacen(
        "[bridge]",
        &format!("[bridge]\nkey_file = '{}'", write_key(dir).display()),
        1,
    );
    toml = toml.replacen(
        "[symbols]",
        &format!("[database]\nurl = \"{}\"\n\n[symbols]", temp_db_url()),
        1,
    );
    jarvis_api::config::Config::from_toml_str(&toml).expect("test config")
}

#[tokio::test]
async fn startup_fails_clearly_when_the_personas_folder_has_no_valid_packs() {
    if std::env::var("JARVIS_BRIDGE_KEY").is_ok_and(|v| !v.trim().is_empty()) {
        return;
    }
    let dir = std::env::temp_dir().join(format!("jarvis-personas-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let broken = dir.join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(
        broken.join("jarvis.toml"),
        "id = \"jarvis\"\nname = \"J\"\n",
    )
    .unwrap();

    let sections = [
        (
            "empty folder",
            format!("[personas]\ndir = '{}'\n", empty.display()),
        ),
        (
            "missing folder",
            format!("[personas]\ndir = '{}'\n", dir.join("nope").display()),
        ),
        (
            "broken pack",
            format!("[personas]\ndir = '{}'\n", broken.display()),
        ),
        (
            "unknown default",
            format!(
                "[personas]\ndir = '{}'\ndefault = \"ultron\"\n",
                personas_dir().display()
            ),
        ),
    ];
    for (what, section) in sections {
        let err = AppState::build(startup_config(&dir, &section))
            .await
            .err()
            .unwrap_or_else(|| panic!("{what}: must refuse to start"));
        let text = format!("{err:#}");
        assert!(text.contains("refusing to start"), "{what}: {text}");
        assert!(text.contains("persona"), "{what}: {text}");
    }

    // The same config with a good folder starts, and the section may be missing altogether
    // (the folder is then `personas` under the working directory, which a test does not have).
    let good = format!("[personas]\ndir = '{}'\n", personas_dir().display());
    let state = AppState::build(startup_config(&dir, &good))
        .await
        .expect("starts");
    assert_eq!(state.personas().default_id(), "jarvis");
    assert_eq!(state.personas().list().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_config_default_persona_is_what_a_new_user_gets() {
    let mock = MockBridge::start().await;
    let friday_default =
        jarvis_persona::PersonaRegistry::load_dir(&personas_dir(), "friday").unwrap();
    let app = build_app(
        &mock,
        ScriptedLlm::new("fake", vec![]),
        Options {
            personas: Some(friday_default),
            ..Options::default()
        },
    )
    .await;
    let (_, settings) = get(&app, "/api/settings").await;
    assert_eq!(settings["persona"], "friday");
    let (_, personas) = get(&app, "/api/personas").await;
    assert_eq!(personas["default"], "friday");
    assert_eq!(personas["personas"][0]["id"], "friday");
}
