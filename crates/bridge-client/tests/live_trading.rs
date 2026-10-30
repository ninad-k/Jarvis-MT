//! Live trading tests against a real `JarvisBridge` EA on an MT5 **demo** account.
//!
//! These tests OPEN, MODIFY and CLOSE real (demo) positions and orders at the symbol's minimum
//! volume. Run them explicitly, single-threaded:
//!
//! ```text
//! cargo test -p jarvis-bridge-client --features mock --test live_trading -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! Environment:
//! - `JARVIS_BRIDGE_ENDPOINT` (default `tcp://127.0.0.1:5555`)
//! - `JARVIS_LIVE_SYMBOL` (default `EURUSD`; use your broker's name, e.g. `EURUSD.m` or `XAUUSD_`)
//!
//! Safety:
//! - Every test first asserts that `ping` reports a **demo** account and aborts otherwise.
//! - Everything a test creates carries the Jarvis magic number and a `jarvis-live:` comment, and
//!   its ticket is recorded. A guard closes/cancels whatever matches (recorded ticket, or magic
//!   plus comment prefix) when the test ends, also when it panics. The broker may blank the
//!   comment of a position after a partial close, which is why tickets are recorded too.
//!   Positions and orders of other origin are never touched.
//! - If the market is closed (retcode 10018) a test prints `SKIPPED` and returns.
use std::sync::Mutex;
use std::time::Duration;

use jarvis_bridge_client::protocol::{
    ExecType, JARVIS_MAGIC, OrderCancelArgs, OrderSendArgs, OrderType, PingInfo, Position,
    PositionCloseArgs, PositionClosePartialArgs, PositionModifyArgs, Reply, Request, Side,
    SymbolInfo, TradeMode, TradeResult,
};
use jarvis_bridge_client::{BridgeClient, BridgeConfig, BridgeError};
use zeromq::{ReqSocket, Socket, SocketRecv, SocketSend, ZmqMessage};

const COMMENT_PREFIX: &str = "jarvis-live:";

/// Tickets of everything the tests created (market orders become positions with the same ticket
/// on hedging accounts).
static CREATED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

fn record(ticket: u64) {
    if ticket != 0 {
        CREATED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(ticket);
    }
}

fn recorded(ticket: u64) -> bool {
    CREATED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(&ticket)
}

fn endpoint() -> String {
    std::env::var("JARVIS_BRIDGE_ENDPOINT").unwrap_or_else(|_| BridgeConfig::default().endpoint)
}

fn symbol() -> String {
    std::env::var("JARVIS_LIVE_SYMBOL").unwrap_or_else(|_| "EURUSD".to_owned())
}

fn client() -> BridgeClient {
    BridgeClient::new(BridgeConfig {
        endpoint: endpoint(),
        // A trade server round trip can take seconds.
        timeout: Duration::from_secs(15),
    })
    .expect("valid loopback endpoint")
}

/// A comment that identifies what this test created.
fn unique_comment() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("{COMMENT_PREFIX}{}", &id[..8])
}

fn is_ours(comment: &str, magic: i64) -> bool {
    magic == JARVIS_MAGIC && comment.starts_with(COMMENT_PREFIX)
}

fn market_closed(error: &BridgeError) -> bool {
    matches!(error, BridgeError::Remote { retcode: 10018, .. })
}

/// Abort unless the account is a demo account with trading enabled.
async fn require_demo(client: &BridgeClient) -> PingInfo {
    let info = client.ping().await.expect("ping");
    assert_eq!(
        info.trade_mode,
        TradeMode::Demo,
        "ABORT: live trading tests run on a demo account only"
    );
    assert!(
        info.trade_allowed,
        "ABORT: Algo Trading is off in the terminal or for the EA; writes would be refused"
    );
    info
}

/// Closes and cancels everything the test tagged, when dropped (also while unwinding a panic).
struct Cleanup;

impl Drop for Cleanup {
    fn drop(&mut self) {
        let outcome = std::thread::spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("cleanup runtime");
            runtime.block_on(cleanup())
        })
        .join();
        match outcome {
            Ok(summary) => println!("cleanup: {summary}"),
            Err(_) => eprintln!("cleanup: the cleanup thread panicked; check the account by hand"),
        }
    }
}

