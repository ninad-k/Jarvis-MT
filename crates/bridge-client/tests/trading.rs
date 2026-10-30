//! Write-command contract tests: the Rust client against the in-process mock EA.
//!
//! The mock mirrors the EA's validation, safety gate and idempotency cache, so these tests also
//! document the behaviour the live tests (`live_trading.rs`) check against the real EA.
#![cfg(feature = "mock")]

use std::time::Duration;

use jarvis_bridge_client::mock::{MockBridge, MockState};
use jarvis_bridge_client::protocol::{
    Command, DealEntry, ExecType, JARVIS_MAGIC, OrderCancelArgs, OrderSendArgs, OrderType,
    PositionCloseArgs, PositionClosePartialArgs, PositionModifyArgs, RETCODE_INVALID_REQUEST,
    RETCODE_WRITE_REFUSED, Reply, Request, Side, TradeMode, TradeResult,
};
use jarvis_bridge_client::{BridgeClient, BridgeError, auth};
use zeromq::{ReqSocket, Socket, SocketRecv, SocketSend};

const TIMEOUT: Duration = Duration::from_millis(500);

async fn started() -> (MockBridge, BridgeClient) {
    let mock = MockBridge::start().await;
    let client = client_for(&mock);
    (mock, client)
}

fn client_for(mock: &MockBridge) -> BridgeClient {
    BridgeClient::with_write_timeout(mock.config(TIMEOUT), TIMEOUT).expect("client")
}

fn order(symbol: &str, side: Side, volume: f64, exec: ExecType) -> OrderSendArgs {
    OrderSendArgs {
        symbol: symbol.to_owned(),
        side,
        volume,
        exec,
        price: None,
        sl: None,
        tp: None,
        deviation_points: None,
        magic: JARVIS_MAGIC,
        comment: "jarvis:test".to_owned(),
    }
}

fn market(symbol: &str, side: Side, volume: f64) -> OrderSendArgs {
    order(symbol, side, volume, ExecType::Market)
}

/// Assert `result` is `Remote { retcode, message containing part }`.
fn assert_remote(result: Result<TradeResult, BridgeError>, retcode: i64, part: &str) {
    match result {
        Err(BridgeError::Remote {
            retcode: got,
            message,
        }) => {
            assert_eq!(got, retcode, "message: {message}");
            assert!(
                message.contains(part),
                "message {message:?} should contain {part:?}"
            );
        }
        other => panic!("expected Remote {retcode} ({part:?}), got {other:?}"),
    }
}

/// Send `request` over a fresh raw REQ socket, signed with the mock's key and stamped with the
/// current time (the client always generates its own id; tests that need a chosen id use this).
/// The reply is verified and returned without its `ts`, so replies can be compared.
async fn raw_call(mock: &MockBridge, request: &Request) -> Reply {
    let mut request = request.clone();
    request.ts = Some(auth::now_ms());
    let key = mock.key();
    let mut socket = ReqSocket::new();
    socket.connect(&mock.endpoint).await.expect("connect");
    socket
        .send(auth::seal(
            &key,
            serde_json::to_vec(&request).expect("json"),
        ))
        .await
        .expect("send");
    let frames = socket.recv().await.expect("recv");
    let payload = auth::open(&key, &frames).expect("the reply is signed");
    let mut reply: Reply = serde_json::from_slice(payload).expect("reply json");
    assert!(reply.ts.is_some(), "every reply carries the EA clock");
    reply.ts = None;
    reply
}

// ---- get_symbol -----------------------------------------------------------------------------

#[tokio::test]
async fn get_symbol_returns_realistic_specs_for_every_mock_symbol() {
    let (_mock, client) = started().await;
    for symbol in ["XAUUSD", "US100", "US30", "EURUSD", "GBPUSD", "BTCUSD"] {
        let spec = client.get_symbol(symbol).await.unwrap();
        assert_eq!(spec.symbol, symbol);
        assert!(!spec.description.is_empty());
        assert!(spec.trade_allowed);
        assert!(spec.volume_min > 0.0 && spec.volume_max >= spec.volume_min);
        assert!(spec.volume_step > 0.0 && spec.tick_size > 0.0 && spec.tick_value > 0.0);
        assert!(spec.contract_size > 0.0 && spec.point > 0.0 && spec.stops_level >= 0);
        assert!((spec.point - 10f64.powi(-i32::try_from(spec.digits).unwrap())).abs() < 1e-12);
        assert_eq!(spec.currency_profit, "USD");
    }
    assert!(matches!(
        client.get_symbol("NOPE").await,
        Err(BridgeError::Remote { retcode: 4301, .. })
    ));
}

// ---- order_send ------------------------------------------------------------------------------

