//! `get_broker` and `list_symbols`: golden fixtures, wire format, and the client against the
//! mock EA (paging, `list_all_symbols`, a catalog that changes while it is read).
use std::path::PathBuf;

use jarvis_bridge_client::protocol::{
    BrokerInfo, Command, LIST_SYMBOLS_LIMIT_MAX, ListSymbolsQuery, Reply, ReplyError, Request,
    SymbolCatalogPage, SymbolTradeMode, TradeMode,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read fixture {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("fixture {name} is not JSON: {e}"))
}

/// Deserialize `value` into `T`, serialize it back and require identical JSON.
fn round_trip<T: Serialize + DeserializeOwned>(what: &str, value: &Value) -> T {
    let typed: T = serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("{what}: cannot deserialize: {e}"));
    let back = serde_json::to_value(&typed).expect("serialize");
    assert_eq!(&back, value, "{what}: round trip changed the JSON");
    typed
}

#[test]
fn request_fixtures_round_trip() {
    let request: Request = round_trip(
        "get_broker.request.json",
        &fixture("get_broker.request.json"),
    );
    assert_eq!(request.command, Command::GetBroker {});
    assert_eq!(request.command.name(), "get_broker");
    assert!(!request.command.is_write());

    let request: Request = round_trip(
        "list_symbols.request.json",
        &fixture("list_symbols.request.json"),
    );
    assert_eq!(
        request.command,
        Command::ListSymbols(ListSymbolsQuery {
            offset: 0,
            limit: Some(3)
        })
    );
    assert_eq!(request.command.name(), "list_symbols");
    assert!(!request.command.is_write());
}

#[test]
fn get_broker_reply_fixture_round_trips_and_decodes() {
    let value = fixture("get_broker.reply.json");
    let reply: Reply = round_trip("get_broker.reply.json", &value);
    assert!(reply.ok && reply.retcode == 0);
    let info: BrokerInfo = round_trip("get_broker data", &value["data"]);
    assert_eq!(info.company, "CFI Financial Markets L.L.C");
    assert_eq!(info.server, "CFI3-Demo");
    assert_eq!(info.trade_mode, TradeMode::Demo);
    // Nothing that identifies the account is on the wire.
    let keys: Vec<&String> = value["data"].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["company", "server", "trade_mode"]);
}

#[test]
fn list_symbols_reply_fixture_round_trips_and_decodes() {
    let value = fixture("list_symbols.reply.json");
    let reply: Reply = round_trip("list_symbols.reply.json", &value);
    assert!(reply.ok && reply.retcode == 0);
    let page: SymbolCatalogPage = round_trip("list_symbols data", &value["data"]);
    assert_eq!(page.total, 1834);
    assert_eq!(page.offset, 0);
    assert_eq!(page.symbols.len(), 3);
    assert_eq!(page.symbols[0].name, "EURUSD");
    assert!(!page.symbols[0].visible);
    assert!(page.symbols[1].visible);
    // Backslashes in the path survive (they are `\\` in the JSON text), and so do quotes.
    assert_eq!(page.symbols[1].path, "FX Elite\\EURUSD_");
    assert_eq!(
        page.symbols[2].description,
        "Nasdaq 100 \"Sep 2026\" future (expired)"
    );
    assert_eq!(page.symbols[2].trade_mode, SymbolTradeMode::CloseOnly);
    assert_eq!(page.symbols[2].digits, 1);
    // A reply with `ok:false` decodes to a remote error, not a page.
    let failed: Reply = serde_json::from_value(
        json!({"id": "x", "ok": false, "error": "'limit' must be between 1 and 1000", "retcode": -1}),
    )
    .unwrap();
    assert!(matches!(
        failed.decode::<SymbolCatalogPage>(),
        Err(ReplyError::Remote { retcode: -1, .. })
    ));
}

#[test]
fn request_wire_format_matches_the_contract() {
    let wire = |command: Command| {
        serde_json::to_value(Request {
            id: "i".into(),
            ts: None,
            command,
        })
        .unwrap()
    };
    assert_eq!(
        wire(Command::GetBroker {}),
        json!({"id": "i", "cmd": "get_broker", "args": {}})
    );
    // `limit` is left out when not set, `offset` is always sent.
    assert_eq!(
        wire(Command::ListSymbols(ListSymbolsQuery {
            offset: 0,
            limit: None
        })),
        json!({"id": "i", "cmd": "list_symbols", "args": {"offset": 0}})
    );
    assert_eq!(
        wire(Command::ListSymbols(ListSymbolsQuery {
            offset: 1000,
            limit: Some(LIST_SYMBOLS_LIMIT_MAX)
        })),
        json!({"id": "i", "cmd": "list_symbols", "args": {"offset": 1000, "limit": 1000}})
    );
    // Both arguments are optional on the wire.
    let request: Request =
        serde_json::from_value(json!({"id": "i", "cmd": "list_symbols", "args": {}})).unwrap();
    assert_eq!(
        request.command,
        Command::ListSymbols(ListSymbolsQuery {
            offset: 0,
            limit: None
        })
    );
    assert_eq!(LIST_SYMBOLS_LIMIT_MAX, 1000);
}

