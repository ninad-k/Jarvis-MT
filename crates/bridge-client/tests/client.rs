//! Bridge contract tests: the Rust client against the in-process mock EA.
#![cfg(feature = "mock")]

use std::net::TcpListener;
use std::time::Duration;

use jarvis_bridge_client::auth;
use jarvis_bridge_client::mock::{MockBridge, MockState};
use jarvis_bridge_client::protocol::{DealType, HistoryQuery, Side, TradeMode};
use jarvis_bridge_client::{BridgeClient, BridgeConfig, BridgeError, BridgeKey};
use serde_json::Value;
use tokio::task::JoinSet;
use zeromq::{RepSocket, Socket, SocketRecv, SocketSend};

const TIMEOUT: Duration = Duration::from_millis(300);

fn new_key() -> BridgeKey {
    BridgeKey::generate().expect("key")
}

fn client_for(endpoint: &str, key: &BridgeKey) -> BridgeClient {
    BridgeClient::new(BridgeConfig::new(endpoint, TIMEOUT, key.clone())).expect("client")
}

async fn started() -> (MockBridge, BridgeClient) {
    let mock = MockBridge::start().await;
    let client = client_for(&mock.endpoint, &mock.key());
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
    assert_eq!(info.protocol_version, 2);
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
    let client = client_for(&mock.endpoint, &mock.key());

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
        let result = BridgeClient::new(BridgeConfig::new(endpoint, TIMEOUT, new_key()));
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
        let result = BridgeClient::new(BridgeConfig::new(endpoint, TIMEOUT, new_key()));
        assert!(
            result.is_ok(),
            "{endpoint:?} should be accepted, got {result:?}"
        );
    }
}