async fn cleanup() -> String {
    let client = client();
    let mut closed = 0;
    let mut cancelled = 0;
    let mut failed = Vec::new();
    match client.get_positions(None).await {
        Ok(positions) => {
            for p in positions
                .positions
                .iter()
                .filter(|p| recorded(p.ticket) || is_ours(&p.comment, p.magic))
            {
                match client
                    .position_close(PositionCloseArgs {
                        ticket: p.ticket,
                        deviation_points: Some(100),
                    })
                    .await
                {
                    Ok(_) => closed += 1,
                    Err(e) => failed.push(format!("position {}: {e}", p.ticket)),
                }
            }
        }
        Err(e) => failed.push(format!("could not list positions: {e}")),
    }
    match client.get_orders(None).await {
        Ok(orders) => {
            for o in orders
                .orders
                .iter()
                .filter(|o| recorded(o.ticket) || o.comment.starts_with(COMMENT_PREFIX))
            {
                match client
                    .order_cancel(OrderCancelArgs { ticket: o.ticket })
                    .await
                {
                    Ok(_) => cancelled += 1,
                    Err(e) => failed.push(format!("order {}: {e}", o.ticket)),
                }
            }
        }
        Err(e) => failed.push(format!("could not list orders: {e}")),
    }
    if !failed.is_empty() {
        eprintln!("cleanup FAILED, close these by hand: {failed:?}");
    }
    format!(
        "closed {closed} position(s), cancelled {cancelled} order(s), {} failure(s)",
        failed.len()
    )
}

fn round_to(value: f64, digits: u32) -> f64 {
    let scale = 10f64.powi(i32::try_from(digits).unwrap_or(0));
    (value * scale).round() / scale
}

fn approx(a: f64, b: f64, spec: &SymbolInfo) -> bool {
    (a - b).abs() <= spec.point * 0.5
}

fn is_success(result: &TradeResult) -> bool {
    matches!(result.retcode, 10008..=10010)
}

fn market_order(spec: &SymbolInfo, side: Side, volume: f64, comment: &str) -> OrderSendArgs {
    OrderSendArgs {
        symbol: spec.symbol.clone(),
        side,
        volume,
        exec: ExecType::Market,
        price: None,
        sl: None,
        tp: None,
        deviation_points: Some(100),
        magic: JARVIS_MAGIC,
        comment: comment.to_owned(),
    }
}

/// The position opened by a test: by ticket, or by comment right after opening.
async fn find_position(
    client: &BridgeClient,
    symbol: &str,
    ticket: u64,
    comment: &str,
) -> Option<Position> {
    client
        .get_positions(Some(symbol))
        .await
        .expect("get_positions")
        .positions
        .into_iter()
        .find(|p| p.magic == JARVIS_MAGIC && (p.ticket == ticket || p.comment == comment))
}

/// Send `request` over a fresh raw REQ socket and return the reply (bypasses the client, which
/// always generates its own request id).
async fn raw_call(request: &Request) -> Reply {
    let mut socket = ReqSocket::new();
    let target = endpoint().replace("localhost", "127.0.0.1");
    socket.connect(&target).await.expect("connect");
    let payload = serde_json::to_vec(request).expect("encode");
    socket
        .send(ZmqMessage::from(payload))
        .await
        .expect("send request");
    let frames = tokio::time::timeout(Duration::from_secs(20), socket.recv())
        .await
        .expect("reply within 20 s")
        .expect("receive reply");
    let frame = frames.get(0).expect("one frame");
    serde_json::from_slice(frame).expect("reply JSON")
}

/// Unwrap a trade call. `None` means the market is closed and the test should stop.
fn trade_or_skip(result: Result<TradeResult, BridgeError>, what: &str) -> Option<TradeResult> {
    match result {
        Ok(r) => {
            record(r.order);
            assert!(is_success(&r), "{what}: unexpected retcode {r:?}");
            println!("{what}: {r:?}");
            Some(r)
        }
        Err(e) if market_closed(&e) => {
            println!("SKIPPED ({what}): market closed (retcode 10018)");
            None
        }
        Err(e) => panic!("{what} failed: {e}"),
    }
}