#[test]
fn bad_list_symbols_arguments_are_rejected() {
    for args in [
        json!({"offset": -1}),
        json!({"offset": "0"}),
        json!({"offset": 0.5}),
        json!({"limit": -5}),
        json!({"limit": "10"}),
        json!({"limit": 1.5}),
    ] {
        let value = json!({"id": "x", "cmd": "list_symbols", "args": args});
        assert!(
            serde_json::from_value::<Request>(value.clone()).is_err(),
            "{value}"
        );
    }
}

#[test]
fn symbol_trade_modes_have_the_documented_wire_names() {
    for (mode, text) in [
        (SymbolTradeMode::Full, "full"),
        (SymbolTradeMode::LongOnly, "long_only"),
        (SymbolTradeMode::ShortOnly, "short_only"),
        (SymbolTradeMode::CloseOnly, "close_only"),
        (SymbolTradeMode::Disabled, "disabled"),
    ] {
        assert_eq!(serde_json::to_value(mode).unwrap(), json!(text));
        assert_eq!(
            serde_json::from_value::<SymbolTradeMode>(json!(text)).unwrap(),
            mode
        );
    }
    for bad in ["FULL", "long", "closed", "close-only", ""] {
        assert!(serde_json::from_value::<SymbolTradeMode>(json!(bad)).is_err());
    }
    let mut data = fixture("list_symbols.reply.json")["data"].clone();
    data["symbols"][0]["trade_mode"] = json!("tradable");
    assert!(serde_json::from_value::<SymbolCatalogPage>(data).is_err());
}

#[cfg(feature = "mock")]
mod against_the_mock {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use jarvis_bridge_client::mock::{MockBridge, MockState, default_catalog, generated_catalog};
    use jarvis_bridge_client::protocol::{
        BrokerInfo, CatalogSymbol, LIST_SYMBOLS_LIMIT_MAX, ListSymbolsQuery, SymbolTradeMode,
        TradeMode,
    };
    use jarvis_bridge_client::{BridgeClient, BridgeConfig, BridgeError, BridgeKey, auth};
    use serde_json::{Value, json};
    use zeromq::{RepSocket, Socket, SocketRecv, SocketSend};

    const TIMEOUT: Duration = Duration::from_secs(5);

    async fn started_with(state: MockState) -> (MockBridge, BridgeClient) {
        let mock = MockBridge::start_with(state).await;
        let client = BridgeClient::new(mock.config(TIMEOUT)).expect("client");
        (mock, client)
    }

    async fn started() -> (MockBridge, BridgeClient) {
        started_with(MockState::default()).await
    }

    fn state_with_catalog(count: usize) -> MockState {
        MockState {
            catalog: generated_catalog(count),
            ..MockState::default()
        }
    }

