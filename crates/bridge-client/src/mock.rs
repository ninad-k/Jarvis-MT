//! In-process ZeroMQ REP server speaking the bridge protocol with canned data.
//! Used by unit tests here and in `jarvis-api`, and for offline development.
//!
//! It enforces the same authentication as the EA (protocol 2, ADR-0016): a request must be a
//! two-frame message with a valid HMAC, a `ts` within 30 s and an `id` not seen in the last
//! 60 s, otherwise it is answered with a signed `-4` "authentication failed" and nothing runs.
//! Every reply is signed. The key is random per mock unless one is given.
//!
//! The server is a spawned task around `zeromq::RepSocket`. That socket does not enforce
//! strict REP lock-step (a request that is never answered simply leaves the socket ready for
//! the next `recv`), so [`MockBridge::drop_next`] is just "receive and do not reply". A client
//! that times out and reconnects is served normally afterwards; its old connection ends and
//! the mock ignores the resulting disconnect.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use zeromq::{RepSocket, Socket, SocketRecv, SocketSend};

use crate::auth::{self, AUTH_ERROR_TEXT, BridgeKey, ReplayGuard};
use crate::client::BridgeConfig;
use crate::error::BridgeError;
pub use crate::mock_catalog::{default_catalog, generated_catalog};
use crate::mock_trade::Engine;
use crate::protocol::{
    Account, BrokerInfo, CatalogSymbol, Command, Deal, DealEntry, DealType, HISTORY_LIMIT_DEFAULT,
    HISTORY_LIMIT_MAX, History, HistoryQuery, LIST_SYMBOLS_LIMIT_MAX, ListSymbolsQuery, Order,
    OrderType, Orders, PingInfo, Position, Positions, Price, RETCODE_AUTH_FAILED,
    RETCODE_INVALID_REQUEST, RETCODE_UNKNOWN_COMMAND, Reply, Request, Side, SymbolCatalogPage,
    SymbolInfo, TradeMode,
};

/// MT5 `ERR_MARKET_UNKNOWN_SYMBOL`.
const RETCODE_UNKNOWN_SYMBOL: i64 = 4301;

/// `list_symbols` page size when the request gives none (same as the EA).
const LIST_SYMBOLS_LIMIT_DEFAULT: u32 = 500;

/// Every command name the mock (and protocol version 1) knows.
const KNOWN_COMMANDS: [&str; 14] = [
    "ping",
    "get_price",
    "get_account",
    "get_positions",
    "get_orders",
    "get_history",
    "get_symbol",
    "get_broker",
    "list_symbols",
    "order_send",
    "position_modify",
    "position_close",
    "position_close_partial",
    "order_cancel",
];

/// State served by [`MockBridge`]. `Default` gives a demo account with
/// XAUUSD, US100, US30, EURUSD, GBPUSD, BTCUSD prices and trading specifications, two open
/// positions, one pending order and five deals (one of them a balance deal). Deal and position
/// times are relative to "now" at the moment `default()` is called.
///
/// Write commands change this state: `order_send` adds a position or a pending order,
/// `position_modify` updates sl/tp, closes remove or reduce positions (and realise profit into
/// `account`), `order_cancel` removes an order. Read it back with [`MockBridge::state`].
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
    /// Trading specification per symbol (`get_symbol`, and the validation of write commands).
    pub symbols: Vec<SymbolInfo>,
    /// Simulate an account the EA refuses to write to: every write answers `-3` after its
    /// arguments were validated. A `Real` `ping.trade_mode` behaves the same unless
    /// `allow_real` is set (the EA input `InpAllowReal`).
    pub refuse_writes: bool,
    /// Mirrors the EA input `InpAllowReal`.
    pub allow_real: bool,
    /// The next write that passes validation fails with this trade retcode (for example
    /// `10019` no money, `10018` market closed) and changes nothing. Consumed by that write.
    pub fail_next_write: Option<i64>,
    /// `get_broker` reply: the company and server the mock pretends to be logged into.
    /// Default: company "Mock Broker Ltd.", server "Mock-Demo", a demo account.
    pub broker: BrokerInfo,
    /// The broker's full symbol catalog served by `list_symbols`, in any order (the mock sorts
    /// by name, byte-wise, as the EA's list is ordered). Default: [`default_catalog`].
    pub catalog: Vec<CatalogSymbol>,
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
            symbols: default_symbols(),
            refuse_writes: false,
            allow_real: false,
            fail_next_write: None,
            broker: BrokerInfo {
                company: "Mock Broker Ltd.".to_owned(),
                server: "Mock-Demo".to_owned(),
                trade_mode: TradeMode::Demo,
            },
            catalog: default_catalog(),
        }
    }
}