#[tokio::test]
async fn market_buy_opens_a_position_at_the_ask() {
    let (mock, client) = started().await;
    let before = mock.state();
    let mut args = market("XAUUSD", Side::Buy, 0.1);
    args.sl = Some(2380.0);
    args.tp = Some(2410.0);
    let result = client.order_send(args).await.unwrap();

    assert_eq!(result.retcode, 10009);
    assert!(result.order > 0 && result.deal > 0);
    assert!(
        (result.price - 2391.40).abs() < 1e-9,
        "buy fills at the ask"
    );
    assert!((result.volume - 0.1).abs() < 1e-9);

    let positions = client
        .get_positions(Some("XAUUSD"))
        .await
        .unwrap()
        .positions;
    assert_eq!(positions.len(), 2);
    let new = positions.iter().find(|p| p.ticket == result.order).unwrap();
    assert_eq!(new.side, Side::Buy);
    assert_eq!(new.magic, JARVIS_MAGIC);
    assert_eq!(new.comment, "jarvis:test");
    assert_eq!((new.sl, new.tp), (Some(2380.0), Some(2410.0)));
    assert!((new.price_open - 2391.40).abs() < 1e-9);

    let after = mock.state();
    assert_eq!(after.positions.len(), before.positions.len() + 1);
    let deal = after
        .deals
        .iter()
        .find(|d| d.ticket == result.deal)
        .unwrap();
    assert_eq!(deal.entry, DealEntry::In);
    assert_eq!(deal.position_id, result.order);
    assert_eq!(mock.executed_writes(), 1);
}

#[tokio::test]
async fn market_sell_fills_at_the_bid_and_tickets_are_unique() {
    let (_mock, client) = started().await;
    let first = client
        .order_send(market("EURUSD", Side::Sell, 0.01))
        .await
        .unwrap();
    let second = client
        .order_send(market("EURUSD", Side::Sell, 0.01))
        .await
        .unwrap();
    assert!(
        (first.price - 1.08462).abs() < 1e-9,
        "sell fills at the bid"
    );
    assert_ne!(first.order, second.order);
    assert_ne!(first.deal, second.deal);
}

#[tokio::test]
async fn limit_and_stop_orders_become_pending_orders() {
    let (mock, client) = started().await;
    let mut buy_limit = order("EURUSD", Side::Buy, 0.02, ExecType::Limit);
    buy_limit.price = Some(1.07);
    buy_limit.sl = Some(1.06);
    buy_limit.tp = Some(1.09);
    let placed = client.order_send(buy_limit).await.unwrap();
    assert_eq!(placed.retcode, 10009);
    assert_eq!(placed.deal, 0, "a pending order has no deal");

    let mut sell_stop = order("EURUSD", Side::Sell, 0.02, ExecType::Stop);
    sell_stop.price = Some(1.07);
    client.order_send(sell_stop).await.unwrap();
    let mut buy_stop = order("XAUUSD", Side::Buy, 0.01, ExecType::Stop);
    buy_stop.price = Some(2400.0);
    client.order_send(buy_stop).await.unwrap();
    let mut sell_limit = order("XAUUSD", Side::Sell, 0.01, ExecType::Limit);
    sell_limit.price = Some(2400.0);
    client.order_send(sell_limit).await.unwrap();

    let orders = client.get_orders(None).await.unwrap().orders;
    assert_eq!(orders.len(), 5, "the canned order plus four new ones");
    let types: Vec<_> = orders.iter().skip(1).map(|o| o.order_type).collect();
    assert_eq!(
        types,
        [
            OrderType::BuyLimit,
            OrderType::SellStop,
            OrderType::BuyStop,
            OrderType::SellLimit
        ]
    );
    let first = orders.iter().find(|o| o.ticket == placed.order).unwrap();
    assert_eq!(first.price_open, 1.07);
    assert_eq!((first.sl, first.tp), (Some(1.06), Some(1.09)));
    assert_eq!(mock.state().positions.len(), 2, "no position was opened");
}