#[test]
fn new_outside_a_runtime_is_an_error_not_a_panic() {
    let result = BridgeClient::new(BridgeConfig::with_key(new_key()));
    assert!(
        matches!(result, Err(BridgeError::Transport(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn client_created_before_the_bridge_starts_connects_lazily() {
    let port = free_port();
    let endpoint = format!("tcp://127.0.0.1:{port}");
    let key = new_key();
    let client = client_for(&endpoint, &key);

    // Nothing is listening yet: an error, not a hang or a panic.
    let err = client.ping().await.unwrap_err();
    assert!(
        matches!(err, BridgeError::Transport(_) | BridgeError::Timeout(_)),
        "{err:?}"
    );

    let mock = MockBridge::start_at_with_key(&endpoint, MockState::default(), key)
        .await
        .unwrap();
    assert!(client.ping().await.is_ok());
    assert_eq!(mock.request_count(), 1);
}

#[tokio::test]
async fn client_reconnects_after_the_bridge_restarts() {
    let port = free_port();
    let endpoint = format!("tcp://127.0.0.1:{port}");
    let key = new_key();
    let client = client_for(&endpoint, &key);

    let mock = MockBridge::start_at_with_key(&endpoint, MockState::default(), key.clone())
        .await
        .unwrap();
    assert!(client.ping().await.is_ok());
    mock.stop().await;

    assert!(client.ping().await.is_err());

    let mock = MockBridge::start_at_with_key(&endpoint, MockState::default(), key)
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
        let client = BridgeClient::new(mock.config(Duration::from_secs(5))).unwrap();
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

/// A server that holds `key` and answers every authentic request with the payload produced by
/// `make_reply(request_json)`, signed correctly.
async fn rogue_server(
    key: BridgeKey,
    make_reply: impl Fn(Value) -> Vec<u8> + Send + 'static,
) -> (String, tokio::task::JoinHandle<()>) {
    rogue_server_with(key.clone(), move |request| {
        Some(auth::seal(&key, make_reply(request)))
    })
    .await
}

/// A server that answers with whatever message `make_reply` builds (None = no reply).
async fn rogue_server_with(
    key: BridgeKey,
    make_reply: impl Fn(Value) -> Option<zeromq::ZmqMessage> + Send + 'static,
) -> (String, tokio::task::JoinHandle<()>) {
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
            if let Some(reply) = make_reply(json) {
                let _ = socket.send(reply).await;
            }
        }
    });
    (endpoint, task)
}

#[tokio::test]
async fn reply_with_wrong_id_is_a_protocol_error() {
    let key = new_key();
    let (endpoint, server) = rogue_server(key.clone(), |_request| {
        br#"{"id":"not-the-request-id","ok":true,"data":{},"retcode":0}"#.to_vec()
    })
    .await;
    let client = client_for(&endpoint, &key);
    match client.ping().await {
        Err(BridgeError::Protocol(m)) => assert!(m.contains("does not match"), "{m}"),
        other => panic!("expected Protocol error, got {other:?}"),
    }
    server.abort();
}

#[tokio::test]
async fn garbage_reply_is_a_protocol_error_and_the_client_recovers() {
    // First reply is garbage (correctly signed), afterwards the server behaves.
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let key = new_key();
    let (endpoint, server) = rogue_server(key.clone(), move |request| {
        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            return b"this is not json".to_vec();
        }
        let reply = serde_json::json!({
            "id": request["id"], "ok": true, "retcode": 0,
            "data": {"protocol_version": 2, "ea_version": "rogue", "trade_mode": "demo",
                     "terminal_connected": true, "trade_allowed": true, "server_time": 1}
        });
        serde_json::to_vec(&reply).unwrap()
    })
    .await;
    let client = client_for(&endpoint, &key);
    assert!(matches!(client.ping().await, Err(BridgeError::Protocol(_))));
    assert_eq!(client.ping().await.unwrap().ea_version, "rogue");
    server.abort();
}

#[tokio::test]
async fn reply_data_of_the_wrong_shape_is_a_protocol_error() {
    let key = new_key();
    let (endpoint, server) = rogue_server(key.clone(), |request| {
        serde_json::to_vec(&serde_json::json!({
            "id": request["id"], "ok": true, "data": {"unexpected": true}, "retcode": 0
        }))
        .unwrap()
    })
    .await;
    let client = client_for(&endpoint, &key);
    assert!(matches!(
        client.get_account().await,
        Err(BridgeError::Protocol(_))
    ));
    server.abort();
}

#[tokio::test]
async fn ping_refuses_an_ea_that_speaks_another_protocol_version() {
    let key = new_key();
    let (endpoint, server) = rogue_server(key.clone(), |request| {
        serde_json::to_vec(&serde_json::json!({
            "id": request["id"], "ok": true, "retcode": 0,
            "data": {"protocol_version": 1, "ea_version": "0.2.0", "trade_mode": "demo",
                     "terminal_connected": true, "trade_allowed": true, "server_time": 1}
        }))
        .unwrap()
    })
    .await;
    let client = client_for(&endpoint, &key);
    match client.ping().await {
        Err(BridgeError::Protocol(m)) => {
            assert!(
                m.contains("protocol version 1") && m.contains("install-ea"),
                "{m}"
            );
        }
        other => panic!("expected a version error, got {other:?}"),
    }
    server.abort();
}

// ---- Authentication (BR-1, ADR-0016) -------------------------------------------------------------

/// Sign `payload` with `key` and send it as a raw request; return the reply message.
async fn raw_exchange(endpoint: &str, message: zeromq::ZmqMessage) -> zeromq::ZmqMessage {
    use zeromq::ReqSocket;
    let mut socket = ReqSocket::new();
    socket.connect(endpoint).await.unwrap();
    socket.send(message).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), socket.recv())
        .await
        .expect("the mock must answer every request exactly once")
        .unwrap()
}

fn request_json(id: &str, ts: i64, cmd: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"id": id, "ts": ts, "cmd": cmd, "args": {}})).unwrap()
}

/// Decode a reply the way a client would: signature first, then JSON.
fn open_reply(key: &BridgeKey, reply: &zeromq::ZmqMessage) -> Value {
    let payload = auth::open(key, reply).expect("the reply must be signed with the shared key");
    serde_json::from_slice(payload).unwrap()
}

fn assert_auth_failed(reply: &Value) {
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(reply["retcode"], -4, "{reply}");
    assert_eq!(reply["error"], "authentication failed", "{reply}");
    assert!(reply.get("data").is_none());
}