/// Realistic trading specifications for the six mock symbols.
fn default_symbols() -> Vec<SymbolInfo> {
    fn spec(
        symbol: &str,
        description: &str,
        digits: u32,
        volume: (f64, f64, f64),
        contract_size: f64,
        tick_value: f64,
        stops_level: i64,
    ) -> SymbolInfo {
        let point = 10f64.powi(-i32::try_from(digits).unwrap_or(0));
        SymbolInfo {
            symbol: symbol.to_owned(),
            description: description.to_owned(),
            digits,
            point,
            volume_min: volume.0,
            volume_max: volume.1,
            volume_step: volume.2,
            contract_size,
            tick_value,
            tick_size: point,
            stops_level,
            trade_allowed: true,
            currency_profit: "USD".to_owned(),
        }
    }
    vec![
        spec(
            "XAUUSD",
            "Gold vs US Dollar",
            2,
            (0.01, 100.0, 0.01),
            100.0,
            1.0,
            10,
        ),
        spec(
            "US100",
            "US Tech 100 Index",
            1,
            (0.1, 100.0, 0.1),
            1.0,
            0.1,
            50,
        ),
        spec(
            "US30",
            "Wall Street 30 Index",
            1,
            (0.1, 100.0, 0.1),
            1.0,
            0.1,
            50,
        ),
        spec(
            "EURUSD",
            "Euro vs US Dollar",
            5,
            (0.01, 100.0, 0.01),
            100_000.0,
            1.0,
            0,
        ),
        spec(
            "GBPUSD",
            "Great Britain Pound vs US Dollar",
            5,
            (0.01, 100.0, 0.01),
            100_000.0,
            1.0,
            0,
        ),
        spec(
            "BTCUSD",
            "Bitcoin vs US Dollar",
            2,
            (0.01, 10.0, 0.01),
            1.0,
            0.01,
            100,
        ),
    ]
}

fn random_key() -> BridgeKey {
    match BridgeKey::generate() {
        Ok(key) => key,
        Err(e) => panic!("MockBridge could not create a random key: {e}"),
    }
}

pub(crate) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Mutable world: the served state plus the write engine (ticket counters, reply cache).
#[derive(Debug)]
struct Inner {
    state: MockState,
    engine: Engine,
    guard: ReplayGuard,
}

/// Counters and state shared between the handle and the server task.
#[derive(Debug)]
struct Shared {
    key: BridgeKey,
    drop_next: AtomicUsize,
    requests: AtomicUsize,
    auth_failures: AtomicUsize,
    allow_replays: AtomicBool,
    inner: Mutex<Inner>,
}

impl Shared {
    /// Lock the world. A panic in another holder cannot corrupt plain data, so poisoning is ignored.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
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
    /// Start on a free loopback port with `MockState::default()` and a random key.
    pub async fn start() -> Self {
        Self::start_with(MockState::default()).await
    }

    /// Start with custom state and a random key.
    ///
    /// # Panics
    /// If no loopback port can be bound.
    pub async fn start_with(state: MockState) -> Self {
        Self::start_with_key(state, random_key()).await
    }

    /// Start with custom state and a given key.
    ///
    /// # Panics
    /// If no loopback port can be bound.
    pub async fn start_with_key(state: MockState, key: BridgeKey) -> Self {
        match Self::start_at_with_key("tcp://127.0.0.1:0", state, key).await {
            Ok(mock) => mock,
            Err(e) => panic!("MockBridge could not bind a loopback port: {e}"),
        }
    }

    /// Start on a specific endpoint (e.g. `tcp://127.0.0.1:5555`, or port `0` for any free
    /// port) with a random key. Use [`MockBridge::start_at_with_key`] to restart "the EA" on
    /// the same port with the key its clients already have.
    pub async fn start_at(endpoint: &str, state: MockState) -> Result<Self, BridgeError> {
        Self::start_at_with_key(endpoint, state, random_key()).await
    }