#[tokio::test]
async fn invalid_order_requests_are_rejected_without_touching_the_state() {
    let (mock, client) = started().await;
    let cases: Vec<(&str, OrderSendArgs, &str)> = vec![
        (
            "unknown symbol",
            market("NOPE", Side::Buy, 0.1),
            "unknown symbol: NOPE",
        ),
        ("volume zero", market("XAUUSD", Side::Buy, 0.0), "positive"),
        (
            "volume below min",
            market("XAUUSD", Side::Buy, 0.005),
            "below the minimum 0.01",
        ),
        (
            "volume above max",
            market("XAUUSD", Side::Buy, 100.01),
            "above the maximum 100",
        ),
        (
            "volume off step",
            market("XAUUSD", Side::Buy, 0.015),
            "multiple of the volume step 0.01",
        ),
        (
            "us100 step 0.1",
            market("US100", Side::Buy, 0.25),
            "multiple of the volume step 0.1",
        ),
        (
            "volume below min on index",
            market("US30", Side::Sell, 0.05),
            "below the minimum 0.1",
        ),
        (
            "market with price",
            {
                let mut o = market("XAUUSD", Side::Buy, 0.1);
                o.price = Some(2391.4);
                o
            },
            "must not carry a price",
        ),
        (
            "limit without price",
            order("XAUUSD", Side::Buy, 0.1, ExecType::Limit),
            "need a price",
        ),
        (
            "stop without price",
            order("XAUUSD", Side::Sell, 0.1, ExecType::Stop),
            "need a price",
        ),
        (
            "comment too long",
            {
                let mut o = market("XAUUSD", Side::Buy, 0.1);
                o.comment = "x".repeat(32);
                o
            },
            "longer than 31",
        ),
        (
            "negative magic",
            {
                let mut o = market("XAUUSD", Side::Buy, 0.1);
                o.magic = -1;
                o
            },
            "magic",
        ),
        (
            "deviation too large",
            {
                let mut o = market("XAUUSD", Side::Buy, 0.1);
                o.deviation_points = Some(10_001);
                o
            },
            "deviation_points",
        ),
        (
            "buy sl above the market",
            {
                let mut o = market("XAUUSD", Side::Buy, 0.1);
                o.sl = Some(2400.0);
                o
            },
            "sl 2400.00 must be below the bid 2391.12",
        ),
        (
            "buy tp below the market",
            {
                let mut o = market("XAUUSD", Side::Buy, 0.1);
                o.tp = Some(2380.0);
                o
            },
            "tp 2380.00 must be above the bid",
        ),
        (
            "sell sl below the market",
            {
                let mut o = market("XAUUSD", Side::Sell, 0.1);
                o.sl = Some(2380.0);
                o
            },
            "sl 2380.00 must be above the ask 2391.40",
        ),
        (
            "sell tp above the market",
            {
                let mut o = market("XAUUSD", Side::Sell, 0.1);
                o.tp = Some(2400.0);
                o
            },
            "tp 2400.00 must be below the ask",
        ),
        (
            "sl inside the stops level",
            {
                let mut o = market("XAUUSD", Side::Buy, 0.1);
                o.sl = Some(2391.05); // 7 points below the bid, level is 10 points = 0.10
                o
            },
            "by at least 10 points (stops level)",
        ),
        (
            "sl with too many decimals",
            {
                let mut o = market("XAUUSD", Side::Buy, 0.1);
                o.sl = Some(2380.123);
                o
            },
            "more than 2 decimals",
        ),
        (
            "negative sl",
            {
                let mut o = market("XAUUSD", Side::Buy, 0.1);
                o.sl = Some(-1.0);
                o
            },
            "sl must not be negative",
        ),
        (
            "buy limit above the ask",
            {
                let mut o = order("XAUUSD", Side::Buy, 0.1, ExecType::Limit);
                o.price = Some(2395.0);
                o
            },
            "buy limit price 2395.00 must be below the ask 2391.40",
        ),
        (
            "buy stop below the ask",
            {
                let mut o = order("XAUUSD", Side::Buy, 0.1, ExecType::Stop);
                o.price = Some(2380.0);
                o
            },
            "buy stop price 2380.00 must be above the ask",
        ),
        (
            "sell limit below the bid",
            {
                let mut o = order("XAUUSD", Side::Sell, 0.1, ExecType::Limit);
                o.price = Some(2380.0);
                o
            },
            "sell limit price 2380.00 must be above the bid 2391.12",
        ),
        (
            "sell stop above the bid",
            {
                let mut o = order("XAUUSD", Side::Sell, 0.1, ExecType::Stop);
                o.price = Some(2400.0);
                o
            },
            "sell stop price 2400.00 must be below the bid",
        ),
        (
            "pending price too close",
            {
                let mut o = order("XAUUSD", Side::Buy, 0.1, ExecType::Limit);
                o.price = Some(2391.35); // 5 points below the ask
                o
            },
            "by at least 10 points",
        ),
        (
            "pending sl on the wrong side of its price",
            {
                let mut o = order("XAUUSD", Side::Buy, 0.1, ExecType::Limit);
                o.price = Some(2300.0);
                o.sl = Some(2310.0);
                o
            },
            "sl 2310.00 must be below the order price 2300.00",
        ),
        (
            "price with too many decimals",
            {
                let mut o = order("EURUSD", Side::Buy, 0.1, ExecType::Limit);
                o.price = Some(1.0700012);
                o
            },
            "price has more than 5 decimals",
        ),
    ];
    for (name, args, part) in cases {
        assert_remote(client.order_send(args).await, RETCODE_INVALID_REQUEST, part);
        assert_eq!(mock.executed_writes(), 0, "{name} must not execute");
    }
    let state = mock.state();
    let fresh = MockState::default();
    assert_eq!(state.positions.len(), fresh.positions.len());
    assert_eq!(state.orders.len(), fresh.orders.len());
    assert_eq!(state.deals.len(), fresh.deals.len());
}

