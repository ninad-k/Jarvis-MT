//! Broker symbol mapping end to end: detection, the resolver, account switches, overrides and
//! the `/api/symbols*` routes, against `MockBridge` (a CFI-like catalog by default).

mod common;

use std::collections::HashMap;
use std::time::Duration;

use axum::http::StatusCode;
use common::*;
use jarvis_api::events::Event;
use jarvis_bridge_client::mock::{MockBridge, MockState};
use jarvis_bridge_client::protocol::{BrokerInfo, CatalogSymbol, SymbolTradeMode, TradeMode};
use serde_json::{Value, json};

// ------------------------------------------------------------------ fixtures

/// The mock's trading data (prices, specs, positions, orders, deals) is keyed by plain names;
/// rename it to the names of the CFI-like catalog the mock serves.
fn cfi_like() -> MockState {
    let mut state = MockState::default();
    rename(
        &mut state,
        &[
            ("XAUUSD", "XAUUSD_"),
            ("US100", "US100_Spot"),
            ("US30", "US30_SPOT"),
            ("EURUSD", "EURUSD_"),
            ("GBPUSD", "GBPUSD_"),
        ],
    );
    state
}

fn rename(state: &mut MockState, pairs: &[(&str, &str)]) {
    let map: HashMap<&str, &str> = pairs.iter().copied().collect();
    let swap = |s: &mut String| {
        if let Some(to) = map.get(s.as_str()) {
            *s = (*to).to_owned();
        }
    };
    state.prices.iter_mut().for_each(|p| swap(&mut p.symbol));
    state.symbols.iter_mut().for_each(|p| swap(&mut p.symbol));
    state.positions.iter_mut().for_each(|p| swap(&mut p.symbol));
    state.orders.iter_mut().for_each(|p| swap(&mut p.symbol));
    state.deals.iter_mut().for_each(|p| swap(&mut p.symbol));
}

fn entry(name: &str, description: &str, visible: bool) -> CatalogSymbol {
    CatalogSymbol {
        name: name.to_owned(),
        description: description.to_owned(),
        path: format!("All\\{name}"),
        trade_mode: SymbolTradeMode::Full,
        visible,
        digits: 2,
        currency_base: "USD".to_owned(),
        currency_profit: "USD".to_owned(),
    }
}

/// An IC-Markets-like broker: plain forex and gold names, Nasdaq as `USTEC`.
fn ic_markets() -> (BrokerInfo, Vec<CatalogSymbol>) {
    (
        BrokerInfo {
            company: "Raw Trading Ltd".to_owned(),
            server: "ICMarketsSC-Demo".to_owned(),
            trade_mode: TradeMode::Demo,
        },
        vec![
            entry("XAUUSD", "Gold vs US Dollar", true),
            entry("XAGUSD", "Silver vs US Dollar", true),
            entry("USTEC", "Nasdaq 100 Index", true),
            entry("US30", "Dow Jones Industrial Average", true),
            entry("EURUSD", "Euro vs US Dollar", true),
            entry("GBPUSD", "Great Britain Pound vs US Dollar", true),
            entry("BTCUSD", "Bitcoin vs US Dollar", true),
        ],
    )
}

fn maps() -> Options<'static> {
    Options {
        maps: true,
        ..Options::default()
    }
}

async fn get(app: &TestApp, uri: &str) -> (StatusCode, Value) {
    request(&app.router, "GET", uri, None).await
}

async fn app_on(mock: &MockBridge, opts: Options<'_>) -> TestApp {
    build_app(mock, ScriptedLlm::new("fake", vec![]), opts).await
}

fn row<'a>(view: &'a Value, canonical: &str) -> &'a Value {
    view["mappings"]
        .as_array()
        .expect("mappings")
        .iter()
        .find(|r| r["canonical"] == canonical)
        .unwrap_or_else(|| panic!("no mapping for {canonical} in {view}"))
}

// ------------------------------------------------------------------ detection