#[tokio::test]
async fn a_request_signed_with_the_wrong_key_is_refused_and_nothing_runs() {
    let mock = MockBridge::start().await;
    let attacker = new_key();
    let order = serde_json::to_vec(&serde_json::json!({
        "id": "evil-1", "ts": auth::now_ms(), "cmd": "order_send",
        "args": {"symbol": "XAUUSD", "side": "buy", "volume": 0.1, "type": "market",
                 "magic": 1, "comment": "evil"}
    }))
    .unwrap();
    let reply = raw_exchange(&mock.endpoint, auth::seal(&attacker, order)).await;
    // The refusal is signed with the EA's key (the attacker cannot read it), carries no id.
    assert!(auth::open(&attacker, &reply).is_err());
    let body = open_reply(&mock.key(), &reply);
    assert_auth_failed(&body);
    assert_eq!(body["id"], "", "an unauthenticated id must not be echoed");
    assert_eq!(mock.executed_writes(), 0);
    assert_eq!(mock.state().positions.len(), 2, "no position was opened");
    assert_eq!(mock.auth_failures(), 1);
}

#[tokio::test]
async fn an_unsigned_single_frame_request_is_refused_and_nothing_runs() {
    let mock = MockBridge::start().await;
    let plain = request_json("plain-1", auth::now_ms(), "ping");
    let reply = raw_exchange(&mock.endpoint, zeromq::ZmqMessage::from(plain)).await;
    assert_auth_failed(&open_reply(&mock.key(), &reply));

    // Three frames are refused too, even when the first two are a valid pair.
    let mut three =
        auth::seal(&mock.key(), request_json("plain-2", auth::now_ms(), "ping")).into_vec();
    three.push(bytes::Bytes::from_static(b"extra"));
    let reply = raw_exchange(&mock.endpoint, zeromq::ZmqMessage::try_from(three).unwrap()).await;
    assert_auth_failed(&open_reply(&mock.key(), &reply));
    assert_eq!(mock.auth_failures(), 2);
}

#[tokio::test]
async fn a_tampered_payload_or_mac_is_refused() {
    let mock = MockBridge::start().await;
    let key = mock.key();
    let good = request_json("tamper-1", auth::now_ms(), "get_account");
    let mut frames = auth::seal(&key, good.clone()).into_vec();

    // Payload changed after signing.
    let mut changed = good.clone();
    let pos = changed
        .windows(11)
        .position(|w| w == b"get_account")
        .unwrap();
    changed[pos..pos + 11].copy_from_slice(b"get_account");
    changed[pos + 4] = b'A'; // get_Account
    let message = zeromq::ZmqMessage::try_from(vec![frames[0].clone(), changed.into()]).unwrap();
    assert_auth_failed(&open_reply(
        &key,
        &raw_exchange(&mock.endpoint, message).await,
    ));

    // One hex digit of the MAC flipped.
    let mut mac = frames[0].to_vec();
    mac[0] = if mac[0] == b'0' { b'1' } else { b'0' };
    frames[0] = mac.into();
    let message = zeromq::ZmqMessage::try_from(frames.clone()).unwrap();
    assert_auth_failed(&open_reply(
        &key,
        &raw_exchange(&mock.endpoint, message).await,
    ));

    // Uppercase hex, truncated and empty MACs.
    let upper = frames[1].clone();
    for bad in [
        bytes::Bytes::from(key.sign(&upper).to_uppercase()),
        bytes::Bytes::from(key.sign(&upper)[..60].to_owned()),
        bytes::Bytes::new(),
    ] {
        let message = zeromq::ZmqMessage::try_from(vec![bad, upper.clone()]).unwrap();
        assert_auth_failed(&open_reply(
            &key,
            &raw_exchange(&mock.endpoint, message).await,
        ));
    }

    // The untouched request still works (the refusals did not consume its id).
    let reply = raw_exchange(&mock.endpoint, auth::seal(&key, good)).await;
    assert_eq!(open_reply(&key, &reply)["ok"], true);
}