#[tokio::test]
async fn volume_tolerance_accepts_float_noise() {
    let (_mock, client) = started().await;
    // 0.1 + 0.2 = 0.30000000000000004, a valid 0.30 volume within the 1e-8 tolerance.
    let result = client
        .order_send(market("XAUUSD", Side::Buy, 0.1 + 0.2))
        .await;
    assert!(result.is_ok(), "{result:?}");
    // A real off-step volume is still rejected.
    assert_remote(
        client
            .order_send(market("XAUUSD", Side::Buy, 0.300001))
            .await,
        RETCODE_INVALID_REQUEST,
        "multiple of the volume step",
    );
}

#[tokio::test]
async fn symbol_that_is_not_tradable_is_rejected() {
    let mut state = MockState::default();
    state.symbols[0].trade_allowed = false;
    let mock = MockBridge::start_with(state).await;
    let client = client_for(&mock);
    assert_remote(
        client.order_send(market("XAUUSD", Side::Buy, 0.1)).await,
        RETCODE_INVALID_REQUEST,
        "trading is not allowed for symbol",
    );
}

// ---- position_modify -------------------------------------------------------------------------

#[tokio::test]
async fn modify_sets_keeps_and_removes_levels() {
    let (mock, client) = started().await;
    // Canned XAUUSD buy 1001001: sl none, tp 2410.
    let ticket = 1_001_001;
    let set = client
        .position_modify(PositionModifyArgs {
            ticket,
            sl: Some(2380.0),
            tp: None,
        })
        .await
        .unwrap();
    assert_eq!(set.retcode, 10009);
    let p = mock.state().positions[0].clone();
    assert_eq!((p.sl, p.tp), (Some(2380.0), Some(2410.0)), "tp is kept");

    client
        .position_modify(PositionModifyArgs {
            ticket,
            sl: None,
            tp: Some(0.0),
        })
        .await
        .unwrap();
    let p = mock.state().positions[0].clone();
    assert_eq!((p.sl, p.tp), (Some(2380.0), None), "0 removes tp");

    client
        .position_modify(PositionModifyArgs {
            ticket,
            sl: Some(0.0),
            tp: Some(2420.0),
        })
        .await
        .unwrap();
    let p = mock.state().positions[0].clone();
    assert_eq!((p.sl, p.tp), (None, Some(2420.0)));
}

#[tokio::test]
async fn invalid_modify_requests_are_rejected() {
    let (mock, client) = started().await;
    let modify = |ticket, sl, tp| PositionModifyArgs { ticket, sl, tp };
    // Canned US100 sell 1001002: bid 19822.5 ask 19823.3, stops level 50 points = 5.0.
    assert_remote(
        client.position_modify(modify(1_001_001, None, None)).await,
        RETCODE_INVALID_REQUEST,
        "at least one of 'sl' and 'tp'",
    );
    assert_remote(
        client.position_modify(modify(0, Some(1.0), None)).await,
        RETCODE_INVALID_REQUEST,
        "ticket",
    );
    assert_remote(
        client
            .position_modify(modify(1_001_001, Some(-1.0), None))
            .await,
        RETCODE_INVALID_REQUEST,
        "must not be negative",
    );
    assert_remote(
        client
            .position_modify(modify(999, Some(2380.0), None))
            .await,
        RETCODE_INVALID_REQUEST,
        "position not found: 999",
    );
    assert_remote(
        client
            .position_modify(modify(1_001_001, Some(2400.0), None))
            .await,
        RETCODE_INVALID_REQUEST,
        "sl 2400.00 must be below the bid",
    );
    assert_remote(
        client
            .position_modify(modify(1_001_001, None, Some(2380.0)))
            .await,
        RETCODE_INVALID_REQUEST,
        "tp 2380.00 must be above the bid",
    );
    assert_remote(
        client
            .position_modify(modify(1_001_002, Some(19800.0), None))
            .await,
        RETCODE_INVALID_REQUEST,
        "sl 19800.0 must be above the ask 19823.3",
    );
    assert_remote(
        client
            .position_modify(modify(1_001_002, None, Some(19824.0)))
            .await,
        RETCODE_INVALID_REQUEST,
        "tp 19824.0 must be below the ask",
    );
    assert_eq!(mock.executed_writes(), 0);
    assert_eq!(mock.state().positions[1].sl, Some(19920.0));
}