#[tokio::test]
async fn startup_detection_writes_the_generated_file_and_the_snapshot() {
    let mock = MockBridge::start_with(cfi_like()).await;
    let app = app_on(&mock, maps()).await;
    let mut events = app.state.events().subscribe();
    app.state.startup_check().await.unwrap();

    let dir = app.data_dir.clone().unwrap();
    let generated = std::fs::read_to_string(dir.join("broker_symbols.generated.toml")).unwrap();
    assert!(generated.contains("Mock Broker Ltd."), "{generated}");
    // FX and metals follow the detected suffix pattern; indices are explicit entries.
    assert!(generated.contains("${1}_"), "{generated}");
    assert!(generated.contains("US100 = \"US100_Spot\""), "{generated}");
    let snapshot: Value = serde_json::from_slice(
        &std::fs::read(dir.join("broker-catalogs/mock-broker-ltd.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(snapshot["company"], "Mock Broker Ltd.");
    assert_eq!(snapshot["symbols"].as_array().unwrap().len(), 55);

    let rows = app.audit.rows(None).await.unwrap();
    let detected: Vec<_> = rows
        .iter()
        .filter(|r| r.kind == "symbol_map_detected")
        .collect();
    assert_eq!(detected.len(), 1);
    let p = &detected[0].payload;
    assert_eq!(p["company"], "Mock Broker Ltd.");
    assert_eq!(p["server"], "Mock-Demo");
    assert_eq!(p["total"], 55);
    assert!(p["matched"].as_u64().unwrap() >= 10, "{p}");
    assert_eq!(p["suffix"], "_");

    assert_eq!(
        events.recv().await.unwrap(),
        Event::SymbolMapChanged {
            broker: "Mock Broker Ltd.".to_owned()
        }
    );

    let (status, view) = get(&app, "/api/symbols").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["broker"]["company"], "Mock Broker Ltd.");
    assert_eq!(view["broker"]["catalog_size"], 55);
    assert_eq!(view["broker"]["detected_suffix"], "_");
    let gold = row(&view, "XAUUSD");
    assert_eq!(gold["broker_symbol"], "XAUUSD_");
    assert_eq!(gold["class"], "metal");
    assert_eq!(gold["available"], true);
    assert!(gold["aliases"].as_array().unwrap().contains(&json!("GOLD")));
    assert!(gold["candidates"].as_array().is_some_and(|c| !c.is_empty()));

    let (_, status_body) = get(&app, "/api/status").await;
    assert_eq!(status_body["broker"]["company"], "Mock Broker Ltd.");
    assert_eq!(status_body["broker"]["server"], "Mock-Demo");
    assert!(status_body["broker"]["mapped"].as_u64().unwrap() >= 10);
    assert!(status_body["broker"]["total"].as_u64().unwrap() >= 30);
}

#[tokio::test]
async fn a_second_start_reuses_nothing_stale_and_detects_again() {
    let mock = MockBridge::start_with(cfi_like()).await;
    let first = app_on(&mock, maps()).await;
    first.state.startup_check().await.unwrap();
    assert_eq!(first.state.broker().detection_stats().0, 1);
    // Another process over the same data folder reads the saved snapshot but still detects.
    let dir = first.data_dir.clone().unwrap();
    let second = app_on(&mock, maps()).await;
    // (the second app has its own data folder; copy the first one's files into it)
    let second_dir = second.data_dir.clone().unwrap();
    std::fs::create_dir_all(second_dir.join("broker-catalogs")).unwrap();
    std::fs::copy(
        dir.join("broker-catalogs/mock-broker-ltd.json"),
        second_dir.join("broker-catalogs/mock-broker-ltd.json"),
    )
    .unwrap();
    second.state.startup_check().await.unwrap();
    assert_eq!(second.state.broker().detection_stats().0, 1);
}

#[tokio::test]
async fn without_maps_loaded_nothing_is_detected_and_names_pass_through() {
    let mock = MockBridge::start().await;
    let app = app_on(&mock, Options::default()).await;
    app.state.startup_check().await.unwrap();
    assert_eq!(app.state.broker().detection_stats().0, 0);
    assert_eq!(
        app.state.resolver().resolve("gold").as_deref(),
        Some("XAUUSD")
    );
}

// ------------------------------------------------------------------ resolver

#[tokio::test]
async fn gold_resolves_to_the_brokers_name_and_rows_carry_canonical() {
    let mock = MockBridge::start_with(cfi_like()).await;
    let app = app_on(&mock, maps()).await;
    app.state.startup_check().await.unwrap();
    let r = app.state.resolver();
    assert_eq!(r.resolve("gold").as_deref(), Some("XAUUSD_"));
    assert_eq!(r.resolve("Gold").as_deref(), Some("XAUUSD_"));
    assert_eq!(
        r.resolve("cold").as_deref(),
        Some("XAUUSD_"),
        "config alias"
    );
    assert_eq!(r.resolve("nasdaq").as_deref(), Some("US100_Spot"));
    assert_eq!(
        r.resolve("NAS100").as_deref(),
        Some("US100_Spot"),
        "catalog alias"
    );
    assert_eq!(r.resolve("dow jones").as_deref(), Some("US30_SPOT"));
    // The broker's own name is accepted as is, and reversed to canonical.
    assert_eq!(r.resolve("xauusd_").as_deref(), Some("XAUUSD_"));
    assert_eq!(r.canonical("xauusd_").as_deref(), Some("XAUUSD"));
    assert_eq!(r.canonical("gold").as_deref(), Some("XAUUSD"));
    assert_eq!(r.canonical_of_broker_symbol("US100_Spot"), "US100");

    let (_, positions) = get(&app, "/api/positions").await;
    let rows = positions["positions"].as_array().unwrap();
    assert!(!rows.is_empty());
    for p in rows {
        let symbol = p["symbol"].as_str().unwrap();
        let expected = match symbol {
            "US100_Spot" => "US100",
            other => other.trim_end_matches('_'),
        };
        assert_eq!(p["canonical"].as_str().unwrap(), expected, "{p}");
    }
    let (_, orders) = get(&app, "/api/orders").await;
    for o in orders["orders"].as_array().unwrap() {
        assert_eq!(
            o["canonical"].as_str().unwrap(),
            o["symbol"].as_str().unwrap().trim_end_matches('_')
        );
    }

    // The model sees both names.
    let tools = app.state.agent().tools().clone();
    let out = tools
        .execute("get_price", &json!({ "symbol": "gold" }))
        .await;
    assert!(!out.is_error, "{}", out.content);
    let price: Value = serde_json::from_str(&out.content).unwrap();
    assert_eq!(price["symbol"], "XAUUSD_");
    assert_eq!(price["canonical"], "XAUUSD");
}

#[tokio::test]
async fn watchlist_rows_carry_canonical() {
    let mock = MockBridge::start_with(cfi_like()).await;
    let app = app_on(&mock, maps()).await;
    app.state.startup_check().await.unwrap();
    let (status, body) = get(&app, "/api/watchlist").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let prices = body["prices"].as_array().unwrap();
    assert_eq!(prices.len(), 3);
    assert_eq!(prices[0]["symbol"], "XAUUSD_");
    assert_eq!(prices[0]["canonical"], "XAUUSD");
    assert_eq!(prices[1]["symbol"], "US100_Spot");
    assert_eq!(prices[1]["canonical"], "US100");
    assert_eq!(prices[2]["canonical"], "EURUSD");
}

#[tokio::test]
async fn the_suffix_is_only_a_fallback_for_brokers_without_a_map() {
    // Detection off: the mock broker has no map in any layer, so the suffix applies.
    let mock = MockBridge::start().await;
    let off = app_on(
        &mock,
        Options {
            maps: true,
            auto_detect: false,
            suffix: ".m",
            ..Options::default()
        },
    )
    .await;
    off.state.startup_check().await.unwrap();
    assert_eq!(
        off.state.resolver().resolve("gold").as_deref(),
        Some("XAUUSD.m")
    );
    assert_eq!(
        off.state.resolver().canonical("gold").as_deref(),
        Some("XAUUSD")
    );

    // Detection on: the detected map wins and the suffix is not appended.
    let mock = MockBridge::start_with(cfi_like()).await;
    let on = app_on(
        &mock,
        Options {
            maps: true,
            suffix: ".m",
            ..Options::default()
        },
    )
    .await;
    on.state.startup_check().await.unwrap();
    assert_eq!(
        on.state.resolver().resolve("gold").as_deref(),
        Some("XAUUSD_")
    );
}

// ------------------------------------------------------------------ account switch

#[tokio::test]
async fn an_account_switch_is_redetected_on_the_next_ping() {
    let mock = MockBridge::start_with(cfi_like()).await;
    let app = app_on(&mock, maps()).await;
    app.state.startup_check().await.unwrap();
    assert_eq!(
        app.state.resolver().resolve("gold").as_deref(),
        Some("XAUUSD_")
    );
    let mut events = app.state.events().subscribe();

    let (info, catalog) = ic_markets();
    mock.set_broker(info, catalog);
    let (status, body) = get(&app, "/api/status").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["broker"]["company"], "Raw Trading Ltd");
    assert_eq!(body["broker"]["server"], "ICMarketsSC-Demo");

    assert_eq!(
        app.state.resolver().resolve("gold").as_deref(),
        Some("XAUUSD")
    );
    assert_eq!(
        app.state.resolver().resolve("nasdaq").as_deref(),
        Some("USTEC")
    );
    assert_eq!(
        app.state.resolver().resolve("eurusd").as_deref(),
        Some("EURUSD")
    );
    assert_eq!(app.state.broker().detection_stats().0, 2);

    // A switch event, then the detection's.
    let mut seen = Vec::new();
    while let Ok(Ok(e)) = tokio::time::timeout(Duration::from_millis(100), events.recv()).await {
        seen.push(e);
    }
    assert!(
        seen.iter().all(|e| matches!(
            e,
            Event::SymbolMapChanged { broker } if broker == "Raw Trading Ltd"
        )) && !seen.is_empty(),
        "{seen:?}"
    );

    let (_, view) = get(&app, "/api/symbols").await;
    assert_eq!(view["broker"]["company"], "Raw Trading Ltd");
    assert_eq!(row(&view, "XAUUSD")["broker_symbol"], "XAUUSD");
    assert_eq!(row(&view, "US100")["broker_symbol"], "USTEC");
    // Instruments this broker does not list are reported, not hidden.
    assert!(
        view["unmapped"]
            .as_array()
            .unwrap()
            .contains(&json!("HK50")),
        "{}",
        view["unmapped"]
    );
    assert_eq!(row(&view, "HK50")["available"], false);

    // Both maps are kept in the generated file; the first broker's is untouched.
    let generated = std::fs::read_to_string(
        app.data_dir
            .clone()
            .unwrap()
            .join("broker_symbols.generated.toml"),
    )
    .unwrap();
    assert!(generated.contains("Mock Broker Ltd."));
    assert!(generated.contains("Raw Trading Ltd"));
}

#[tokio::test]
async fn a_preview_made_before_an_account_switch_is_refused_at_confirm() {
    let mock = MockBridge::start_with(cfi_like()).await;
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            tool_use(
                "t1",
                "propose_order",
                json!({ "symbol": "gold", "side": "buy", "volume": 0.1 }),
            ),
            text("Please confirm."),
        ],
    );
    let app = build_app(&mock, llm, maps()).await;
    app.state.startup_check().await.unwrap();
    let (status, body) = chat(&app, "buy 0.1 gold").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let preview = &body["preview"];
    assert_eq!(preview["symbols"], json!(["XAUUSD_"]), "{body}");
    let token = preview["token"].as_str().unwrap().to_owned();

    let (info, catalog) = ic_markets();
    mock.set_broker(info, catalog);
    let (status, body) = request(&app.router, "POST", &format!("/api/confirm/{token}"), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "broker_changed");
    assert_eq!(
        mock.executed_writes(),
        0,
        "nothing may reach the new broker"
    );

    // The token is spent: switching back does not revive it.
    mock.set_broker(
        BrokerInfo {
            company: "Mock Broker Ltd.".to_owned(),
            server: "Mock-Demo".to_owned(),
            trade_mode: TradeMode::Demo,
        },
        jarvis_bridge_client::mock::default_catalog(),
    );
    let (status, body) = request(&app.router, "POST", &format!("/api/confirm/{token}"), None).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(mock.executed_writes(), 0);

    let rows = app.audit.rows(None).await.unwrap();
    assert!(
        rows.iter().any(|r| r.kind == "confirm"
            && r.payload["accepted"] == false
            && r.payload["reason"] == "broker_changed"),
        "the refusal must be audited"
    );
}

