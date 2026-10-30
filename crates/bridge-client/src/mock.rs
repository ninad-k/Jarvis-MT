//! In-process ZeroMQ REP server speaking the bridge protocol with canned data.
//! Used by unit tests here and in `jarvis-api`, and for offline development.
//!
//! The server is a spawned task around `zeromq::RepSocket`. That socket does not enforce
//! strict REP lock-step (a request that is never answered simply leaves the socket ready for
//! the next `recv`), so [`MockBridge::drop_next`] is just "receive and do not reply". A client
//! that times out and reconnects is served normally afterwards; its old connection ends and
//! the mock ignores the resulting disconnect.
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use zeromq::{RepSocket, Socket, SocketRecv, SocketSend, ZmqMessage};

use crate::error::BridgeError;
use crate::protocol::{
    Account, Command, Deal, DealEntry, DealType, HISTORY_LIMIT_DEFAULT, HISTORY_LIMIT_MAX, History,
    HistoryQuery, Order, OrderType, Orders, PingInfo, Position, Positions, Price,
    RETCODE_INVALID_REQUEST, RETCODE_UNKNOWN_COMMAND, Reply, Request, Side, TradeMode,
};

/// MT5 `ERR_MARKET_UNKNOWN_SYMBOL`.
const RETCODE_UNKNOWN_SYMBOL: i64 = 4301;

/// Every command name the mock (and protocol version 1) knows.
const KNOWN_COMMANDS: [&str; 6] = [
    "ping",
    "get_price",
    "get_account",
    "get_positions",
    "get_orders",
    "get_history",
];

/// Canned state served by [`MockBridge`]. `Default` gives a demo account with
/// XAUUSD, US100, US30, EURUSD, GBPUSD, BTCUSD prices, two open positions, one pending
/// order and five deals (one of them a balance deal). Deal and position times are relative
/// to "now" at the moment `default()` is called.
#[derive(Debug, Clone)]
pub struct MockState {
    pub ping: PingInfo,
    pub account: Account,
    pub prices: Vec<Price>,
    pub positions: Vec<Position>,
    pub orders: Vec<Order>,
    pub deals: Vec<Deal>,
    /// When set, the mock swallows this many requests without replying (timeout tests).
    pub drop_next: usize,
}