    /// Start on a specific endpoint with a given key. Lets tests create a client first and
    /// bring the "EA" up later, or restart it on the same port.
    pub async fn start_at_with_key(
        endpoint: &str,
        state: MockState,
        key: BridgeKey,
    ) -> Result<Self, BridgeError> {
        let mut socket = RepSocket::new();
        let bound = socket
            .bind(endpoint)
            .await
            .map_err(|e| BridgeError::Transport(format!("mock bind failed: {e}")))?;
        let shared = Arc::new(Shared {
            key,
            drop_next: AtomicUsize::new(state.drop_next),
            requests: AtomicUsize::new(0),
            auth_failures: AtomicUsize::new(0),
            allow_replays: AtomicBool::new(false),
            inner: Mutex::new(Inner {
                state,
                engine: Engine::default(),
                guard: ReplayGuard::new(),
            }),
        });
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(serve(socket, shared.clone(), stopped));
        Ok(Self {
            endpoint: bound.to_string(),
            shared,
            stop: Some(stop),
            task: Some(task),
        })
    }

    /// The shared key of this mock.
    pub fn key(&self) -> BridgeKey {
        self.shared.key.clone()
    }

    /// A client configuration for this mock: its endpoint, this timeout and its key.
    pub fn config(&self, timeout: Duration) -> BridgeConfig {
        BridgeConfig::new(self.endpoint.clone(), timeout, self.key())
    }

    /// Requests refused by the authentication step (answered with a signed `-4`).
    pub fn auth_failures(&self) -> usize {
        self.shared.auth_failures.load(Ordering::SeqCst)
    }

    /// Test hook: stop refusing repeated request ids (the `ts` window still applies). Only for
    /// exercising the write reply cache, which is the second line of defence behind the replay
    /// guard and unreachable with it on.
    pub fn allow_replays(&self, allow: bool) {
        self.shared.allow_replays.store(allow, Ordering::SeqCst);
    }

    /// Make the mock swallow the next `n` requests without replying.
    pub fn drop_next(&self, n: usize) {
        self.shared.drop_next.store(n, Ordering::SeqCst);
    }

    /// Number of requests received so far (including swallowed ones).
    pub fn request_count(&self) -> usize {
        self.shared.requests.load(Ordering::SeqCst)
    }

    /// A copy of the current state (positions, orders, deals, account after any writes).
    pub fn state(&self) -> MockState {
        self.shared.lock().state.clone()
    }

    /// Change the state while the server runs, e.g. `mock.update_state(|s| s.prices[0].ask += 1.0)`.
    pub fn update_state(&self, change: impl FnOnce(&mut MockState)) {
        change(&mut self.shared.lock().state);
    }

    /// Pretend the MT5 user logged into another account: from now on `get_broker` answers
    /// `info` and `list_symbols` serves `catalog`. Nothing else (prices, positions...) changes.
    /// A client in the middle of `list_all_symbols` sees the catalog change under it.
    pub fn set_broker(&self, info: BrokerInfo, catalog: Vec<CatalogSymbol>) {
        let mut inner = self.shared.lock();
        inner.state.broker = info;
        inner.state.catalog = catalog;
    }

    /// Make every write answer `-3` (a real account without `InpAllowReal`) until switched off.
    pub fn refuse_writes(&self, refuse: bool) {
        self.shared.lock().state.refuse_writes = refuse;
    }

    /// Make the next write that passes validation fail with this trade retcode and change nothing.
    pub fn fail_next_write(&self, retcode: i64) {
        self.shared.lock().state.fail_next_write = Some(retcode);
    }

    /// Writes that reached execution (passed validation and were not answered from the cache),
    /// including ones forced to fail with [`MockBridge::fail_next_write`].
    pub fn executed_writes(&self) -> usize {
        self.shared.lock().engine.executed()
    }