#[tokio::test]
async fn modify_does_not_revalidate_values_that_do_not_change() {
    let (_mock, client) = started().await;
    // The canned US100 sell has sl 19920 (valid), so re-sending it while only moving tp works.
    client
        .position_modify(PositionModifyArgs {
            ticket: 1_001_002,
            sl: Some(19920.0),
            tp: Some(19700.0),
        })
        .await
        .unwrap();
}

// ---- position_close / position_close_partial ---------------------------------------------------

#[tokio::test]
async fn close_removes_the_position_and_realises_its_profit() {
    let (mock, client) = started().await;
    let balance = mock.state().account.balance;
    let result = client
        .position_close(PositionCloseArgs {
            ticket: 1_001_001,
            deviation_points: Some(30),
        })
        .await
        .unwrap();
    assert_eq!(result.retcode, 10009);
    assert!((result.volume - 0.10).abs() < 1e-9);
    assert!(
        (result.price - 2391.12).abs() < 1e-9,
        "a buy closes at the bid"
    );
    let state = mock.state();
    assert!(state.positions.iter().all(|p| p.ticket != 1_001_001));
    assert!((state.account.balance - (balance + 57.20)).abs() < 1e-6);
    assert!((state.account.equity - (state.account.balance + state.account.profit)).abs() < 1e-6);
    let deal = state
        .deals
        .iter()
        .find(|d| d.ticket == result.deal)
        .unwrap();
    assert_eq!(deal.entry, DealEntry::Out);
    assert_eq!(deal.position_id, 1_001_001);
    assert_eq!(deal.symbol, "XAUUSD");

    assert_remote(
        client
            .position_close(PositionCloseArgs {
                ticket: 1_001_001,
                deviation_points: None,
            })
            .await,
        RETCODE_INVALID_REQUEST,
        "position not found",
    );
}

#[tokio::test]
async fn partial_close_reduces_the_volume() {
    let (mock, client) = started().await;
    let result = client
        .position_close_partial(PositionClosePartialArgs {
            ticket: 1_001_002,
            volume: 0.2,
            deviation_points: None,
        })
        .await
        .unwrap();
    assert!((result.volume - 0.2).abs() < 1e-9);
    let p = mock.state().positions[1].clone();
    assert!((p.volume - 0.3).abs() < 1e-9, "0.5 - 0.2 = {}", p.volume);
    assert!(
        (p.profit - 13.35 * 0.3 / 0.5).abs() < 0.01,
        "profit scales with volume"
    );
}

#[tokio::test]
async fn invalid_partial_closes_are_rejected() {
    let (mock, client) = started().await;
    let partial = |ticket, volume| PositionClosePartialArgs {
        ticket,
        volume,
        deviation_points: None,
    };
    assert_remote(
        client.position_close_partial(partial(1_001_002, 0.5)).await,
        RETCODE_INVALID_REQUEST,
        "must be less than the position volume 0.5",
    );
    assert_remote(
        client.position_close_partial(partial(1_001_002, 0.6)).await,
        RETCODE_INVALID_REQUEST,
        "must be less than the position volume",
    );
    assert_remote(
        client
            .position_close_partial(partial(1_001_002, 0.25))
            .await,
        RETCODE_INVALID_REQUEST,
        "multiple of the volume step 0.1",
    );
    assert_remote(
        client.position_close_partial(partial(1_001_002, 0.0)).await,
        RETCODE_INVALID_REQUEST,
        "positive",
    );
    assert_remote(
        client.position_close_partial(partial(777, 0.1)).await,
        RETCODE_INVALID_REQUEST,
        "position not found: 777",
    );
    assert_eq!(mock.executed_writes(), 0);
    assert!((mock.state().positions[1].volume - 0.5).abs() < 1e-9);
}

// ---- order_cancel ----------------------------------------------------------------------------

#[tokio::test]
async fn cancel_removes_a_pending_order() {
    let (mock, client) = started().await;
    let result = client
        .order_cancel(OrderCancelArgs { ticket: 2_001_001 })
        .await
        .unwrap();
    assert_eq!(result.retcode, 10009);
    assert_eq!(result.order, 2_001_001);
    assert!(mock.state().orders.is_empty());
    assert_remote(
        client
            .order_cancel(OrderCancelArgs { ticket: 2_001_001 })
            .await,
        RETCODE_INVALID_REQUEST,
        "pending order not found: 2001001",
    );
    assert_remote(
        client.order_cancel(OrderCancelArgs { ticket: 0 }).await,
        RETCODE_INVALID_REQUEST,
        "ticket",
    );
}

// ---- a full lifecycle -------------------------------------------------------------------------