#[tokio::test]
async fn a_replayed_request_id_is_refused() {
    let (mock, _client) = started().await;
    let key = mock.key();
    let ts = auth::now_ms();
    let order = serde_json::to_vec(&serde_json::json!({
        "id": "replay-1", "ts": ts, "cmd": "order_send",
        "args": {"symbol": "XAUUSD", "side": "buy", "volume": 0.1, "type": "market",
                 "magic": 770077, "comment": "jarvis:replay"}
    }))
    .unwrap();
    let first = raw_exchange(&mock.endpoint, auth::seal(&key, order.clone())).await;
    assert_eq!(open_reply(&key, &first)["ok"], true);
    assert_eq!(mock.executed_writes(), 1);

    // The very same signed bytes again: refused, not even answered from the reply cache.
    let again = raw_exchange(&mock.endpoint, auth::seal(&key, order)).await;
    let body = open_reply(&key, &again);
    assert_auth_failed(&body);
    assert_eq!(body["id"], "replay-1", "an authentic peer gets its id back");
    assert_eq!(mock.executed_writes(), 1, "the replay opened nothing");
    assert_eq!(mock.cache_hits(), 0);
    assert_eq!(mock.state().positions.len(), 3);
}

#[tokio::test]
async fn a_stale_or_missing_timestamp_is_refused() {
    let mock = MockBridge::start().await;
    let key = mock.key();
    let now = auth::now_ms();
    for (i, ts) in [now - 31_000, now + 31_000, now - 3_600_000, 0, i64::MAX]
        .into_iter()
        .enumerate()
    {
        let id = format!("stale-{i}");
        let reply = raw_exchange(
            &mock.endpoint,
            auth::seal(&key, request_json(&id, ts, "get_account")),
        )
        .await;
        let body = open_reply(&key, &reply);
        assert_auth_failed(&body);
        assert_eq!(body["id"], id.as_str());
    }
    // No ts at all, and a ts that is not an integer.
    for (i, payload) in [
        serde_json::json!({"id": "nots-1", "cmd": "get_account", "args": {}}),
        serde_json::json!({"id": "nots-2", "ts": "now", "cmd": "get_account", "args": {}}),
        serde_json::json!({"id": "nots-3", "ts": 1.5, "cmd": "get_account", "args": {}}),
    ]
    .into_iter()
    .enumerate()
    {
        let reply = raw_exchange(
            &mock.endpoint,
            auth::seal(&key, serde_json::to_vec(&payload).unwrap()),
        )
        .await;
        assert_auth_failed(&open_reply(&key, &reply));
        let _ = i;
    }
    // Inside the window (25 s off either way) is accepted.
    for (i, ts) in [now - 25_000, now + 25_000].into_iter().enumerate() {
        let reply = raw_exchange(
            &mock.endpoint,
            auth::seal(&key, request_json(&format!("fresh-{i}"), ts, "get_account")),
        )
        .await;
        assert_eq!(open_reply(&key, &reply)["ok"], true);
    }
}

#[tokio::test]
async fn a_client_with_the_wrong_key_cannot_use_the_bridge() {
    let mock = MockBridge::start().await;
    let client = client_for(&mock.endpoint, &new_key());
    // The refusal is signed with the EA's key, which this client does not have, so the client
    // cannot even tell what the reply says: it is an authentication failure of the reply.
    match client.ping().await {
        Err(BridgeError::Protocol(m)) => assert_eq!(m, "reply authentication failed"),
        other => panic!("expected a reply authentication failure, got {other:?}"),
    }
    assert!(client.get_positions(None).await.is_err());
    assert_eq!(mock.executed_writes(), 0);
    assert_eq!(mock.auth_failures(), 2);
}

#[tokio::test]
async fn a_client_whose_clock_is_off_gets_a_clear_refusal() {
    // Talk to the mock through a raw socket with a skewed clock, as the client would.
    let mock = MockBridge::start().await;
    let key = mock.key();
    let reply = raw_exchange(
        &mock.endpoint,
        auth::seal(
            &key,
            request_json("skew-1", auth::now_ms() - 120_000, "ping"),
        ),
    )
    .await;
    let body = open_reply(&key, &reply);
    assert_auth_failed(&body);
    // And the typed client turns a signed -4 into a readable remote error.
    let (endpoint, server) = rogue_server(key.clone(), |request| {
        serde_json::to_vec(&serde_json::json!({
            "id": request["id"], "ok": false, "error": "authentication failed", "retcode": -4
        }))
        .unwrap()
    })
    .await;
    match client_for(&endpoint, &key).ping().await {
        Err(BridgeError::Remote { message, retcode }) => {
            assert_eq!(retcode, -4);
            assert!(message.starts_with("authentication failed"), "{message}");
            assert!(message.contains("clock"), "{message}");
        }
        other => panic!("expected a remote -4, got {other:?}"),
    }
    server.abort();
}