    /// Writes answered from the idempotency cache instead of being executed.
    pub fn cache_hits(&self) -> usize {
        self.shared.lock().engine.cache_hits()
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

async fn serve(mut socket: RepSocket, shared: Arc<Shared>, mut stopped: oneshot::Receiver<()>) {
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
        let swallow = take_one(&shared.drop_next);
        if swallow {
            continue;
        }
        let mut reply = match auth::open(&shared.key, &request) {
            // Not authentic: say so without echoing anything the peer sent, so the signed
            // reply is useless as a signing oracle.
            Err(failure) => {
                tracing::debug!(%failure, "mock bridge: request refused");
                shared.auth_failures.fetch_add(1, Ordering::SeqCst);
                err_reply("", RETCODE_AUTH_FAILED, AUTH_ERROR_TEXT)
            }
            Ok(payload) => {
                let mut inner = shared.lock();
                let Inner {
                    state,
                    engine,
                    guard,
                } = &mut *inner;
                let replays = shared.allow_replays.load(Ordering::SeqCst);
                let reply = handle(state, engine, guard, replays, payload);
                if reply.retcode == RETCODE_AUTH_FAILED {
                    shared.auth_failures.fetch_add(1, Ordering::SeqCst);
                }
                reply
            }
        };
        reply.ts = Some(auth::now_ms());
        let bytes = match serde_json::to_vec(&reply) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        if let Err(e) = socket.send(auth::seal(&shared.key, bytes)).await {
            tracing::debug!(error = %e, "mock bridge: client went away before the reply");
        }
    }
    socket.close().await;
}

pub(crate) fn ok_reply<T: Serialize>(id: &str, data: &T) -> Reply {
    match serde_json::to_value(data) {
        Ok(value) => Reply {
            id: id.to_owned(),
            ok: true,
            data: Some(value),
            error: None,
            retcode: 0,
            ts: None,
        },
        Err(e) => err_reply(id, RETCODE_INVALID_REQUEST, &format!("encode failed: {e}")),
    }
}

pub(crate) fn err_reply(id: &str, retcode: i64, message: &str) -> Reply {
    Reply {
        id: id.to_owned(),
        ok: false,
        data: None,
        error: Some(message.to_owned()),
        retcode,
        ts: None,
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
fn handle(
    state: &mut MockState,
    engine: &mut Engine,
    guard: &mut ReplayGuard,
    allow_replays: bool,
    payload: &[u8],
) -> Reply {
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
    // The MAC is valid, so the sender holds the key; now freshness and replay, before anything
    // is interpreted or executed. The id is echoed because this peer is authentic.
    let fresh = match value.get("ts").and_then(Value::as_i64) {
        Some(ts) if allow_replays => ReplayGuard::new().check(id, ts),
        Some(ts) => guard.check(id, ts),
        None => Err(auth::AuthFailure::Envelope),
    };
    if let Err(failure) = fresh {
        tracing::debug!(%failure, "mock bridge: request refused");
        return err_reply(id, RETCODE_AUTH_FAILED, AUTH_ERROR_TEXT);
    }
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

    if request.command.is_write() {
        return engine.write(state, id, request.command);
    }
    match request.command {
        Command::GetBroker {} => ok_reply(id, &state.broker),
        Command::ListSymbols(query) => list_symbols(state, id, &query),
        Command::GetSymbol { symbol } => match state.symbols.iter().find(|s| s.symbol == symbol) {
            Some(info) => ok_reply(id, info),
            None => unknown_symbol(id, &symbol),
        },
        Command::OrderSend(_)
        | Command::PositionModify(_)
        | Command::PositionClose(_)
        | Command::PositionClosePartial(_)
        | Command::OrderCancel(_) => err_reply(id, RETCODE_UNKNOWN_COMMAND, "unreachable write"),
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

/// `list_symbols`: one page of the catalog, ordered byte-wise by name.
fn list_symbols(state: &MockState, id: &str, query: &ListSymbolsQuery) -> Reply {
    let limit = query.limit.unwrap_or(LIST_SYMBOLS_LIMIT_DEFAULT);
    if !(1..=LIST_SYMBOLS_LIMIT_MAX).contains(&limit) {
        return err_reply(
            id,
            RETCODE_INVALID_REQUEST,
            &format!("'limit' must be between 1 and {LIST_SYMBOLS_LIMIT_MAX}"),
        );
    }
    let mut sorted: Vec<&CatalogSymbol> = state.catalog.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let symbols = sorted
        .into_iter()
        .skip(query.offset as usize)
        .take(limit as usize)
        .cloned()
        .collect();
    ok_reply(
        id,
        &SymbolCatalogPage {
            symbols,
            total: u32::try_from(state.catalog.len()).unwrap_or(u32::MAX),
            offset: query.offset,
        },
    )
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

/// Decrement `counter` if it is above zero; true when one was taken. A plain
/// compare-exchange loop (works on every supported Rust version).
fn take_one(counter: &std::sync::atomic::AtomicUsize) -> bool {
    let mut current = counter.load(Ordering::SeqCst);
    while current > 0 {
        match counter.compare_exchange_weak(
            current,
            current - 1,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => return true,
            Err(actual) => current = actual,
        }
    }
    false
}
