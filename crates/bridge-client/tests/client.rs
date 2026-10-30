//! Bridge contract tests: the Rust client against the in-process mock EA.
#![cfg(feature = "mock")]

use std::net::TcpListener;
use std::time::Duration;

use jarvis_bridge_client::mock::{MockBridge, MockState};
use jarvis_bridge_client::protocol::{DealType, HistoryQuery, Side, TradeMode};
use jarvis_bridge_client::{BridgeClient, BridgeConfig, BridgeError};
use serde_json::Value;
use tokio::task::JoinSet;
use zeromq::{RepSocket, Socket, SocketRecv, SocketSend, ZmqMessage};

const TIMEOUT: Duration = Duration::from_millis(300);

fn client_for(endpoint: &str) -> BridgeClient {
    BridgeClient::new(BridgeConfig {
        endpoint: endpoint.to_owned(),
        timeout: TIMEOUT,
    })
    .expect("client")
}

async fn started() -> (MockBridge, BridgeClient) {
    let mock = MockBridge::start().await;
    let client = client_for(&mock.endpoint);
    (mock, client)
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("addr").port()
}

#[tokio::test]
async fn ping_reports_protocol_version_and_demo_mode() {
    let (_mock, client) = started().await;
    let info = client.ping().await.unwrap();
    assert_eq!(info.protocol_version, 1);
    assert_eq!(info.trade_mode, TradeMode::Demo);
    assert!(info.terminal_connected);
}

#[tokio::test]
async fn get_price_returns_quote() {
    let (_mock, client) = started().await;
    for symbol in ["XAUUSD", "US100", "US30", "EURUSD", "GBPUSD", "BTCUSD"] {
        let price = client.get_price(symbol).await.unwrap();
        assert_eq!(price.symbol, symbol);
        assert!(
            price.ask >= price.bid && price.bid > 0.0,
            "{symbol}: {price:?}"
        );
    }
}

#[tokio::test]
async fn get_account_returns_demo_account() {
    let (_mock, client) = started().await;
    let account = client.get_account().await.unwrap();
    assert_eq!(account.currency, "USD");
    assert!((account.balance - 10000.0).abs() < f64::EPSILON);
    assert_eq!(account.trade_mode, TradeMode::Demo);
}

#[tokio::test]
async fn get_positions_all_and_filtered() {
    let (_mock, client) = started().await;
    let all = client.get_positions(None).await.unwrap();
    assert_eq!(all.positions.len(), 2);

    let gold = client.get_positions(Some("XAUUSD")).await.unwrap();
    assert_eq!(gold.positions.len(), 1);
    assert_eq!(gold.positions[0].side, Side::Buy);
    assert!(gold.positions[0].sl.is_none() && gold.positions[0].tp.is_some());

    let none = client.get_positions(Some("EURUSD")).await.unwrap();
    assert!(none.positions.is_empty());
}

#[tokio::test]
async fn get_orders_all_and_filtered() {
    let (_mock, client) = started().await;
    assert_eq!(client.get_orders(None).await.unwrap().orders.len(), 1);
    assert_eq!(
        client
            .get_orders(Some("EURUSD"))
            .await
            .unwrap()
            .orders
            .len(),
        1
    );
    assert!(
        client
            .get_orders(Some("XAUUSD"))
            .await
            .unwrap()
            .orders
            .is_empty()
    );
}