#[tokio::test]
async fn open_modify_partial_close_close_round_trip() {
    let (mock, client) = started().await;
    let opened = client
        .order_send(market("EURUSD", Side::Buy, 0.2))
        .await
        .unwrap();
    let ticket = opened.order;
    client
        .position_modify(PositionModifyArgs {
            ticket,
            sl: Some(1.07),
            tp: Some(1.1),
        })
        .await
        .unwrap();
    client
        .position_close_partial(PositionClosePartialArgs {
            ticket,
            volume: 0.1,
            deviation_points: None,
        })
        .await
        .unwrap();
    let p = mock
        .state()
        .positions
        .into_iter()
        .find(|p| p.ticket == ticket)
        .unwrap();
    assert!((p.volume - 0.1).abs() < 1e-9);
    assert_eq!((p.sl, p.tp), (Some(1.07), Some(1.1)));
    client
        .position_close(PositionCloseArgs {
            ticket,
            deviation_points: None,
        })
        .await
        .unwrap();
    assert!(mock.state().positions.iter().all(|p| p.ticket != ticket));
    assert_eq!(mock.executed_writes(), 4);
}

// ---- safety gate (-3) -------------------------------------------------------------------------

const WRITE_NAMES: [&str; 5] = [
    "order_send",
    "position_modify",
    "position_close",
    "position_close_partial",
    "order_cancel",
];

/// Send write number `which` (an index into `WRITE_NAMES`) with arguments valid on the canned state.
async fn do_write(client: &BridgeClient, which: usize) -> Result<TradeResult, BridgeError> {
    match which {
        0 => client.order_send(market("XAUUSD", Side::Buy, 0.1)).await,
        1 => {
            client
                .position_modify(PositionModifyArgs {
                    ticket: 1_001_001,
                    sl: Some(2380.0),
                    tp: None,
                })
                .await
        }
        2 => {
            client
                .position_close(PositionCloseArgs {
                    ticket: 1_001_001,
                    deviation_points: None,
                })
                .await
        }
        3 => {
            client
                .position_close_partial(PositionClosePartialArgs {
                    ticket: 1_001_002,
                    volume: 0.1,
                    deviation_points: None,
                })
                .await
        }
        _ => {
            client
                .order_cancel(OrderCancelArgs { ticket: 2_001_001 })
                .await
        }
    }
}