#[tokio::test]
#[ignore = "places demo trades; needs MT5 demo terminal with the JarvisBridge EA running"]
async fn live_market_buy_modify_partial_close_close() {
    let client = client();
    require_demo(&client).await;
    let _cleanup = Cleanup;
    let symbol = symbol();
    let spec = client.get_symbol(&symbol).await.expect("get_symbol");
    assert!(spec.trade_allowed, "{symbol} is not tradable");
    println!("{spec:?}");

    // Open twice the minimum so a partial close of one minimum lot is possible.
    let double = round_to(spec.volume_min * 2.0, 8);
    let can_split = double <= spec.volume_max
        && ((double / spec.volume_step).round() * spec.volume_step - double).abs() < 1e-8;
    let volume = if can_split { double } else { spec.volume_min };

    let price = client.get_price(&symbol).await.expect("get_price");
    // Stops 1% away: far enough not to trigger during the test and beyond any stops level.
    let distance = round_to(price.ask * 0.01, spec.digits);
    let sl = round_to(price.bid - distance, spec.digits);
    let tp = round_to(price.ask + distance, spec.digits);
    let comment = unique_comment();

    let mut order = market_order(&spec, Side::Buy, volume, &comment);
    order.sl = Some(sl);
    order.tp = Some(tp);
    let Some(opened) = trade_or_skip(client.order_send(order).await, "market buy") else {
        return;
    };
    assert!(opened.order > 0 && opened.deal > 0, "{opened:?}");
    assert!((opened.volume - volume).abs() < 1e-8, "{opened:?}");
    assert!(opened.price > 0.0, "{opened:?}");

    let position = find_position(&client, &symbol, opened.order, &comment)
        .await
        .expect("the new position is listed");
    assert_eq!(position.side, Side::Buy);
    assert!((position.volume - volume).abs() < 1e-8);
    assert!(
        position.sl.is_some_and(|v| approx(v, sl, &spec)),
        "{position:?}"
    );
    assert!(
        position.tp.is_some_and(|v| approx(v, tp, &spec)),
        "{position:?}"
    );
    let ticket = position.ticket;

    // Modify: tighten both levels.
    let new_sl = round_to(sl + distance / 4.0, spec.digits);
    let new_tp = round_to(tp - distance / 4.0, spec.digits);
    let modified = client
        .position_modify(PositionModifyArgs {
            ticket,
            sl: Some(new_sl),
            tp: Some(new_tp),
        })
        .await
        .expect("position_modify");
    assert!(is_success(&modified), "{modified:?}");
    println!("modify: {modified:?}");
    let position = find_position(&client, &symbol, ticket, &comment)
        .await
        .expect("position");
    assert!(
        position.sl.is_some_and(|v| approx(v, new_sl, &spec)),
        "{position:?}"
    );
    assert!(
        position.tp.is_some_and(|v| approx(v, new_tp, &spec)),
        "{position:?}"
    );

    // Remove only the stop loss: tp stays.
    client
        .position_modify(PositionModifyArgs {
            ticket,
            sl: Some(0.0),
            tp: None,
        })
        .await
        .expect("remove sl");
    let position = find_position(&client, &symbol, ticket, &comment)
        .await
        .expect("position");
    assert!(position.sl.is_none(), "{position:?}");
    assert!(
        position.tp.is_some_and(|v| approx(v, new_tp, &spec)),
        "{position:?}"
    );

    // Partial close of one minimum lot.
    if can_split {
        let partial = client
            .position_close_partial(PositionClosePartialArgs {
                ticket,
                volume: spec.volume_min,
                deviation_points: Some(100),
            })
            .await
            .expect("position_close_partial");
        assert!(is_success(&partial), "{partial:?}");
        println!("partial close: {partial:?}");
        let position = find_position(&client, &symbol, ticket, &comment)
            .await
            .expect("position");
        assert!(
            (position.volume - spec.volume_min).abs() < 1e-8,
            "volume after the partial close: {position:?}"
        );
    } else {
        println!("partial close skipped: 2 x volume_min is not a valid volume for {symbol}");
    }

    // Close the rest.
    let closed = client
        .position_close(PositionCloseArgs {
            ticket,
            deviation_points: Some(100),
        })
        .await
        .expect("position_close");
    assert!(is_success(&closed), "{closed:?}");
    println!("close: {closed:?}");
    assert!(
        find_position(&client, &symbol, ticket, &comment)
            .await
            .is_none()
    );
}

#[tokio::test]
#[ignore = "places demo orders; needs MT5 demo terminal with the JarvisBridge EA running"]
async fn live_far_away_limit_order_is_placed_and_cancelled() {
    let client = client();
    require_demo(&client).await;
    let _cleanup = Cleanup;
    let symbol = symbol();
    let spec = client.get_symbol(&symbol).await.expect("get_symbol");
    let price = client.get_price(&symbol).await.expect("get_price");
    let limit = round_to(price.ask * 0.95, spec.digits);
    let comment = unique_comment();

    let mut order = market_order(&spec, Side::Buy, spec.volume_min, &comment);
    order.exec = ExecType::Limit;
    order.price = Some(limit);
    order.deviation_points = None;
    let Some(placed) = trade_or_skip(client.order_send(order).await, "buy limit") else {
        return;
    };
    assert!(placed.order > 0, "{placed:?}");

    let orders = client.get_orders(Some(&symbol)).await.expect("get_orders");
    let ours = orders
        .orders
        .iter()
        .find(|o| o.ticket == placed.order)
        .expect("the pending order is listed");
    assert_eq!(ours.order_type, OrderType::BuyLimit);
    assert!(approx(ours.price_open, limit, &spec), "{ours:?}");
    assert_eq!(ours.comment, comment);

    let cancelled = client
        .order_cancel(OrderCancelArgs {
            ticket: placed.order,
        })
        .await
        .expect("order_cancel");
    assert!(is_success(&cancelled), "{cancelled:?}");
    println!("cancel: {cancelled:?}");
    let orders = client.get_orders(Some(&symbol)).await.expect("get_orders");
    assert!(orders.orders.iter().all(|o| o.ticket != placed.order));
}