#[tokio::test]
async fn a_forged_reply_is_detected_and_the_socket_is_reset() {
    let key = new_key();
    let impostor = new_key();
    let calls = std::sync::atomic::AtomicUsize::new(0);
    // Squatter on the port: answers with a perfectly shaped reply, signed with another key,
    // then (second call) with an unsigned single frame, then (third) with a good reply.
    let (endpoint, server) = {
        let ok_key = key.clone();
        let bad_key = impostor.clone();
        rogue_server_with(key.clone(), move |request| {
            let reply = serde_json::to_vec(&serde_json::json!({
                "id": request["id"], "ok": true, "retcode": 0,
                "data": {"protocol_version": 2, "ea_version": "forged", "trade_mode": "demo",
                         "terminal_connected": true, "trade_allowed": true, "server_time": 1}
            }))
            .unwrap();
            match calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0 => Some(auth::seal(&bad_key, reply)),
                1 => Some(zeromq::ZmqMessage::from(reply)),
                _ => Some(auth::seal(&ok_key, reply)),
            }
        })
        .await
    };
    let client = client_for(&endpoint, &key);
    match client.ping().await {
        Err(BridgeError::Protocol(m)) => assert_eq!(m, "reply authentication failed"),
        other => panic!("forged reply accepted or wrong error: {other:?}"),
    }
    match client.ping().await {
        Err(BridgeError::Protocol(m)) => assert!(m.contains("two-frame"), "{m}"),
        other => panic!("unsigned reply accepted or wrong error: {other:?}"),
    }
    // The client recovers on a fresh socket once a genuine peer answers.
    assert_eq!(client.ping().await.unwrap().ea_version, "forged");
    server.abort();
}

#[tokio::test]
async fn a_tampered_reply_payload_is_detected() {
    let key = new_key();
    let (endpoint, server) = rogue_server_with(key.clone(), {
        let key = key.clone();
        move |request| {
            let reply = serde_json::to_vec(&serde_json::json!({
                "id": request["id"], "ok": true, "retcode": 0,
                "data": {"currency": "USD", "balance": 10.0, "equity": 10.0, "profit": 0.0,
                         "margin": 0.0, "free_margin": 10.0, "margin_level": 0.0,
                         "leverage": 100, "trade_mode": "demo"}
            }))
            .unwrap();
            let mut frames = auth::seal(&key, reply.clone()).into_vec();
            // Inflate the balance after signing.
            let text = String::from_utf8(reply).unwrap().replace("10.0", "99999.0");
            frames[1] = text.into_bytes().into();
            Some(zeromq::ZmqMessage::try_from(frames).unwrap())
        }
    })
    .await;
    let client = client_for(&endpoint, &key);
    match client.get_account().await {
        Err(BridgeError::Protocol(m)) => assert_eq!(m, "reply authentication failed"),
        other => panic!("tampered reply accepted: {other:?}"),
    }
    server.abort();
}

#[tokio::test]
async fn an_old_unsigned_ea_is_reported_as_such() {
    // A protocol-1 EA reads frame 1 (our MAC) as the request and answers with one plain frame.
    let mut socket = RepSocket::new();
    let endpoint = socket.bind("tcp://127.0.0.1:0").await.unwrap().to_string();
    let server = tokio::spawn(async move {
        loop {
            if socket.recv().await.is_ok() {
                let _ = socket
                    .send(zeromq::ZmqMessage::from(
                        r#"{"id":"","ok":false,"error":"malformed request","retcode":-1}"#,
                    ))
                    .await;
            }
        }
    });
    match client_for(&endpoint, &new_key()).ping().await {
        Err(BridgeError::Protocol(m)) => {
            assert!(
                m.contains("1 frame") && m.contains("older than protocol"),
                "{m}"
            );
        }
        other => panic!("expected a protocol error, got {other:?}"),
    }
    server.abort();
}