    fn find<'a>(symbols: &'a [CatalogSymbol], name: &str) -> &'a CatalogSymbol {
        symbols
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog"))
    }

    #[tokio::test]
    async fn get_broker_answers_company_and_server() {
        let (_mock, client) = started().await;
        let info = client.get_broker().await.unwrap();
        assert_eq!(info.company, "Mock Broker Ltd.");
        assert_eq!(info.server, "Mock-Demo");
        assert_eq!(info.trade_mode, TradeMode::Demo);
    }

    #[tokio::test]
    async fn set_broker_simulates_an_account_switch() {
        let (mock, client) = started().await;
        assert_eq!(client.get_broker().await.unwrap().server, "Mock-Demo");
        let before = client.list_all_symbols().await.unwrap();
        assert!(before.iter().any(|s| s.name == "XAUUSD_"));

        mock.set_broker(
            BrokerInfo {
                company: "Other Broker Inc.".into(),
                server: "Other-Live".into(),
                trade_mode: TradeMode::Real,
            },
            vec![CatalogSymbol {
                name: "GOLD".into(),
                description: "Gold".into(),
                path: "Metals\\GOLD".into(),
                trade_mode: SymbolTradeMode::Full,
                visible: true,
                digits: 2,
                currency_base: "XAU".into(),
                currency_profit: "USD".into(),
            }],
        );
        let info = client.get_broker().await.unwrap();
        assert_eq!(info.company, "Other Broker Inc.");
        assert_eq!(info.trade_mode, TradeMode::Real);
        let after = client.list_all_symbols().await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].name, "GOLD");
        // The state helper sees the same thing.
        assert_eq!(mock.state().broker.server, "Other-Live");
        assert_eq!(mock.state().catalog.len(), 1);
    }

    #[tokio::test]
    async fn default_catalog_is_cfi_like_and_sorted_by_name() {
        let (_mock, client) = started().await;
        let all = client.list_all_symbols().await.unwrap();
        assert_eq!(all.len(), default_catalog().len());
        assert!(all.windows(2).all(|w| w[0].name < w[1].name), "not sorted");

        // Both EURUSD sets, only the traded one visible.
        assert!(!find(&all, "EURUSD").visible);
        assert!(find(&all, "EURUSD_").visible);
        assert_eq!(find(&all, "EURUSD").description, "Euro vs US Dollar");
        // The instruments the symbol engine maps.
        for name in [
            "XAUUSD_",
            "XAGUSD_",
            "US100_Spot",
            "US30_SPOT",
            "US500_SPOT",
            "GER30_SPOT",
            "NAS1226",
            "GC1226",
            "DOWS1226",
            "BTCUSD",
            "ETHUSD",
            "WTI",
            "BRENT",
            "NatGas_SPOT",
            "AAPL",
        ] {
            find(&all, name);
        }
        assert_eq!(find(&all, "US100_Spot").description, "Nasdaq 100 - Cash");
        assert!(!find(&all, "NAS1226").visible);
        assert_eq!(find(&all, "NAS0926").trade_mode, SymbolTradeMode::CloseOnly);
        // Paths cross the wire with their backslashes.
        assert_eq!(find(&all, "XAUUSD_").path, "Metals\\XAUUSD_");
        assert!(all.iter().all(|s| s.path.contains('\\')));
        let mut names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
        names.dedup();
        assert_eq!(names.len(), all.len(), "duplicate names");
    }

    #[tokio::test]
    async fn pages_follow_offset_and_limit() {
        let (_mock, client) = started_with(state_with_catalog(120)).await;
        let page = client
            .list_symbols(ListSymbolsQuery {
                offset: 10,
                limit: Some(5),
            })
            .await
            .unwrap();
        assert_eq!(page.total, 120);
        assert_eq!(page.offset, 10);
        let names: Vec<&str> = page.symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            ["SYM00010", "SYM00011", "SYM00012", "SYM00013", "SYM00014"]
        );

        // Last, partial page.
        let page = client
            .list_symbols(ListSymbolsQuery {
                offset: 118,
                limit: Some(50),
            })
            .await
            .unwrap();
        assert_eq!(page.symbols.len(), 2);
        assert_eq!(page.total, 120);

        // At and past the end: an empty page, not an error; `offset` is echoed.
        for offset in [120, 5000] {
            let page = client
                .list_symbols(ListSymbolsQuery {
                    offset,
                    limit: Some(10),
                })
                .await
                .unwrap();
            assert!(page.symbols.is_empty());
            assert_eq!((page.total, page.offset), (120, offset));
        }
    }

    #[tokio::test]
    async fn the_default_page_is_500_and_the_limits_are_checked() {
        let (_mock, client) = started_with(state_with_catalog(1200)).await;
        let page = client
            .list_symbols(ListSymbolsQuery {
                offset: 0,
                limit: None,
            })
            .await
            .unwrap();
        assert_eq!(page.symbols.len(), 500);
        let page = client
            .list_symbols(ListSymbolsQuery {
                offset: 0,
                limit: Some(LIST_SYMBOLS_LIMIT_MAX),
            })
            .await
            .unwrap();
        assert_eq!(page.symbols.len(), 1000);
        for limit in [0, 1001, u32::MAX] {
            match client
                .list_symbols(ListSymbolsQuery {
                    offset: 0,
                    limit: Some(limit),
                })
                .await
            {
                Err(BridgeError::Remote { retcode, message }) => {
                    assert_eq!(retcode, -1, "limit {limit}");
                    assert!(message.contains("limit"), "{message}");
                }
                other => panic!("limit {limit}: expected Remote -1, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn list_all_symbols_reads_2345_symbols_in_three_pages() {
        let (mock, client) = started_with(state_with_catalog(2345)).await;
        let before = mock.request_count();
        let all = client.list_all_symbols().await.unwrap();
        assert_eq!(mock.request_count() - before, 3, "1000 + 1000 + 345");
        assert_eq!(all, generated_catalog(2345));
    }

    #[tokio::test]
    async fn list_all_symbols_handles_exact_multiples_and_tiny_catalogs() {
        for (count, requests) in [(0, 1), (1, 1), (999, 1), (1000, 1), (1001, 2), (2000, 2)] {
            let (mock, client) = started_with(state_with_catalog(count)).await;
            let before = mock.request_count();
            let all = client.list_all_symbols().await.unwrap();
            assert_eq!(all.len(), count, "{count} symbols");
            assert_eq!(mock.request_count() - before, requests, "{count} symbols");
        }
    }

    #[tokio::test]
    async fn a_12k_symbol_catalog_is_read_completely() {
        // MetaQuotes-Demo has about 12,224 symbols.
        let (mock, client) = started_with(state_with_catalog(12_224)).await;
        let before = mock.request_count();
        let started = Instant::now();
        let all = client.list_all_symbols().await.unwrap();
        let elapsed = started.elapsed();
        eprintln!(
            "12,224 symbols in {} requests took {elapsed:?}",
            mock.request_count() - before
        );
        assert_eq!(all.len(), 12_224);
        assert_eq!(mock.request_count() - before, 13);
        assert_eq!(all.first().unwrap().name, "SYM00000");
        assert_eq!(all.last().unwrap().name, "SYM12223");
        assert!(elapsed < Duration::from_secs(30), "took {elapsed:?}");
    }

    #[tokio::test]
    async fn a_timeout_on_a_page_is_returned_and_the_next_read_works() {
        let mock = MockBridge::start_with(state_with_catalog(2345)).await;
        let client = BridgeClient::new(mock.config(Duration::from_millis(300))).expect("client");
        mock.drop_next(1);
        assert!(matches!(
            client.list_all_symbols().await,
            Err(BridgeError::Timeout(_))
        ));
        assert_eq!(client.list_all_symbols().await.unwrap().len(), 2345);
    }

    // ---- a catalog that changes while it is read ---------------------------------------------

    /// What the scripted server answers to the n-th `list_symbols` request: the `total` it claims
    /// and how many symbols it really lists at the requested offset.
    #[derive(Clone, Copy)]
    struct Step {
        total: u32,
        /// Symbols returned, at most the requested limit; `None` = as many as exist.
        listed: Option<u32>,
    }

    fn step(total: u32) -> Step {
        Step {
            total,
            listed: None,
        }
    }

    fn catalog_entry(i: u32) -> Value {
        json!({
            "name": format!("SYM{i:05}"), "description": "d", "path": "p\\q",
            "trade_mode": "full", "visible": false, "digits": 2,
            "currency_base": "USD", "currency_profit": "USD"
        })
    }

    /// A server that answers the n-th `list_symbols` request according to `script` (the last
    /// step repeats) and counts the requests it served.
    async fn scripted_server(
        key: BridgeKey,
        script: Vec<Step>,
    ) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let served = Arc::new(AtomicUsize::new(0));
        let counter = served.clone();
        let mut socket = RepSocket::new();
        let endpoint = socket.bind("tcp://127.0.0.1:0").await.unwrap().to_string();
        let task = tokio::spawn(async move {
            loop {
                let Ok(request) = socket.recv().await else {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    continue;
                };
                let Ok(payload) = auth::open(&key, &request) else {
                    continue;
                };
                let Ok(json) = serde_json::from_slice::<Value>(payload) else {
                    continue;
                };
                let n = counter.fetch_add(1, Ordering::SeqCst);
                let step = script[n.min(script.len() - 1)];
                let offset = json["args"]["offset"].as_u64().unwrap_or(0) as u32;
                let limit = json["args"]["limit"].as_u64().unwrap_or(500) as u32;
                let available = step.total.saturating_sub(offset);
                let count = step.listed.unwrap_or(available).min(available).min(limit);
                let symbols: Vec<Value> = (offset..offset + count).map(catalog_entry).collect();
                let reply = json!({
                    "id": json["id"], "ok": true, "retcode": 0,
                    "data": {"symbols": symbols, "total": step.total, "offset": offset}
                });
                let _ = socket
                    .send(auth::seal(&key, serde_json::to_vec(&reply).unwrap()))
                    .await;
            }
        });
        (endpoint, served, task)
    }

    async fn scripted(
        script: Vec<Step>,
    ) -> (BridgeClient, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let key = BridgeKey::generate().unwrap();
        let (endpoint, served, task) = scripted_server(key.clone(), script).await;
        let client = BridgeClient::new(BridgeConfig::new(endpoint, TIMEOUT, key)).unwrap();
        (client, served, task)
    }

    #[tokio::test]
    async fn a_total_that_changes_mid_way_restarts_from_offset_zero() {
        // Page 1 of 2500, then page 2 reports 2501 (a symbol was added) -> restart, then stable.
        let (client, served, server) = scripted(vec![step(2500), step(2501), step(2501)]).await;
        let all = client.list_all_symbols().await.unwrap();
        assert_eq!(all.len(), 2501);
        assert!(
            all.iter()
                .enumerate()
                .all(|(i, s)| s.name == format!("SYM{i:05}"))
        );
        // 2 requests of the first pass, then 3 of the second.
        assert_eq!(served.load(Ordering::SeqCst), 5);
        server.abort();
    }

    #[tokio::test]
    async fn two_restarts_are_tolerated() {
        let (client, _served, server) = scripted(vec![
            step(2500),
            step(2400), // changed -> restart 1
            step(2300),
            step(2200), // changed -> restart 2
            step(2100), // from here on stable at 2100
        ])
        .await;
        // After the second restart the script repeats its last step (2100), so pass 3 succeeds.
        let all = client.list_all_symbols().await.unwrap();
        assert_eq!(all.len(), 2100);
        server.abort();
    }

    #[tokio::test]
    async fn a_catalog_that_keeps_changing_is_a_protocol_error() {
        // Every request reports a different total.
        let script: Vec<Step> = (0..40).map(|i| step(3000 + i)).collect();
        let (client, served, server) = scripted(script).await;
        match client.list_all_symbols().await {
            Err(BridgeError::Protocol(message)) => {
                assert!(message.contains("kept changing"), "{message}");
            }
            other => panic!("expected a Protocol error, got {other:?}"),
        }
        // Three passes, each stopping at its second page.
        assert_eq!(served.load(Ordering::SeqCst), 6);
        server.abort();
    }

    #[tokio::test]
    async fn an_empty_page_before_the_end_counts_as_a_change() {
        // Claims 1500 symbols but lists none on the second page, then a consistent 1000.
        let (client, _served, server) = scripted(vec![
            step(1500),
            Step {
                total: 1500,
                listed: Some(0),
            },
            step(1000),
        ])
        .await;
        let all = client.list_all_symbols().await.unwrap();
        assert_eq!(all.len(), 1000);
        server.abort();
    }

    #[tokio::test]
    async fn a_catalog_over_the_cap_is_refused() {
        let (client, served, server) = scripted(vec![step(100_001)]).await;
        match client.list_all_symbols().await {
            Err(BridgeError::Protocol(message)) => {
                assert!(message.contains("100000"), "{message}");
            }
            other => panic!("expected a Protocol error, got {other:?}"),
        }
        assert_eq!(served.load(Ordering::SeqCst), 1, "stops at the first page");
        server.abort();
    }

    #[tokio::test]
    async fn a_catalog_of_exactly_the_cap_is_accepted() {
        // 100,000 symbols = 100 pages; build them from the scripted server.
        let (client, served, server) = scripted(vec![step(100_000)]).await;
        let all = client.list_all_symbols().await.unwrap();
        assert_eq!(all.len(), 100_000);
        assert_eq!(served.load(Ordering::SeqCst), 100);
        server.abort();
    }

    #[tokio::test]
    async fn a_catalog_switched_between_pages_by_the_mock_is_noticed() {
        // The mock-driven version of the account switch: the second broker has fewer symbols.
        let (mock, client) = started_with(state_with_catalog(2345)).await;
        let first = client
            .list_symbols(ListSymbolsQuery {
                offset: 0,
                limit: Some(1000),
            })
            .await
            .unwrap();
        assert_eq!(first.total, 2345);
        mock.set_broker(
            BrokerInfo {
                company: "Other".into(),
                server: "Other-Demo".into(),
                trade_mode: TradeMode::Demo,
            },
            generated_catalog(1200),
        );
        let second = client
            .list_symbols(ListSymbolsQuery {
                offset: 1000,
                limit: Some(1000),
            })
            .await
            .unwrap();
        // The caller can tell: the total differs from the first page's.
        assert_eq!(second.total, 1200);
        assert_eq!(second.symbols.len(), 200);
        // And a full read after the switch is consistent.
        assert_eq!(client.list_all_symbols().await.unwrap().len(), 1200);
    }
}