#[tokio::test]
async fn allowed_symbols_are_canonical_and_work_on_every_broker() {
    let mock = MockBridge::start_with(cfi_like()).await;
    let llm = ScriptedLlm::new(
        "fake",
        vec![
            // CFI-like: gold is allowed, bitcoin is not.
            tool_use(
                "t1",
                "propose_order",
                json!({ "symbol": "gold", "side": "buy", "volume": 0.1 }),
            ),
            text("ok"),
            tool_use(
                "t2",
                "propose_order",
                json!({ "symbol": "bitcoin", "side": "buy", "volume": 0.1 }),
            ),
            text("blocked"),
            // IC-like: the same canonical limit lets the broker's plain XAUUSD through.
            tool_use(
                "t3",
                "propose_order",
                json!({ "symbol": "gold", "side": "buy", "volume": 0.1 }),
            ),
            text("ok again"),
        ],
    );
    let app = build_app(
        &mock,
        llm,
        Options {
            maps: true,
            limits: "max_lot = 1.0\nmax_positions = 5\nmax_daily_loss = 500.0\nallowed_symbols = [\"XAUUSD\", \"USTEC\"]",
            ..Options::default()
        },
    )
    .await;
    app.state.startup_check().await.unwrap();
    // A generic alias in the config is canonical now.
    assert_eq!(
        app.state.trading().limits().allowed_symbols,
        ["XAUUSD", "US100"]
    );

    let (_, body) = chat(&app, "buy 0.1 gold").await;
    assert_eq!(body["preview"]["symbols"], json!(["XAUUSD_"]), "{body}");

    let (_, body) = chat(&app, "buy 0.1 bitcoin").await;
    assert!(body["preview"].is_null(), "{body}");
    let rows = app.audit.rows(None).await.unwrap();
    assert!(
        rows.iter()
            .any(|r| r.kind == "gate_rejected" && r.payload["reason"] == "limit:allowed_symbols"),
        "bitcoin must be refused by allowed_symbols"
    );

    // Switch the account: the mock now trades plain names.
    mock.update_state(|s| {
        rename(
            s,
            &[
                ("XAUUSD_", "XAUUSD"),
                ("US100_Spot", "USTEC"),
                ("US30_SPOT", "US30"),
                ("EURUSD_", "EURUSD"),
                ("GBPUSD_", "GBPUSD"),
            ],
        );
    });
    let (info, catalog) = ic_markets();
    mock.set_broker(info, catalog);
    app.state.ping_bridge().await.unwrap();
    let (_, body) = chat(&app, "buy 0.1 gold again").await;
    assert_eq!(body["preview"]["symbols"], json!(["XAUUSD"]), "{body}");
}