impl Default for MockState {
    fn default() -> Self {
        let now = unix_now();
        let h = 3600;
        let d = 24 * h;
        let price = |symbol: &str, bid: f64, ask: f64, digits: u32, spread: i64| Price {
            symbol: symbol.to_owned(),
            bid,
            ask,
            last: 0.0,
            spread_points: spread,
            digits,
            time: now,
        };
        let deal = |ticket: u64,
                    position_id: u64,
                    symbol: &str,
                    deal_type: DealType,
                    entry: DealEntry,
                    volume: f64,
                    price: f64,
                    profit: f64,
                    commission: f64,
                    time: i64,
                    comment: &str| Deal {
            ticket,
            order: if position_id == 0 { 0 } else { ticket + 1_000 },
            position_id,
            symbol: symbol.to_owned(),
            deal_type,
            entry,
            volume,
            price,
            profit,
            commission,
            swap: 0.0,
            time,
            comment: comment.to_owned(),
        };
        Self {
            ping: PingInfo {
                protocol_version: crate::protocol::PROTOCOL_VERSION,
                ea_version: "mock-0.1.0".to_owned(),
                trade_mode: TradeMode::Demo,
                terminal_connected: true,
                trade_allowed: true,
                server_time: now,
            },
            account: Account {
                currency: "USD".to_owned(),
                balance: 10000.0,
                equity: 10070.55,
                profit: 70.55,
                margin: 600.0,
                free_margin: 9470.55,
                margin_level: 1678.43,
                leverage: 100,
                trade_mode: TradeMode::Demo,
            },
            prices: vec![
                price("XAUUSD", 2391.12, 2391.40, 2, 28),
                price("US100", 19822.5, 19823.3, 1, 8),
                price("US30", 43120.0, 43121.5, 1, 15),
                price("EURUSD", 1.08462, 1.0847, 5, 8),
                price("GBPUSD", 1.29315, 1.29326, 5, 11),
                price("BTCUSD", 71250.4, 71262.4, 2, 1200),
            ],
            positions: vec![
                Position {
                    ticket: 1_001_001,
                    symbol: "XAUUSD".to_owned(),
                    side: Side::Buy,
                    volume: 0.10,
                    price_open: 2385.40,
                    price_current: 2391.12,
                    sl: None,
                    tp: Some(2410.0),
                    profit: 57.20,
                    swap: -0.35,
                    magic: 0,
                    comment: String::new(),
                    time: now - 3 * h,
                },
                Position {
                    ticket: 1_001_002,
                    symbol: "US100".to_owned(),
                    side: Side::Sell,
                    volume: 0.50,
                    price_open: 19850.0,
                    price_current: 19823.3,
                    sl: Some(19920.0),
                    tp: None,
                    profit: 13.35,
                    swap: 0.0,
                    magic: 0,
                    comment: String::new(),
                    time: now - 90 * 60,
                },
            ],
            orders: vec![Order {
                ticket: 2_001_001,
                symbol: "EURUSD".to_owned(),
                order_type: OrderType::BuyLimit,
                volume_initial: 0.20,
                volume_current: 0.20,
                price_open: 1.08,
                sl: Some(1.075),
                tp: Some(1.09),
                time_setup: now - d,
                comment: String::new(),
            }],
            deals: vec![
                deal(
                    3_000_001,
                    0,
                    "",
                    DealType::Balance,
                    DealEntry::In,
                    0.0,
                    0.0,
                    10000.0,
                    0.0,
                    now - 30 * d,
                    "Deposit",
                ),
                deal(
                    3_000_002,
                    900_001,
                    "EURUSD",
                    DealType::Buy,
                    DealEntry::In,
                    0.10,
                    1.0821,
                    0.0,
                    -0.35,
                    now - 2 * d,
                    "",
                ),
                deal(
                    3_000_003,
                    900_001,
                    "EURUSD",
                    DealType::Sell,
                    DealEntry::Out,
                    0.10,
                    1.0846,
                    25.0,
                    -0.35,
                    now - d - 2 * h,
                    "",
                ),
                deal(
                    3_000_004,
                    1_001_001,
                    "XAUUSD",
                    DealType::Buy,
                    DealEntry::In,
                    0.10,
                    2385.40,
                    0.0,
                    -0.70,
                    now - 3 * h,
                    "",
                ),
                deal(
                    3_000_005,
                    1_001_002,
                    "US100",
                    DealType::Sell,
                    DealEntry::In,
                    0.50,
                    19850.0,
                    0.0,
                    -1.25,
                    now - 90 * 60,
                    "",
                ),
            ],
            drop_next: 0,
        }
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Counters shared between the handle and the server task.
#[derive(Debug, Default)]
struct Shared {
    drop_next: AtomicUsize,
    requests: AtomicUsize,
}

/// A running mock EA. Dropping it stops the server.
pub struct MockBridge {
    /// Endpoint to pass to `BridgeConfig`, e.g. `tcp://127.0.0.1:53211`.
    pub endpoint: String,
    shared: Arc<Shared>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl MockBridge {
    /// Start on a free loopback port with `MockState::default()`.
    pub async fn start() -> Self {
        Self::start_with(MockState::default()).await
    }

    /// Start with custom state.
    ///
    /// # Panics
    /// If no loopback port can be bound.
    pub async fn start_with(state: MockState) -> Self {
        match Self::start_at("tcp://127.0.0.1:0", state).await {
            Ok(mock) => mock,
            Err(e) => panic!("MockBridge could not bind a loopback port: {e}"),
        }
    }

    /// Start on a specific endpoint (e.g. `tcp://127.0.0.1:5555`, or port `0` for any free
    /// port). Lets tests create a client first and bring the "EA" up later, or restart it
    /// on the same port.
    pub async fn start_at(endpoint: &str, state: MockState) -> Result<Self, BridgeError> {
        let mut socket = RepSocket::new();
        let bound = socket
            .bind(endpoint)
            .await
            .map_err(|e| BridgeError::Transport(format!("mock bind failed: {e}")))?;
        let shared = Arc::new(Shared::default());
        shared.drop_next.store(state.drop_next, Ordering::SeqCst);
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(serve(socket, Arc::new(state), shared.clone(), stopped));
        Ok(Self {
            endpoint: bound.to_string(),
            shared,
            stop: Some(stop),
            task: Some(task),
        })
    }

    /// Make the mock swallow the next `n` requests without replying.
    pub fn drop_next(&self, n: usize) {
        self.shared.drop_next.store(n, Ordering::SeqCst);
    }

    /// Number of requests received so far (including swallowed ones).
    pub fn request_count(&self) -> usize {
        self.shared.requests.load(Ordering::SeqCst)
    }

    /// Stop the server and wait until its port is released.
    pub async fn stop(mut self) {
        drop(self.stop.take());
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for MockBridge {
    fn drop(&mut self) {
        // Dropping the sender wakes the server task, which closes the socket and exits.
        drop(self.stop.take());
    }
}

async fn serve(
    mut socket: RepSocket,
    state: Arc<MockState>,
    shared: Arc<Shared>,
    mut stopped: oneshot::Receiver<()>,
) {
    loop {
        let request = tokio::select! {
            _ = &mut stopped => break,
            received = socket.recv() => received,
        };
        let request = match request {
            Ok(request) => request,
            Err(e) => {
                // A client connection ended (normal after a client-side timeout reset).
                tracing::debug!(error = %e, "mock bridge: recv error");
                tokio::time::sleep(Duration::from_millis(5)).await;
                continue;
            }
        };
        shared.requests.fetch_add(1, Ordering::SeqCst);
        let swallow = shared
            .drop_next
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        if swallow {
            continue;
        }
        let payload = request
            .get(0)
            .map(|frame| frame.as_ref())
            .unwrap_or_default();
        let reply = handle(&state, payload);
        let reply = match serde_json::to_vec(&reply) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        if let Err(e) = socket.send(ZmqMessage::from(reply)).await {
            tracing::debug!(error = %e, "mock bridge: client went away before the reply");
        }
    }
    socket.close().await;
}

fn ok_reply<T: Serialize>(id: &str, data: &T) -> Reply {
    match serde_json::to_value(data) {
        Ok(value) => Reply {
            id: id.to_owned(),
            ok: true,
            data: Some(value),
            error: None,
            retcode: 0,
        },
        Err(e) => err_reply(id, RETCODE_INVALID_REQUEST, &format!("encode failed: {e}")),
    }
}

fn err_reply(id: &str, retcode: i64, message: &str) -> Reply {
    Reply {
        id: id.to_owned(),
        ok: false,
        data: None,
        error: Some(message.to_owned()),
        retcode,
    }
}

fn unknown_symbol(id: &str, symbol: &str) -> Reply {
    err_reply(
        id,
        RETCODE_UNKNOWN_SYMBOL,
        &format!("unknown symbol: {symbol}"),
    )
}

/// Decode one request and produce its reply. Never panics on bad input.
fn handle(state: &MockState, payload: &[u8]) -> Reply {
    let value: Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(e) => {
            return err_reply(
                "",
                RETCODE_INVALID_REQUEST,
                &format!("malformed request: {e}"),
            );
        }
    };
    let Some(id) = value.get("id").and_then(Value::as_str) else {
        return err_reply("", RETCODE_INVALID_REQUEST, "malformed request: missing id");
    };
    let Some(cmd) = value.get("cmd").and_then(Value::as_str) else {
        return err_reply(
            id,
            RETCODE_INVALID_REQUEST,
            "malformed request: missing cmd",
        );
    };
    if !KNOWN_COMMANDS.contains(&cmd) {
        return err_reply(
            id,
            RETCODE_UNKNOWN_COMMAND,
            &format!("unknown command: {cmd}"),
        );
    }
    let request: Request = match serde_json::from_value(value.clone()) {
        Ok(r) => r,
        Err(e) => {
            return err_reply(
                id,
                RETCODE_INVALID_REQUEST,
                &format!("invalid request: {e}"),
            );
        }
    };

    match request.command {
        Command::Ping {} => ok_reply(id, &state.ping),
        Command::GetAccount {} => ok_reply(id, &state.account),
        Command::GetPrice { symbol } => match state.prices.iter().find(|p| p.symbol == symbol) {
            Some(price) => ok_reply(id, price),
            None => unknown_symbol(id, &symbol),
        },
        Command::GetPositions { symbol } => {
            if let Some(s) = symbol.as_deref().filter(|s| !state.knows(s)) {
                return unknown_symbol(id, s);
            }
            let positions = state
                .positions
                .iter()
                .filter(|p| symbol.as_deref().is_none_or(|s| p.symbol == s))
                .cloned()
                .collect();
            ok_reply(id, &Positions { positions })
        }
        Command::GetOrders { symbol } => {
            if let Some(s) = symbol.as_deref().filter(|s| !state.knows(s)) {
                return unknown_symbol(id, s);
            }
            let orders = state
                .orders
                .iter()
                .filter(|o| symbol.as_deref().is_none_or(|s| o.symbol == s))
                .cloned()
                .collect();
            ok_reply(id, &Orders { orders })
        }
        Command::GetHistory(query) => history(state, id, &query),
    }
}

fn history(state: &MockState, id: &str, query: &HistoryQuery) -> Reply {
    let limit = query.limit.unwrap_or(HISTORY_LIMIT_DEFAULT);
    if !(1..=HISTORY_LIMIT_MAX).contains(&limit) {
        return err_reply(
            id,
            RETCODE_INVALID_REQUEST,
            &format!("limit must be between 1 and {HISTORY_LIMIT_MAX}"),
        );
    }
    if query.from > query.to {
        return err_reply(id, RETCODE_INVALID_REQUEST, "from must not be after to");
    }
    if let Some(s) = query.symbol.as_deref().filter(|s| !state.knows(s)) {
        return unknown_symbol(id, s);
    }
    let mut matching: Vec<&Deal> = state
        .deals
        .iter()
        .filter(|d| d.time >= query.from && d.time <= query.to)
        .filter(|d| query.symbol.as_deref().is_none_or(|s| d.symbol == s))
        .collect();
    // Newest first, so a truncated reply keeps the most recent deals.
    matching.sort_by_key(|d| std::cmp::Reverse(d.time));
    let limit = limit as usize;
    let truncated = matching.len() > limit;
    let deals = matching.into_iter().take(limit).cloned().collect();
    ok_reply(id, &History { deals, truncated })
}

impl MockState {
    /// A symbol is "known" when it has a price.
    fn knows(&self, symbol: &str) -> bool {
        self.prices.iter().any(|p| p.symbol == symbol)
    }
}
