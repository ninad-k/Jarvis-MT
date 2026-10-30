//! Tool registry + agent loop against `MockBridge` with a scripted fake LLM.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use jarvis_api::clock::ServerClock;
use jarvis_api::symbols::SymbolResolver;
use jarvis_api::tools::ToolRegistry;
use jarvis_bridge_client::mock::MockBridge;
use jarvis_bridge_client::{BridgeClient, BridgeConfig};
use serde_json::{Value, json};

fn registry(endpoint: &str, suffix: &str) -> ToolRegistry {
    let aliases = [
        ("gold", "XAUUSD"),
        ("cold", "XAUUSD"),
        ("nasdaq", "US100"),
        ("dow jones", "US30"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let bridge = BridgeClient::new(BridgeConfig {
        endpoint: endpoint.to_owned(),
        timeout: Duration::from_millis(300),
    })
    .unwrap();
    ToolRegistry::new(
        bridge,
        Arc::new(SymbolResolver::new(&aliases, suffix)),
        ServerClock::default(),
    )
}

fn parse(content: &str) -> Value {
    serde_json::from_str(content).unwrap_or_else(|e| panic!("not JSON ({e}): {content}"))
}

#[tokio::test]
async fn get_price_resolves_aliases() {
    let mock = MockBridge::start().await;
    let tools = registry(&mock.endpoint, "");
    for input in ["gold", "Gold", "GOLD", "cold", "xauusd", "  XAUUSD "] {
        let out = tools
            .execute("get_price", &json!({ "symbol": input }))
            .await;
        assert!(!out.is_error, "{input}: {}", out.content);
        let v = parse(&out.content);
        assert_eq!(v["symbol"], "XAUUSD", "{input}");
        assert_eq!(v["bid"], 2391.12);
        assert_eq!(
            out.bridge_request.as_ref().unwrap()["args"]["symbol"],
            "XAUUSD"
        );
        assert_eq!(out.bridge_reply.as_ref().unwrap()["symbol"], "XAUUSD");
    }
    let out = tools
        .execute("get_price", &json!({ "symbol": "dow jones" }))
        .await;
    assert_eq!(parse(&out.content)["symbol"], "US30");
}

#[tokio::test]
async fn get_price_errors_are_tool_errors() {
    let mock = MockBridge::start().await;
    let tools = registry(&mock.endpoint, "");

    let unknown = tools
        .execute("get_price", &json!({ "symbol": "goldd" }))
        .await;
    assert!(unknown.is_error);
    assert!(
        unknown.content.contains("check the symbol name"),
        "{}",
        unknown.content
    );
    assert!(unknown.bridge_reply.is_some());

    let missing = tools.execute("get_price", &json!({})).await;
    assert!(missing.is_error);
    assert!(
        missing.content.contains("invalid arguments"),
        "{}",
        missing.content
    );
    assert!(missing.bridge_request.is_none());

    let junk = tools
        .execute("get_price", &json!({ "symbol": "***" }))
        .await;
    assert!(junk.is_error);
    assert!(junk.bridge_request.is_none());

    let wrong_type = tools.execute("get_price", &json!({ "symbol": 7 })).await;
    assert!(wrong_type.is_error);

    let not_object = tools.execute("get_price", &json!("gold")).await;
    assert!(not_object.is_error);
}

#[tokio::test]
async fn unknown_or_write_tool_names_are_rejected() {
    let mock = MockBridge::start().await;
    let tools = registry(&mock.endpoint, "");
    for name in [
        "order_send",
        "propose_order",
        "position_close",
        "close_all",
        "nonsense",
    ] {
        let out = tools
            .execute(name, &json!({ "symbol": "XAUUSD", "volume": 1 }))
            .await;
        assert!(out.is_error, "{name}");
        assert!(
            out.bridge_request.is_none(),
            "{name} must not reach the bridge"
        );
    }
    assert_eq!(mock.request_count(), 0);
}

#[tokio::test]
async fn bridge_down_becomes_an_error_result() {
    let tools = registry(DEAD_ENDPOINT, "");
    let out = tools.execute("get_account", &json!({})).await;
    assert!(out.is_error);
    assert!(out.content.contains("not reachable"), "{}", out.content);
    assert!(out.bridge_reply.unwrap()["error"].is_string());
}

#[tokio::test]
async fn get_account_has_no_identity_fields() {
    let mock = MockBridge::start().await;
    let tools = registry(&mock.endpoint, "");
    let out = tools.execute("get_account", &Value::Null).await;
    assert!(!out.is_error, "{}", out.content);
    let v = parse(&out.content);
    assert_eq!(v["currency"], "USD");
    assert_eq!(v["balance"], 10000.0);
    for forbidden in ["login", "name", "server"] {
        assert!(v.get(forbidden).is_none());
    }
}

#[tokio::test]
async fn get_positions_filters_by_symbol_and_side() {
    let mock = MockBridge::start().await;
    let tools = registry(&mock.endpoint, "");

    let all = parse(&tools.execute("get_positions", &json!({})).await.content);
    assert_eq!(all["count"], 2);
    assert_eq!(all["total_profit"], 70.55);

    let buys = parse(
        &tools
            .execute("get_positions", &json!({ "side": "buy" }))
            .await
            .content,
    );
    assert_eq!(buys["count"], 1);
    assert_eq!(buys["positions"][0]["symbol"], "XAUUSD");

    let sells = parse(
        &tools
            .execute("get_positions", &json!({ "side": "SELL" }))
            .await
            .content,
    );
    assert_eq!(sells["count"], 1);
    assert_eq!(sells["positions"][0]["symbol"], "US100");

    let gold = parse(
        &tools
            .execute("get_positions", &json!({ "symbol": "gold" }))
            .await
            .content,
    );
    assert_eq!(gold["count"], 1);

    let none = parse(
        &tools
            .execute(
                "get_positions",
                &json!({ "symbol": "gold", "side": "sell" }),
            )
            .await
            .content,
    );
    assert_eq!(none["count"], 0);
    assert_eq!(none["total_profit"], 0.0);

    let bad = tools
        .execute("get_positions", &json!({ "side": "sideways" }))
        .await;
    assert!(bad.is_error);

    // Empty-string optionals (some models send them) mean "no filter".
    let blank = parse(
        &tools
            .execute("get_positions", &json!({ "symbol": "", "side": "" }))
            .await
            .content,
    );
    assert_eq!(blank["count"], 2);
}

#[tokio::test]
async fn get_orders_and_history() {
    let mock = MockBridge::start().await;
    let tools = registry(&mock.endpoint, "");

    let orders = parse(&tools.execute("get_orders", &json!({})).await.content);
    assert_eq!(orders["count"], 1);
    assert_eq!(orders["orders"][0]["symbol"], "EURUSD");

    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let from = (chrono::Utc::now() - chrono::Duration::days(30))
        .format("%Y-%m-%d")
        .to_string();
    let hist = tools
        .execute("get_history", &json!({ "from": from, "to": today }))
        .await;
    assert!(!hist.is_error, "{}", hist.content);
    let v = parse(&hist.content);
    assert!(v["listed"].as_u64().unwrap() >= 1);
    assert!(v["deals"].is_array());
    // Totals are computed in Rust over every trade deal in the range.
    assert!(v["summary"]["net_profit"].is_number());
    assert!(v["summary"]["deals"].as_u64().unwrap() >= 1);
    assert_eq!(v["summary"]["complete"], true);
    let sent = hist.bridge_request.unwrap();
    assert_eq!(sent["cmd"], "get_history");
    assert!(sent["args"]["from"].is_i64());
    // Always fetches the maximum so the summary covers the whole range.
    assert_eq!(sent["args"]["limit"], 500);

    // `limit` caps the deals listed to the model, not the summary.
    let one = parse(
        &tools
            .execute(
                "get_history",
                &json!({ "from": from, "to": today, "limit": 1 }),
            )
            .await
            .content,
    );
    assert_eq!(one["listed"], 1);
    assert_eq!(one["summary"]["deals"], v["summary"]["deals"]);

    // `to` as a bare date is the end of that day.
    let d = tools
        .execute(
            "get_history",
            &json!({ "from": "2026-10-30", "to": "2026-10-30", "limit": 9999 }),
        )
        .await;
    let sent = d.bridge_request.unwrap();
    assert_eq!(
        sent["args"]["to"].as_i64().unwrap() - sent["args"]["from"].as_i64().unwrap(),
        86_399
    );
    assert_eq!(sent["args"]["limit"], 500);

    let bad = tools
        .execute(
            "get_history",
            &json!({ "from": "last week", "to": "today" }),
        )
        .await;
    assert!(bad.is_error);
    assert!(bad.bridge_request.is_none());
    let backwards = tools
        .execute(
            "get_history",
            &json!({ "from": "2026-10-30", "to": "2026-10-01" }),
        )
        .await;
    assert!(backwards.is_error);
}

// ------------------------------------------------------------- agent loop

#[tokio::test]
async fn price_of_gold_scenario_end_to_end_through_the_agent() {
    let mock = MockBridge::start().await;
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            tool_use("t1", "get_price", json!({ "symbol": "Gold" })),
            tool_use("t2", "get_positions", json!({ "side": "buy" })),
            text("Gold is 2391.12 and you hold one long."),
        ],
    );
    let app = build_app(&mock.endpoint, llm.clone(), Options::default()).await;
    let (status, body) = chat(&app, "price of gold and my longs").await;
    assert_eq!(status, 200);
    let calls = body["tool_calls"].as_array().unwrap();
    assert_eq!(
        calls
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["get_price", "get_positions"]
    );
    assert!(calls.iter().all(|c| c["ok"] == true));
    assert_eq!(llm.request_count(), 3);
    // Every request offered only read tools and propose tools.
    for r in llm.requests.lock().unwrap().iter() {
        for t in &r.tools {
            assert!(
                t.name.starts_with("get_") || t.name.starts_with("propose_"),
                "{}",
                t.name
            );
        }
    }
}