// ------------------------------------------------------------------ overrides

#[tokio::test]
async fn a_user_override_can_be_set_and_cleared() {
    let mock = MockBridge::start_with(cfi_like()).await;
    let app = app_on(&mock, maps()).await;
    app.state.startup_check().await.unwrap();
    let mut events = app.state.events().subscribe();

    let (status, view) = request(
        &app.router,
        "PUT",
        "/api/symbols/map",
        Some(json!({ "canonical": "US100", "broker_symbol": "NAS1226" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    // The response is the refreshed state.
    assert_eq!(row(&view, "US100")["broker_symbol"], "NAS1226");
    assert_eq!(row(&view, "US100")["source"], "user");
    assert_eq!(
        app.state.resolver().resolve("nasdaq").as_deref(),
        Some("NAS1226")
    );
    assert_eq!(
        events.recv().await.unwrap(),
        Event::SymbolMapChanged {
            broker: "Mock Broker Ltd.".to_owned()
        }
    );
    let user_file = std::fs::read_to_string(
        app.data_dir
            .clone()
            .unwrap()
            .join("broker_symbols.user.toml"),
    )
    .unwrap();
    assert!(user_file.contains("NAS1226"), "{user_file}");

    let (status, view) = request(
        &app.router,
        "PUT",
        "/api/symbols/map",
        Some(json!({ "canonical": "us100", "broker_symbol": null })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(row(&view, "US100")["broker_symbol"], "US100_Spot");
    assert_ne!(row(&view, "US100")["source"], "user");

    let rows = app.audit.rows(None).await.unwrap();
    let audits: Vec<_> = rows
        .iter()
        .filter(|r| r.kind == "symbol_map_override")
        .collect();
    assert_eq!(audits.len(), 2);
    assert_eq!(audits[0].payload["canonical"], "US100");
    assert_eq!(audits[0].payload["broker_symbol"], "NAS1226");
    assert!(audits[1].payload["broker_symbol"].is_null());
}

#[tokio::test]
async fn overrides_are_validated() {
    let mock = MockBridge::start_with(cfi_like()).await;
    let app = app_on(&mock, maps()).await;
    app.state.startup_check().await.unwrap();

    let (status, body) = request(
        &app.router,
        "PUT",
        "/api/symbols/map",
        Some(json!({ "canonical": "US100", "broker_symbol": "NOT_A_SYMBOL" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "unknown_broker_symbol");

    for bad in [
        json!({ "canonical": "NOT_AN_INSTRUMENT", "broker_symbol": "NAS1226" }),
        json!({ "canonical": "US100" }),
        json!({ "broker_symbol": "NAS1226" }),
        json!({ "canonical": "US100", "broker_symbol": "NAS1226", "extra": 1 }),
    ] {
        let (status, body) =
            request(&app.router, "PUT", "/api/symbols/map", Some(bad.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    assert_eq!(
        app.state.resolver().resolve("nasdaq").as_deref(),
        Some("US100_Spot"),
        "nothing changed"
    );
}

#[tokio::test]
async fn overrides_need_a_totp_code_when_login_is_on() {
    let mock = MockBridge::start_with(cfi_like()).await;
    let app = app_on(
        &mock,
        Options {
            maps: true,
            auth: true,
            ..Options::default()
        },
    )
    .await;
    let user = create_user(&app, "ninad", PASSWORD).await;
    let cookie = login(&app, &user).await;
    app.state.startup_check().await.unwrap();

    let body = json!({ "canonical": "US100", "broker_symbol": "NAS1226" });
    let (status, resp) =
        request_as(&app, &cookie, "PUT", "/api/symbols/map", Some(body.clone())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{resp}");
    assert_eq!(
        app.state.resolver().resolve("nasdaq").as_deref(),
        Some("US100_Spot")
    );

    // An unknown symbol is refused before the code is spent.
    let (status, resp) = request_as(
        &app,
        &cookie,
        "PUT",
        "/api/symbols/map",
        Some(json!({ "canonical": "US100", "broker_symbol": "nope", "totp": "000000" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{resp}");

    let code = app.fresh_code(&user);
    let mut with_code = body.clone();
    with_code["totp"] = json!(code);
    let (status, resp) =
        request_as(&app, &cookie, "PUT", "/api/symbols/map", Some(with_code)).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(row(&resp, "US100")["source"], "user");

    // Without a session nothing works at all; detection needs a session but no code.
    let (status, _) = request(&app.router, "POST", "/api/symbols/detect", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, resp) = request_as(&app, &cookie, "POST", "/api/symbols/detect", None).await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    let rows = app.audit.rows(None).await.unwrap();
    let audit = rows
        .iter()
        .find(|r| r.kind == "symbol_map_override")
        .expect("override audited");
    assert_eq!(audit.payload["by"], "ninad");
}

// ------------------------------------------------------------------ routes

#[tokio::test]
async fn catalog_search_export_and_detect_routes() {
    let mock = MockBridge::start_with(cfi_like()).await;
    let app = app_on(&mock, maps()).await;
    app.state.startup_check().await.unwrap();

    let (status, body) = get(&app, "/api/symbols/catalog?q=nas&limit=50").await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = body["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"US100_Spot"), "{names:?}");
    assert!(names.contains(&"NAS1226"), "{names:?}");
    assert!(!names.contains(&"XAUUSD_"));
    assert_eq!(body["total"].as_u64().unwrap() as usize, names.len());
    let first = &body["symbols"][0];
    for key in ["name", "description", "path", "trade_mode", "visible"] {
        assert!(first.get(key).is_some(), "{key} in {first}");
    }
    assert!(
        ["full", "long_only", "short_only", "close_only", "disabled"]
            .contains(&first["trade_mode"].as_str().unwrap()),
        "{first}"
    );

    // Description match, case-insensitive; limit applies to the list, not to total.
    let (_, body) = get(&app, "/api/symbols/catalog?q=BITCOIN&limit=1").await;
    assert_eq!(body["symbols"][0]["name"], "BTCUSD");
    let (_, body) = get(&app, "/api/symbols/catalog?q=_&limit=3").await;
    assert_eq!(body["symbols"].as_array().unwrap().len(), 3);
    assert!(body["total"].as_u64().unwrap() > 3);
    let (_, body) = get(&app, "/api/symbols/catalog").await;
    assert_eq!(body["symbols"].as_array().unwrap().len(), 50);
    assert_eq!(body["total"], 55);

    let (status, headers, bytes) =
        raw_request(&app.router, "GET", "/api/symbols/export", &[], None).await;
    assert_eq!(status, StatusCode::OK);
    let disposition = headers["content-disposition"].to_str().unwrap();
    assert!(disposition.starts_with("attachment;"), "{disposition}");
    assert!(disposition.contains(".json"), "{disposition}");
    let export: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(export["broker"], "Mock Broker Ltd.");
    assert_eq!(export["server"], "Mock-Demo");
    assert!(export["generated_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(export["map"]["XAUUSD"], "XAUUSD_");
    assert_eq!(export["map"]["US100"], "US100_Spot");
    assert!(export["map"].get("HK50").is_none() || export["map"]["HK50"] == "HK50_SPOT");

    let before = app.state.broker().detection_stats().0;
    let (status, view) = request(&app.router, "POST", "/api/symbols/detect", None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(row(&view, "XAUUSD")["broker_symbol"], "XAUUSD_");
    assert_eq!(app.state.broker().detection_stats().0, before + 1);
}

#[tokio::test]
async fn routes_before_any_broker_was_seen() {
    let mock = MockBridge::start().await;
    let app = app_on(&mock, maps()).await; // no ping yet
    let (status, view) = get(&app, "/api/symbols").await;
    assert_eq!(status, StatusCode::OK);
    assert!(view["broker"].is_null());
    assert!(!view["mappings"].as_array().unwrap().is_empty());
    let (_, catalog) = get(&app, "/api/symbols/catalog?q=x").await;
    assert_eq!(catalog, json!({ "symbols": [], "total": 0 }));
    let (status, _) = get(&app, "/api/symbols/export").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, body) = request(
        &app.router,
        "PUT",
        "/api/symbols/map",
        Some(json!({ "canonical": "US100", "broker_symbol": null })),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let (_, status_body) = get(&app, "/api/status").await;
    assert_eq!(status_body["broker"]["company"], "Mock Broker Ltd.");
}

#[tokio::test]
async fn ui_selectors_accept_canonical_or_broker_symbols() {
    let mock = MockBridge::start_with(cfi_like()).await;
    let app = app_on(&mock, maps()).await;
    app.state.startup_check().await.unwrap();
    for symbol in ["XAUUSD", "gold", "XAUUSD_"] {
        let (status, body) = request(
            &app.router,
            "POST",
            "/api/positions/close",
            Some(json!({ "symbol": symbol })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{symbol}: {body}");
        assert_eq!(body["symbols"], json!(["XAUUSD_"]), "{symbol}");
    }
}

// ------------------------------------------------------------------ concurrency

#[tokio::test]
async fn detections_never_overlap() {
    let mock = MockBridge::start_with(cfi_like()).await;
    let app = app_on(&mock, maps()).await;
    app.state.startup_check().await.unwrap();
    let base = app.state.broker().detection_stats().0;

    let mut tasks = Vec::new();
    for _ in 0..4 {
        let router = app.router.clone();
        tasks.push(tokio::spawn(async move {
            request(&router, "POST", "/api/symbols/detect", None)
                .await
                .0
        }));
    }
    // Pings that see the broker meanwhile never start a second one either.
    for _ in 0..4 {
        let state = app.state.clone();
        tasks.push(tokio::spawn(async move {
            let _ = state.ping_bridge().await;
            StatusCode::OK
        }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap(), StatusCode::OK);
    }
    let (done, max_at_once) = app.state.broker().detection_stats();
    assert_eq!(done, base + 4);
    assert_eq!(max_at_once, 1, "detections must run one at a time");
}