#[tokio::test]
async fn get_history_filters_by_range_symbol_and_limit() {
    let state = MockState::default();
    let (first, last) = (state.deals[0].time, state.deals[state.deals.len() - 1].time);
    let total = state.deals.len();
    let mock = MockBridge::start_with(state).await;
    let client = client_for(&mock.endpoint);

    let all = client
        .get_history(HistoryQuery {
            from: first,
            to: last,
            symbol: None,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(all.deals.len(), total);
    assert!(!all.truncated);
    assert_eq!(all.deals[total - 1].deal_type, DealType::Balance);
    assert!(
        all.deals.windows(2).all(|w| w[0].time >= w[1].time),
        "newest first"
    );

    let limited = client
        .get_history(HistoryQuery {
            from: first,
            to: last,
            symbol: None,
            limit: Some(2),
        })
        .await
        .unwrap();
    assert_eq!(limited.deals.len(), 2);
    assert!(limited.truncated);

    let eur = client
        .get_history(HistoryQuery {
            from: first,
            to: last,
            symbol: Some("EURUSD".into()),
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(eur.deals.len(), 2);
    assert!(eur.deals.iter().all(|d| d.symbol == "EURUSD"));

    let narrow = client
        .get_history(HistoryQuery {
            from: last,
            to: last,
            symbol: None,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(narrow.deals.len(), 1);
}

#[tokio::test]
async fn invalid_history_arguments_are_remote_errors() {
    let (_mock, client) = started().await;
    for query in [
        HistoryQuery {
            from: 10,
            to: 5,
            symbol: None,
            limit: None,
        },
        HistoryQuery {
            from: 1,
            to: 5,
            symbol: None,
            limit: Some(0),
        },
        HistoryQuery {
            from: 1,
            to: 5,
            symbol: None,
            limit: Some(501),
        },
    ] {
        match client.get_history(query).await {
            Err(BridgeError::Remote { retcode: -1, .. }) => {}
            other => panic!("expected retcode -1, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn unknown_symbol_maps_to_remote_error_4301() {
    let (_mock, client) = started().await;
    match client.get_price("XAUUS").await {
        Err(BridgeError::Remote { message, retcode }) => {
            assert_eq!(retcode, 4301);
            assert_eq!(message, "unknown symbol: XAUUS");
        }
        other => panic!("expected Remote error, got {other:?}"),
    }
    // A remote error is a normal reply; the next call works on the same socket.
    assert!(client.get_price("XAUUSD").await.is_ok());
    assert!(matches!(
        client.get_positions(Some("NOPE")).await,
        Err(BridgeError::Remote { retcode: 4301, .. })
    ));
}

#[tokio::test]
async fn timeout_then_recovery_without_recreating_the_client() {
    let (mock, client) = started().await;
    assert!(client.ping().await.is_ok());

    mock.drop_next(1);
    match client.get_price("XAUUSD").await {
        Err(BridgeError::Timeout(ms)) => assert_eq!(ms, 300),
        other => panic!("expected Timeout, got {other:?}"),
    }
    // Same client: the actor recreated its socket, so lock-step is not wedged.
    assert!(client.get_price("XAUUSD").await.is_ok());
    assert!(client.ping().await.is_ok());
    assert_eq!(mock.request_count(), 4);
}

#[tokio::test]
async fn several_consecutive_timeouts_then_recovery() {
    let (mock, client) = started().await;
    mock.drop_next(2);
    assert!(matches!(client.ping().await, Err(BridgeError::Timeout(_))));
    assert!(matches!(client.ping().await, Err(BridgeError::Timeout(_))));
    assert!(client.ping().await.is_ok());
}

#[tokio::test]
async fn non_loopback_endpoints_are_rejected() {
    for endpoint in [
        "tcp://192.168.1.10:5555",
        "tcp://0.0.0.0:5555",
        "tcp://example.com:5555",
        "tcp://127.0.0.2:5555",
        "tcp://127.0.0.1.evil.com:5555",
        "tcp://127.0.0.1",
        "tcp://127.0.0.1:0",
        "tcp://127.0.0.1:notaport",
        "tcp://[::2]:5555",
        "ipc:///tmp/jarvis",
        "inproc://jarvis",
        "127.0.0.1:5555",
        "",
    ] {
        let result = BridgeClient::new(BridgeConfig {
            endpoint: endpoint.to_owned(),
            timeout: TIMEOUT,
        });
        assert!(
            matches!(result, Err(BridgeError::Transport(_))),
            "{endpoint:?} should be rejected, got {result:?}"
        );
    }
}

#[tokio::test]
async fn loopback_endpoint_spellings_are_accepted() {
    for endpoint in [
        "tcp://127.0.0.1:5555",
        "tcp://localhost:5555",
        "tcp://[::1]:5555",
    ] {
        let result = BridgeClient::new(BridgeConfig {
            endpoint: endpoint.to_owned(),
            timeout: TIMEOUT,
        });
        assert!(
            result.is_ok(),
            "{endpoint:?} should be accepted, got {result:?}"
        );
    }
}

#[test]
fn new_outside_a_runtime_is_an_error_not_a_panic() {
    let result = BridgeClient::new(BridgeConfig::default());
    assert!(
        matches!(result, Err(BridgeError::Transport(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn client_created_before_the_bridge_starts_connects_lazily() {
    let port = free_port();
    let endpoint = format!("tcp://127.0.0.1:{port}");
    let client = client_for(&endpoint);

    // Nothing is listening yet: an error, not a hang or a panic.
    let err = client.ping().await.unwrap_err();
    assert!(
        matches!(err, BridgeError::Transport(_) | BridgeError::Timeout(_)),
        "{err:?}"
    );

    let mock = MockBridge::start_at(&endpoint, MockState::default())
        .await
        .unwrap();
    assert!(client.ping().await.is_ok());
    assert_eq!(mock.request_count(), 1);
}

#[tokio::test]
async fn client_reconnects_after_the_bridge_restarts() {
    let port = free_port();
    let endpoint = format!("tcp://127.0.0.1:{port}");
    let client = client_for(&endpoint);

    let mock = MockBridge::start_at(&endpoint, MockState::default())
        .await
        .unwrap();
    assert!(client.ping().await.is_ok());
    mock.stop().await;

    assert!(client.ping().await.is_err());

    let mock = MockBridge::start_at(&endpoint, MockState::default())
        .await
        .unwrap();
    assert!(client.ping().await.is_ok());
    assert!(client.get_account().await.is_ok());
    assert_eq!(mock.request_count(), 2);
}

#[tokio::test]
async fn concurrent_calls_from_clones_are_serialised_and_all_succeed() {
    let (mock, client) = {
        let mock = MockBridge::start().await;
        // Generous timeout: this test is about ordering, not speed.
        let client = BridgeClient::new(BridgeConfig {
            endpoint: mock.endpoint.clone(),
            timeout: Duration::from_secs(5),
        })
        .unwrap();
        (mock, client)
    };
    let mut tasks = JoinSet::new();
    for i in 0..8 {
        let client = client.clone();
        tasks.spawn(async move {
            for round in 0..5 {
                let symbol = ["XAUUSD", "US100", "EURUSD"][(i + round) % 3];
                let price = client.get_price(symbol).await.expect("price");
                assert_eq!(price.symbol, symbol, "reply must belong to this request");
                client.get_positions(None).await.expect("positions");
            }
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.expect("task panicked");
    }
    assert_eq!(mock.request_count(), 8 * 5 * 2);
}

#[tokio::test]
async fn cancelled_call_does_not_wedge_the_socket() {
    let (mock, client) = started().await;
    assert!(client.ping().await.is_ok());
    mock.drop_next(1);
    // The caller gives up long before the client's own timeout fires.
    let cancelled = tokio::time::timeout(Duration::from_millis(50), client.ping()).await;
    assert!(cancelled.is_err());
    // The actor is still finishing the abandoned exchange (it times out), then recovers.
    let next = client.ping().await;
    assert!(
        next.is_ok() || matches!(next, Err(BridgeError::Timeout(_))),
        "{next:?}"
    );
    assert!(client.ping().await.is_ok());
}

/// A server that answers every request with the reply produced by `make_reply(request_json)`.
async fn rogue_server(
    make_reply: impl Fn(Value) -> Vec<u8> + Send + 'static,
) -> (String, tokio::task::JoinHandle<()>) {
    let mut socket = RepSocket::new();
    let endpoint = socket.bind("tcp://127.0.0.1:0").await.unwrap().to_string();
    let task = tokio::spawn(async move {
        loop {
            let Ok(request) = socket.recv().await else {
                tokio::time::sleep(Duration::from_millis(5)).await;
                continue;
            };
            let Some(frame) = request.get(0) else {
                continue;
            };
            let Ok(json) = serde_json::from_slice::<Value>(frame) else {
                continue;
            };
            let _ = socket.send(ZmqMessage::from(make_reply(json))).await;
        }
    });
    (endpoint, task)
}

#[tokio::test]
async fn reply_with_wrong_id_is_a_protocol_error() {
    let (endpoint, server) = rogue_server(|_request| {
        br#"{"id":"not-the-request-id","ok":true,"data":{},"retcode":0}"#.to_vec()
    })
    .await;
    let client = client_for(&endpoint);
    match client.ping().await {
        Err(BridgeError::Protocol(m)) => assert!(m.contains("does not match"), "{m}"),
        other => panic!("expected Protocol error, got {other:?}"),
    }
    server.abort();
}

#[tokio::test]
async fn garbage_reply_is_a_protocol_error_and_the_client_recovers() {
    // First reply is garbage, afterwards the server behaves.
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let (endpoint, server) = rogue_server(move |request| {
        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            return b"this is not json".to_vec();
        }
        let reply = serde_json::json!({
            "id": request["id"], "ok": true, "retcode": 0,
            "data": {"protocol_version": 1, "ea_version": "rogue", "trade_mode": "demo",
                     "terminal_connected": true, "trade_allowed": true, "server_time": 1}
        });
        serde_json::to_vec(&reply).unwrap()
    })
    .await;
    let client = client_for(&endpoint);
    assert!(matches!(client.ping().await, Err(BridgeError::Protocol(_))));
    assert_eq!(client.ping().await.unwrap().ea_version, "rogue");
    server.abort();
}

#[tokio::test]
async fn reply_data_of_the_wrong_shape_is_a_protocol_error() {
    let (endpoint, server) = rogue_server(|request| {
        serde_json::to_vec(&serde_json::json!({
            "id": request["id"], "ok": true, "data": {"unexpected": true}, "retcode": 0
        }))
        .unwrap()
    })
    .await;
    let client = client_for(&endpoint);
    assert!(matches!(
        client.get_account().await,
        Err(BridgeError::Protocol(_))
    ));
    server.abort();
}

#[tokio::test]
async fn mock_answers_malformed_and_unknown_requests_with_error_replies() {
    // Talk to the mock with a raw REQ socket to check its error handling.
    use zeromq::ReqSocket;
    let mock = MockBridge::start().await;
    let mut socket = ReqSocket::new();
    socket.connect(&mock.endpoint).await.unwrap();
    for (request, retcode) in [
        (r#"{"id":"1","cmd":"order_send","args":{}}"#, -1), // known command, invalid arguments
        (r#"{"id":"2","cmd":"nope","args":{}}"#, -2),
        (r#"{"id":"3","cmd":"get_price","args":{}}"#, -1),
        (r#"{"cmd":"ping","args":{}}"#, -1),
        ("not json at all", -1),
    ] {
        socket.send(ZmqMessage::from(request)).await.unwrap();
        let reply = socket.recv().await.unwrap();
        let reply: Value = serde_json::from_slice(reply.get(0).unwrap()).unwrap();
        assert_eq!(reply["ok"], false, "{request}");
        assert_eq!(reply["retcode"], retcode, "{request}: {reply}");
        assert!(reply["error"].as_str().is_some_and(|e| !e.is_empty()));
    }
}