#[tokio::test]
#[ignore = "places demo trades; needs MT5 demo terminal with the JarvisBridge EA running"]
async fn live_duplicate_request_id_returns_the_cached_reply_and_trades_once() {
    let client = client();
    require_demo(&client).await;
    let _cleanup = Cleanup;
    let symbol = symbol();
    let spec = client.get_symbol(&symbol).await.expect("get_symbol");
    let comment = unique_comment();
    let request = Request {
        id: uuid::Uuid::new_v4().to_string(),
        command: jarvis_bridge_client::protocol::Command::OrderSend(market_order(
            &spec,
            Side::Buy,
            spec.volume_min,
            &comment,
        )),
    };

    let first = raw_call(&request).await;
    if !first.ok && first.retcode == 10018 {
        println!("SKIPPED (duplicate id): market closed (retcode 10018)");
        return;
    }
    assert!(first.ok, "first request failed: {first:?}");
    if let Some(order) = first
        .data
        .as_ref()
        .and_then(|d| d.get("order"))
        .and_then(serde_json::Value::as_u64)
    {
        record(order);
    }
    let second = raw_call(&request).await;
    assert_eq!(
        first, second,
        "the repeated id must get the cached reply verbatim"
    );

    let ours: Vec<_> = client
        .get_positions(Some(&symbol))
        .await
        .expect("get_positions")
        .positions
        .into_iter()
        .filter(|p| p.comment == comment)
        .collect();
    assert_eq!(
        ours.len(),
        1,
        "the duplicate must not open a second position: {ours:?}"
    );
}

#[tokio::test]
#[ignore = "sends write commands that must be rejected; needs MT5 demo terminal with the EA running"]
async fn live_invalid_requests_are_rejected_with_minus_one() {
    let client = client();
    require_demo(&client).await;
    let _cleanup = Cleanup;
    let symbol = symbol();
    let spec = client.get_symbol(&symbol).await.expect("get_symbol");
    let before = client
        .get_positions(None)
        .await
        .expect("positions")
        .positions
        .len();
    let orders_before = client.get_orders(None).await.expect("orders").orders.len();

    let expect_minus_one =
        |what: &str, result: Result<TradeResult, BridgeError>, part: &str| match result {
            Err(BridgeError::Remote {
                retcode: -1,
                message,
            }) => {
                assert!(
                    message.contains(part),
                    "{what}: {message:?} should mention {part:?}"
                );
                println!("{what}: rejected: {message}");
            }
            other => panic!("{what}: expected retcode -1, got {other:?}"),
        };

    let comment = unique_comment();
    let mut too_small = market_order(&spec, Side::Buy, spec.volume_min / 2.0, &comment);
    too_small.deviation_points = None;
    expect_minus_one(
        "volume below minimum",
        client.order_send(too_small).await,
        "below the minimum",
    );

    let off_step = market_order(
        &spec,
        Side::Buy,
        spec.volume_min + spec.volume_step / 2.0,
        &comment,
    );
    expect_minus_one(
        "volume off the step",
        client.order_send(off_step).await,
        "multiple of the volume step",
    );

    let too_big = market_order(
        &spec,
        Side::Buy,
        spec.volume_max + spec.volume_step,
        &comment,
    );
    expect_minus_one(
        "volume above maximum",
        client.order_send(too_big).await,
        "above the maximum",
    );

    let mut market_with_price = market_order(&spec, Side::Buy, spec.volume_min, &comment);
    market_with_price.price = Some(1.0);
    expect_minus_one(
        "market order with a price",
        client.order_send(market_with_price).await,
        "price",
    );

    let price = client.get_price(&symbol).await.expect("price");
    let mut wrong_side = market_order(&spec, Side::Buy, spec.volume_min, &comment);
    wrong_side.sl = Some(round_to(price.ask * 1.01, spec.digits));
    expect_minus_one(
        "buy with sl above the market",
        client.order_send(wrong_side).await,
        "sl",
    );

    expect_minus_one(
        "unknown position",
        client
            .position_modify(PositionModifyArgs {
                ticket: 999_999_999_999,
                sl: Some(1.0),
                tp: None,
            })
            .await,
        "position not found",
    );
    expect_minus_one(
        "unknown pending order",
        client
            .order_cancel(OrderCancelArgs {
                ticket: 999_999_999_999,
            })
            .await,
        "pending order not found",
    );

    let after = client
        .get_positions(None)
        .await
        .expect("positions")
        .positions
        .len();
    let orders_after = client.get_orders(None).await.expect("orders").orders.len();
    assert_eq!(
        (before, orders_before),
        (after, orders_after),
        "nothing may change"
    );
}