#[tokio::test]
async fn a_signed_write_goes_through_end_to_end() {
    use jarvis_bridge_client::protocol::{ExecType, JARVIS_MAGIC, OrderSendArgs};
    let (mock, client) = started().await;
    let before = mock.state().positions.len();
    let result = client
        .order_send(OrderSendArgs {
            symbol: "XAUUSD".into(),
            side: Side::Buy,
            volume: 0.1,
            exec: ExecType::Market,
            price: None,
            sl: None,
            tp: None,
            deviation_points: None,
            magic: JARVIS_MAGIC,
            comment: "jarvis:auth".into(),
        })
        .await
        .unwrap();
    assert_eq!(result.retcode, 10009);
    assert_eq!(mock.state().positions.len(), before + 1);
    assert_eq!(mock.executed_writes(), 1);
    assert_eq!(mock.auth_failures(), 0);
}

#[tokio::test]
async fn every_request_has_a_fresh_id_so_the_replay_guard_never_trips_the_client() {
    let (mock, client) = started().await;
    for _ in 0..50 {
        client.get_account().await.unwrap();
    }
    assert_eq!(mock.auth_failures(), 0);
}

#[tokio::test]
async fn the_golden_vector_matches_the_implementation_and_the_ea_self_test() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/auth.golden.json");
    let golden: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let key = BridgeKey::from_hex(golden["key"].as_str().unwrap()).unwrap();
    for (payload, mac) in [
        ("request_payload", "request_mac"),
        ("reply_payload", "reply_mac"),
    ] {
        let payload = golden[payload].as_str().unwrap().as_bytes();
        let mac = golden[mac].as_str().unwrap();
        assert_eq!(key.sign(payload), mac);
        assert!(key.verify(payload, mac.as_bytes()));
        assert!(serde_json::from_slice::<Value>(payload).is_ok());
    }
    // The request payload is a valid protocol request.
    let request: jarvis_bridge_client::protocol::Request =
        serde_json::from_str(golden["request_payload"].as_str().unwrap()).unwrap();
    assert_eq!(request.ts, Some(1_793_341_200_000));

    // The MQL5 self-test embeds the same values; keep the two in step.
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../mql5/Scripts/JarvisMT/JarvisSelfTest.mq5");
    let source = std::fs::read_to_string(script).unwrap();
    let mql_string = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    for field in ["key", "request_mac", "reply_mac"] {
        let value = golden[field].as_str().unwrap();
        assert!(
            source.contains(&format!("\"{value}\"")),
            "JarvisSelfTest.mq5 does not contain the golden {field}"
        );
    }
    for field in ["request_payload", "reply_payload"] {
        let value = mql_string(golden[field].as_str().unwrap());
        assert!(
            source.contains(&format!("\"{value}\"")),
            "JarvisSelfTest.mq5 does not contain the golden {field}"
        );
    }
}

#[tokio::test]
async fn mock_answers_malformed_and_unknown_requests_with_error_replies() {
    // Talk to the mock with a raw REQ socket to check its error handling.
    use zeromq::ReqSocket;
    let mock = MockBridge::start().await;
    let key = mock.key();
    let mut socket = ReqSocket::new();
    socket.connect(&mock.endpoint).await.unwrap();
    let ts = auth::now_ms();
    for (request, retcode) in [
        // known command, invalid arguments
        (
            format!(r#"{{"id":"1","ts":{ts},"cmd":"order_send","args":{{}}}}"#),
            -1,
        ),
        (
            format!(r#"{{"id":"2","ts":{ts},"cmd":"nope","args":{{}}}}"#),
            -2,
        ),
        (
            format!(r#"{{"id":"3","ts":{ts},"cmd":"get_price","args":{{}}}}"#),
            -1,
        ),
        (format!(r#"{{"ts":{ts},"cmd":"ping","args":{{}}}}"#), -1),
        ("not json at all".to_owned(), -1),
    ] {
        socket
            .send(auth::seal(&key, request.clone().into_bytes()))
            .await
            .unwrap();
        let reply = socket.recv().await.unwrap();
        let reply = open_reply(&key, &reply);
        assert_eq!(reply["ok"], false, "{request}");
        assert_eq!(reply["retcode"], retcode, "{request}: {reply}");
        assert!(reply["error"].as_str().is_some_and(|e| !e.is_empty()));
    }
}