async fn assert_all_writes_refused(client: &BridgeClient, part: &str) {
    for (which, name) in WRITE_NAMES.iter().enumerate() {
        match do_write(client, which).await {
            Err(BridgeError::Remote { retcode, message }) => {
                assert_eq!(retcode, RETCODE_WRITE_REFUSED, "{name}: {message}");
                assert!(message.contains(part), "{name}: {message}");
            }
            other => panic!("{name}: expected a refusal, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn refuse_writes_flag_answers_minus_three_and_changes_nothing() {
    let (mock, client) = started().await;
    mock.refuse_writes(true);
    assert_all_writes_refused(
        &client,
        "writes refused on a real account (InpAllowReal is false)",
    )
    .await;
    assert_eq!(mock.executed_writes(), 0);
    let state = mock.state();
    assert_eq!(state.positions.len(), 2);
    assert_eq!(state.orders.len(), 1);
    // Reads still work.
    assert!(client.get_positions(None).await.is_ok());
    assert!(client.get_symbol("XAUUSD").await.is_ok());

    mock.refuse_writes(false);
    assert!(
        client
            .order_send(market("XAUUSD", Side::Buy, 0.1))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn real_account_refuses_writes_unless_allowed() {
    let mut state = MockState::default();
    state.ping.trade_mode = TradeMode::Real;
    state.account.trade_mode = TradeMode::Real;
    let mock = MockBridge::start_with(state.clone()).await;
    let client = client_for(&mock);
    assert_all_writes_refused(&client, "real account").await;

    state.allow_real = true;
    let mock = MockBridge::start_with(state).await;
    let client = client_for(&mock);
    assert!(
        client
            .order_send(market("XAUUSD", Side::Buy, 0.1))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn algo_trading_off_refuses_writes() {
    let mut state = MockState::default();
    state.ping.trade_allowed = false;
    let mock = MockBridge::start_with(state).await;
    let client = client_for(&mock);
    assert_all_writes_refused(&client, "enable Algo Trading").await;
}

#[tokio::test]
async fn malformed_requests_are_rejected_before_the_gate() {
    let (mock, client) = started().await;
    mock.refuse_writes(true);
    // Structural problems (-1) are reported even where the account would refuse (-3).
    let mut bad = market("XAUUSD", Side::Buy, 0.1);
    bad.price = Some(1.0);
    assert_remote(
        client.order_send(bad).await,
        RETCODE_INVALID_REQUEST,
        "must not carry a price",
    );
    assert_remote(
        client
            .position_modify(PositionModifyArgs {
                ticket: 1,
                sl: None,
                tp: None,
            })
            .await,
        RETCODE_INVALID_REQUEST,
        "at least one",
    );
    // A state-dependent problem comes after the gate.
    assert_remote(
        client.order_send(market("XAUUSD", Side::Buy, 0.005)).await,
        RETCODE_WRITE_REFUSED,
        "writes refused",
    );
}

// ---- forced trade failures ----------------------------------------------------------------------

#[tokio::test]
async fn fail_next_write_returns_the_trade_retcode_and_changes_nothing() {
    let (mock, client) = started().await;
    mock.fail_next_write(10019);
    assert_remote(
        client.order_send(market("XAUUSD", Side::Buy, 0.1)).await,
        10019,
        "trade failed: no money (10019)",
    );
    assert_eq!(mock.state().positions.len(), 2);
    assert_eq!(mock.executed_writes(), 1, "it reached execution");
    // The flag is consumed by that one write.
    assert!(
        client
            .order_send(market("XAUUSD", Side::Buy, 0.1))
            .await
            .is_ok()
    );
    assert_eq!(mock.state().positions.len(), 3);

    mock.fail_next_write(10018);
    assert_remote(
        client
            .position_close(PositionCloseArgs {
                ticket: 1_001_001,
                deviation_points: None,
            })
            .await,
        10018,
        "market closed",
    );
    assert!(mock.state().positions.iter().any(|p| p.ticket == 1_001_001));
}

#[tokio::test]
async fn validation_errors_do_not_consume_fail_next_write() {
    let (mock, client) = started().await;
    mock.fail_next_write(10019);
    assert_remote(
        client.order_send(market("XAUUSD", Side::Buy, 0.005)).await,
        RETCODE_INVALID_REQUEST,
        "below the minimum",
    );
    assert_remote(
        client.order_send(market("XAUUSD", Side::Buy, 0.1)).await,
        10019,
        "no money",
    );
}

// ---- idempotency --------------------------------------------------------------------------------
//
// With authentication on, a repeated request id is refused before it can reach the reply cache
// (`a_replayed_request_id_is_refused` in tests/client.rs). The cache is the second line of defence,
// so these tests switch the replay guard off to exercise it, exactly as the EA's cache is
// exercised in the MQL5 self-test.

fn raw_order_request(id: &str) -> Request {
    Request {
        id: id.to_owned(),
        ts: None,
        command: Command::OrderSend(market("XAUUSD", Side::Buy, 0.1)),
    }
}

#[tokio::test]
async fn a_repeated_request_id_gets_the_cached_reply_and_executes_once() {
    let (mock, _client) = started().await;
    mock.allow_replays(true);
    let request = raw_order_request("same-id-1");
    let first = raw_call(&mock, &request).await;
    assert!(first.ok, "{first:?}");
    let second = raw_call(&mock, &request).await;
    assert_eq!(first, second, "verbatim cached reply");
    assert_eq!(mock.state().positions.len(), 3, "one new position, not two");
    assert_eq!(mock.executed_writes(), 1);
    assert_eq!(mock.cache_hits(), 1);

    // A different command with the same id still returns the cached reply.
    let other = Request {
        id: "same-id-1".to_owned(),
        ts: None,
        command: Command::PositionClose(PositionCloseArgs {
            ticket: 1_001_001,
            deviation_points: None,
        }),
    };
    assert_eq!(raw_call(&mock, &other).await, first);
    assert_eq!(mock.state().positions.len(), 3);
    assert_eq!(mock.cache_hits(), 2);
}

#[tokio::test]
async fn rejected_and_refused_replies_are_cached_too() {
    let (mock, _client) = started().await;
    mock.allow_replays(true);
    let bad = Request {
        id: "bad-id".to_owned(),
        ts: None,
        command: Command::OrderSend(market("XAUUSD", Side::Buy, 0.005)),
    };
    let first = raw_call(&mock, &bad).await;
    assert!(!first.ok && first.retcode == RETCODE_INVALID_REQUEST);
    // Even after the situation changes, the same id keeps its first answer.
    mock.refuse_writes(true);
    assert_eq!(raw_call(&mock, &bad).await, first);
    mock.refuse_writes(false);

    let refused = Request {
        id: "refused-id".to_owned(),
        ts: None,
        command: Command::OrderSend(market("XAUUSD", Side::Buy, 0.1)),
    };
    mock.refuse_writes(true);
    let first = raw_call(&mock, &refused).await;
    assert_eq!(first.retcode, RETCODE_WRITE_REFUSED);
    mock.refuse_writes(false);
    assert_eq!(
        raw_call(&mock, &refused).await,
        first,
        "not executed after the flag is cleared"
    );
    assert_eq!(mock.state().positions.len(), 2);
}

#[tokio::test]
async fn the_cache_holds_64_replies() {
    let (mock, _client) = started().await;
    mock.allow_replays(true);
    let first = raw_call(&mock, &raw_order_request("first")).await;
    assert!(first.ok);
    // 63 more distinct ids keep "first" cached (64 entries); the 64th evicts it.
    for i in 0..63 {
        let cancel = Request {
            id: format!("filler-{i}"),
            ts: None,
            command: Command::OrderCancel(OrderCancelArgs { ticket: 99 }),
        };
        raw_call(&mock, &cancel).await;
    }
    assert_eq!(raw_call(&mock, &raw_order_request("first")).await, first);
    assert_eq!(mock.executed_writes(), 1);
    let cancel = Request {
        id: "filler-63".to_owned(),
        ts: None,
        command: Command::OrderCancel(OrderCancelArgs { ticket: 99 }),
    };
    raw_call(&mock, &cancel).await;
    // Now "first" is gone and the same id executes again.
    let again = raw_call(&mock, &raw_order_request("first")).await;
    assert!(again.ok);
    assert_ne!(again, first, "a new order ticket");
    assert_eq!(mock.executed_writes(), 2);
}

#[tokio::test]
async fn reads_are_never_cached() {
    let (mock, _client) = started().await;
    let request = Request {
        id: "read-id".to_owned(),
        ts: None,
        command: Command::GetPositions { symbol: None },
    };
    let before = raw_call(&mock, &request).await;
    mock.update_state(|s| s.positions.clear());
    let after = raw_call(&mock, &request).await;
    assert_ne!(before, after);
    assert_eq!(mock.cache_hits(), 0);
}

// ---- the client never resends a write ------------------------------------------------------------

#[tokio::test]
async fn a_write_that_times_out_returns_timeout_and_is_not_resent() {
    let (mock, client) = started().await;
    assert!(client.ping().await.is_ok());
    mock.drop_next(1);
    match client.order_send(market("XAUUSD", Side::Buy, 0.1)).await {
        Err(BridgeError::Timeout(ms)) => assert_eq!(ms, 500),
        other => panic!("expected Timeout, got {other:?}"),
    }
    // Give a (wrongly) retrying client time to resend.
    tokio::time::sleep(TIMEOUT * 3).await;
    assert_eq!(mock.request_count(), 2, "ping + exactly one order_send");
    assert_eq!(mock.executed_writes(), 0);
    // The client is usable again on a fresh socket, and nothing was queued behind the failure.
    assert!(client.ping().await.is_ok());
    assert_eq!(mock.request_count(), 3);
    assert_eq!(mock.state().positions.len(), 2);
}

#[tokio::test]
async fn every_write_command_times_out_without_a_resend() {
    let (mock, client) = started().await;
    for (which, name) in WRITE_NAMES.iter().enumerate() {
        mock.drop_next(1);
        match do_write(&client, which).await {
            Err(BridgeError::Timeout(_)) => {}
            other => panic!("{name}: expected Timeout, got {other:?}"),
        }
    }
    tokio::time::sleep(TIMEOUT * 2).await;
    assert_eq!(
        mock.request_count(),
        WRITE_NAMES.len(),
        "one request per write, none resent"
    );
    assert_eq!(mock.executed_writes(), 0);
}

// ---- the golden fixtures are what the mock answers --------------------------------------------------

fn fixture(name: &str) -> serde_json::Value {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read fixture {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("fixture {name} is not JSON: {e}"))
}

/// Run a fixture request against a fresh default mock (after `setup`) and require the fixture reply.
async fn assert_mock_answers(request: &str, reply: &str, setup: impl FnOnce(&MockBridge)) {
    let mock = MockBridge::start().await;
    setup(&mock);
    let request: Request = serde_json::from_value(fixture(request)).expect("request fixture");
    let got = raw_call(&mock, &request).await;
    let got = serde_json::to_value(got).expect("reply to value");
    assert_eq!(got, fixture(reply), "{reply}");
}

#[tokio::test]
async fn the_mock_reproduces_every_golden_write_fixture() {
    for base in [
        "get_symbol",
        "order_send",
        "order_send_limit",
        "position_modify",
        "position_close",
        "position_close_partial",
        "order_cancel",
    ] {
        assert_mock_answers(
            &format!("{base}.request.json"),
            &format!("{base}.reply.json"),
            |_| {},
        )
        .await;
    }
    assert_mock_answers(
        "order_send_invalid_volume.request.json",
        "order_send_invalid_volume.reply.json",
        |_| {},
    )
    .await;
    assert_mock_answers(
        "order_send.request.json",
        "order_send_refused.reply.json",
        |m| m.refuse_writes(true),
    )
    .await;
    assert_mock_answers(
        "order_send.request.json",
        "order_send_no_money.reply.json",
        |m| m.fail_next_write(10019),
    )
    .await;
}
