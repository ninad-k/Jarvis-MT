//! Async bridge client: one actor task owns the ZeroMQ REQ socket.
//!
//! REQ/REP is strictly lock-step: after a request is sent, the socket can only receive.
//! If a reply is lost the socket is wedged forever, so the actor
//!
//! - keeps exactly one request in flight (callers queue on an mpsc channel),
//! - serves queued writes before queued reads (a separate lane, see below),
//! - drops the socket after any timeout, transport error or protocol error, and
//! - connects lazily, so a missing EA only affects the call that needs it.
//!
//! # Write priority
//!
//! Reads and writes queue on two channels and the actor always drains the write lane first.
//! A confirmed write therefore waits for at most the one exchange that is already in flight
//! (at most the read timeout, 3 s by default) plus any writes queued ahead of it, never for a
//! whole line of reads. The exchange in flight cannot be taken back: REQ/REP is lock-step.
use std::time::Duration;

use serde::de::DeserializeOwned;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use uuid::Uuid;
use zeromq::{ReqSocket, Socket, SocketRecv, SocketSend};

use crate::auth::{self, AuthFailure, BridgeKey};
use crate::error::BridgeError;
use crate::protocol::{
    Account, BrokerInfo, CatalogSymbol, Command, History, HistoryQuery, LIST_SYMBOLS_LIMIT_MAX,
    ListSymbolsQuery, OrderCancelArgs, OrderSendArgs, Orders, PROTOCOL_VERSION, PingInfo,
    PositionCloseArgs, PositionClosePartialArgs, PositionModifyArgs, Positions, Price,
    RETCODE_AUTH_FAILED, Reply, Request, SymbolCatalogPage, SymbolInfo, TradeResult,
};

/// Calls that may wait for the actor at the same time before `call` starts to back-pressure.
const QUEUE_DEPTH: usize = 64;

/// Endpoint used when none is given.
pub const DEFAULT_ENDPOINT: &str = "tcp://127.0.0.1:5555";
/// Per-request timeout used when none is given.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(3000);

/// Connection settings for [`BridgeClient`]. There is no `Default`: a client cannot exist
/// without the shared key (ADR-0016).
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeConfig {
    /// ZeroMQ endpoint of the EA, e.g. `tcp://127.0.0.1:5555`. Must be loopback
    /// (`127.0.0.1`, `localhost` or `[::1]`); `localhost` is connected as `127.0.0.1`.
    pub endpoint: String,
    /// Per-request timeout. It covers the whole call: time spent waiting behind other
    /// calls, connecting, sending and waiting for the reply. Write commands use at least
    /// [`MIN_WRITE_TIMEOUT`], because a trade round trip can take seconds.
    pub timeout: Duration,
    /// The shared key of the EA (`bridge.key`). Every message is signed with it and every
    /// reply must carry a valid signature.
    pub key: BridgeKey,
}

impl BridgeConfig {
    /// The default endpoint and timeout with this key.
    pub fn with_key(key: BridgeKey) -> Self {
        Self {
            endpoint: DEFAULT_ENDPOINT.to_owned(),
            timeout: DEFAULT_TIMEOUT,
            key,
        }
    }

    /// All three settings.
    pub fn new(endpoint: impl Into<String>, timeout: Duration, key: BridgeKey) -> Self {
        Self {
            endpoint: endpoint.into(),
            timeout,
            key,
        }
    }
}

/// Lower bound on the timeout of write commands. A write that times out has an
/// unknown outcome, so a generous budget avoids needless "outcome unknown" results.
pub const MIN_WRITE_TIMEOUT: Duration = Duration::from_secs(15);

/// Async client for the EA. Cheap to clone; all clones share one socket actor.
///
/// When the last clone is dropped the actor stops and the socket is closed.
#[derive(Debug, Clone)]
pub struct BridgeClient {
    /// Read lane.
    tx: mpsc::Sender<Job>,
    /// Write lane: served first.
    write_tx: mpsc::Sender<Job>,
}

impl BridgeClient {
    /// Create the client and start its actor. Connecting is lazy: this succeeds even
    /// when the EA is not running; calls then fail with `Timeout`/`Transport`.
    /// Rejects non-loopback endpoints with `BridgeError::Transport`.
    ///
    /// The actor is spawned with the current Tokio runtime, so this must be called
    /// from inside one (otherwise it returns `BridgeError::Transport`, it does not panic).
    pub fn new(config: BridgeConfig) -> Result<Self, BridgeError> {
        let write_timeout = config.timeout.max(MIN_WRITE_TIMEOUT);
        Self::with_write_timeout(config, write_timeout)
    }

    /// Like [`BridgeClient::new`], with an explicit timeout for write commands instead
    /// of `max(config.timeout, MIN_WRITE_TIMEOUT)`. Mainly for tests.
    pub fn with_write_timeout(
        config: BridgeConfig,
        write_timeout: Duration,
    ) -> Result<Self, BridgeError> {
        let target = Target::parse(&config.endpoint)?;
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            BridgeError::Transport("BridgeClient::new must be called inside a Tokio runtime".into())
        })?;
        let (tx, rx) = mpsc::channel(QUEUE_DEPTH);
        let (write_tx, write_rx) = mpsc::channel(QUEUE_DEPTH);
        let actor = Actor {
            target,
            key: config.key,
            timeout: config.timeout,
            write_timeout,
            socket: None,
        };
        runtime.spawn(actor.run(rx, write_rx));
        Ok(Self { tx, write_tx })
    }

    /// Send one command and decode the reply data into `T`.
    ///
    /// Failure modes:
    /// - `Transport`: the EA is not reachable (connection refused) or the socket broke.
    /// - `Timeout`: no reply within the configured timeout. The next call starts on a new socket.
    /// - `Protocol`: the reply was not valid, its `id` did not match, or `data` did not match `T`.
    /// - `Remote`: the EA answered `ok: false`.
    pub async fn call<T: DeserializeOwned>(&self, command: Command) -> Result<T, BridgeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let lane = if command.is_write() {
            &self.write_tx
        } else {
            &self.tx
        };
        let job = Job {
            command,
            enqueued: Instant::now(),
            reply: reply_tx,
        };
        lane.send(job).await.map_err(|_| actor_stopped())?;
        let reply = reply_rx.await.map_err(|_| actor_stopped())??;
        reply.decode().map_err(|e| match BridgeError::from(e) {
            BridgeError::Remote { retcode, .. } if retcode == RETCODE_AUTH_FAILED => {
                BridgeError::Remote {
                    message: format!(
                        "{}: the EA refused the request (bridge key mismatch, PC clock more \
                         than 30 s off, or a repeated request id)",
                        auth::AUTH_ERROR_TEXT
                    ),
                    retcode,
                }
            }
            other => other,
        })
    }

    /// Health check. Also refuses an EA that speaks another protocol version.
    pub async fn ping(&self) -> Result<PingInfo, BridgeError> {
        let info: PingInfo = self.call(Command::Ping {}).await?;
        if info.protocol_version != PROTOCOL_VERSION {
            return Err(BridgeError::Protocol(format!(
                "the EA speaks protocol version {} but this client speaks {PROTOCOL_VERSION}; \
                 reinstall the EA with scripts/install-ea.ps1 and recompile it",
                info.protocol_version
            )));
        }
        Ok(info)
    }

    pub async fn get_price(&self, symbol: &str) -> Result<Price, BridgeError> {
        self.call(Command::GetPrice {
            symbol: symbol.to_owned(),
        })
        .await
    }

    pub async fn get_account(&self) -> Result<Account, BridgeError> {
        self.call(Command::GetAccount {}).await
    }

    pub async fn get_positions(&self, symbol: Option<&str>) -> Result<Positions, BridgeError> {
        self.call(Command::GetPositions {
            symbol: symbol.map(str::to_owned),
        })
        .await
    }

    pub async fn get_orders(&self, symbol: Option<&str>) -> Result<Orders, BridgeError> {
        self.call(Command::GetOrders {
            symbol: symbol.map(str::to_owned),
        })
        .await
    }

    pub async fn get_history(&self, query: HistoryQuery) -> Result<History, BridgeError> {
        self.call(Command::GetHistory(query)).await
    }

    pub async fn get_broker(&self) -> Result<BrokerInfo, BridgeError> {
        self.call(Command::GetBroker {}).await
    }

    pub async fn list_symbols(
        &self,
        query: ListSymbolsQuery,
    ) -> Result<SymbolCatalogPage, BridgeError> {
        self.call(Command::ListSymbols(query)).await
    }

    /// The broker's whole symbol catalog, fetched in pages of [`LIST_SYMBOLS_LIMIT_MAX`] and
    /// returned in the EA's name order.
    ///
    /// The catalog can change while it is being read (the broker adds a symbol, or the user
    /// switches account in MT5). A page whose `total` differs from the first page of the pass,
    /// or an unexpectedly empty page, makes the pass start again from offset 0; this is
    /// tolerated twice. A third inconsistent pass fails with [`BridgeError::Protocol`], as does
    /// a catalog of more than [`LIST_ALL_SYMBOLS_CAP`] symbols. Transport errors, timeouts and
    /// `Remote` errors of any page are returned as they are.
    pub async fn list_all_symbols(&self) -> Result<Vec<CatalogSymbol>, BridgeError> {
        let mut changes = 0_u32;
        loop {
            match self.list_all_symbols_pass().await? {
                Pass::Complete(symbols) => return Ok(symbols),
                Pass::Changed => {
                    changes += 1;
                    if changes > LIST_ALL_SYMBOLS_RESTARTS {
                        return Err(BridgeError::Protocol(format!(
                            "the broker's symbol catalog kept changing while it was being read                              ({changes} attempts)"
                        )));
                    }
                    tracing::debug!(changes, "symbol catalog changed while paging; restarting");
                }
            }
        }
    }

    /// One pass over all pages from offset 0.
    async fn list_all_symbols_pass(&self) -> Result<Pass, BridgeError> {
        let mut symbols: Vec<CatalogSymbol> = Vec::new();
        let mut expected_total: Option<u32> = None;
        loop {
            let offset = u32::try_from(symbols.len()).map_err(|_| catalog_too_large())?;
            let page = self
                .list_symbols(ListSymbolsQuery {
                    offset,
                    limit: Some(LIST_SYMBOLS_LIMIT_MAX),
                })
                .await?;
            let total = *expected_total.get_or_insert(page.total);
            if page.total as usize > LIST_ALL_SYMBOLS_CAP {
                return Err(catalog_too_large());
            }
            if page.total != total || page.offset != offset {
                return Ok(Pass::Changed);
            }
            if offset >= total {
                // Past the end: done, unless symbols vanished and the list is now short.
                return Ok(if symbols.len() == total as usize {
                    Pass::Complete(symbols)
                } else {
                    Pass::Changed
                });
            }
            if page.symbols.is_empty() {
                // The EA lists fewer symbols than it promised: the catalog shrank.
                return Ok(Pass::Changed);
            }
            symbols.extend(page.symbols);
            if symbols.len() > LIST_ALL_SYMBOLS_CAP {
                return Err(catalog_too_large());
            }
            if symbols.len() >= total as usize {
                return Ok(if symbols.len() == total as usize {
                    Pass::Complete(symbols)
                } else {
                    Pass::Changed
                });
            }
        }
    }

    pub async fn get_symbol(&self, symbol: &str) -> Result<SymbolInfo, BridgeError> {
        self.call(Command::GetSymbol {
            symbol: symbol.to_owned(),
        })
        .await
    }

    // ---- Write commands. Callers must only use these after a confirmed gate
    // ---- preview. A failed trade (`ok:false`) comes back as `BridgeError::Remote`
    // ---- with the MT5 trade retcode; a `Timeout` after sending means the outcome
    // ---- is UNKNOWN and must be reconciled by reading positions, never retried.

    pub async fn order_send(&self, args: OrderSendArgs) -> Result<TradeResult, BridgeError> {
        self.call(Command::OrderSend(args)).await
    }

    pub async fn position_modify(
        &self,
        args: PositionModifyArgs,
    ) -> Result<TradeResult, BridgeError> {
        self.call(Command::PositionModify(args)).await
    }

    pub async fn position_close(
        &self,
        args: PositionCloseArgs,
    ) -> Result<TradeResult, BridgeError> {
        self.call(Command::PositionClose(args)).await
    }

    pub async fn position_close_partial(
        &self,
        args: PositionClosePartialArgs,
    ) -> Result<TradeResult, BridgeError> {
        self.call(Command::PositionClosePartial(args)).await
    }

    pub async fn order_cancel(&self, args: OrderCancelArgs) -> Result<TradeResult, BridgeError> {
        self.call(Command::OrderCancel(args)).await
    }
}

/// Most symbols [`BridgeClient::list_all_symbols`] will collect. Real brokers have up to
/// about 12,000.
pub const LIST_ALL_SYMBOLS_CAP: usize = 100_000;

/// How many times a catalog read that was overtaken by a change is started again.
const LIST_ALL_SYMBOLS_RESTARTS: u32 = 2;

/// Result of one pass of [`BridgeClient::list_all_symbols`].
enum Pass {
    Complete(Vec<CatalogSymbol>),
    /// The catalog changed under the reader; start again.
    Changed,
}

fn catalog_too_large() -> BridgeError {
    BridgeError::Protocol(format!(
        "the broker's symbol catalog is larger than {LIST_ALL_SYMBOLS_CAP} symbols"
    ))
}

fn actor_stopped() -> BridgeError {
    BridgeError::Transport("bridge client task is not running (Tokio runtime shut down?)".into())
}

/// One queued call.
struct Job {
    command: Command,
    /// When the caller started waiting; the call's deadline is `enqueued + timeout`.
    enqueued: Instant,
    reply: oneshot::Sender<Result<Reply, BridgeError>>,
}

/// A validated loopback endpoint.
#[derive(Debug, Clone)]
struct Target {
    /// Normalised endpoint string, handed to ZeroMQ (`localhost` becomes `127.0.0.1`).
    endpoint: String,
    /// Host without brackets, for the TCP reachability probe.
    host: String,
    port: u16,
}

impl Target {
    /// Accepts exactly `tcp://127.0.0.1:<port>`, `tcp://localhost:<port>`, `tcp://[::1]:<port>`.
    fn parse(endpoint: &str) -> Result<Self, BridgeError> {
        let reject = |why: &str| {
            BridgeError::Transport(format!(
                "invalid bridge endpoint {endpoint:?}: {why} \
                 (expected tcp://127.0.0.1:<port>, tcp://localhost:<port> or tcp://[::1]:<port>)"
            ))
        };
        let rest = endpoint
            .strip_prefix("tcp://")
            .ok_or_else(|| reject("not a tcp:// endpoint"))?;
        let (host, port) = rest
            .rsplit_once(':')
            .ok_or_else(|| reject("missing port"))?;
        let port: u16 = port.parse().map_err(|_| reject("invalid port"))?;
        if port == 0 {
            return Err(reject("port must not be 0"));
        }
        // `localhost` is pinned to IPv4: the EA (libzmq) binds 127.0.0.1, and on Windows a
        // refused attempt on `::1` costs ~2 s before the resolver falls back to IPv4.
        let (host, endpoint) = match host {
            "127.0.0.1" => ("127.0.0.1", endpoint.to_owned()),
            "[::1]" => ("::1", endpoint.to_owned()),
            h if h.eq_ignore_ascii_case("localhost") => {
                ("127.0.0.1", format!("tcp://127.0.0.1:{port}"))
            }
            _ => return Err(reject("only loopback hosts are allowed")),
        };
        Ok(Self {
            endpoint,
            host: host.to_owned(),
            port,
        })
    }
}

struct Actor {
    target: Target,
    key: BridgeKey,
    timeout: Duration,
    write_timeout: Duration,
    /// Connected socket, kept between calls. `None` until the first call and after any failure.
    socket: Option<ReqSocket>,
}

impl Actor {
    async fn run(mut self, mut reads: mpsc::Receiver<Job>, mut writes: mpsc::Receiver<Job>) {
        loop {
            // `biased` makes the write lane win whenever both have a job. Both senders live in
            // the same `BridgeClient`, so the lanes close together; a closed, empty write lane
            // falls through to the read lane, which ends the loop once it is drained too.
            let job = tokio::select! {
                biased;
                Some(job) = writes.recv() => job,
                job = reads.recv() => match job {
                    Some(job) => job,
                    None => break,
                },
            };
            if job.reply.is_closed() {
                // Caller gave up while queued; nothing was sent, so skip it.
                continue;
            }
            let budget = if job.command.is_write() {
                self.write_timeout
            } else {
                self.timeout
            };
            let deadline = job.enqueued + budget;
            let result = self.handle(job.command, deadline, budget).await;
            // The caller may have been cancelled meanwhile; the exchange still ran to completion
            // (or reset the socket), so lock-step stays intact.
            let _ = job.reply.send(result);
        }
        tracing::debug!(endpoint = %self.target.endpoint, "bridge client stopped");
    }

    async fn handle(
        &mut self,
        command: Command,
        deadline: Instant,
        budget: Duration,
    ) -> Result<Reply, BridgeError> {
        let id = Uuid::new_v4().to_string();
        let cmd = command.name();
        let started = Instant::now();
        let outcome = match serde_json::to_vec(&Request {
            id: id.clone(),
            ts: Some(auth::now_ms()),
            command,
        }) {
            Ok(payload) => self.exchange(&id, payload, deadline, budget).await,
            Err(e) => Err(BridgeError::Protocol(format!(
                "could not encode request: {e}"
            ))),
        };
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match &outcome {
            Ok(r) if r.ok => {
                tracing::debug!(%id, cmd, duration_ms, ok = true, retcode = r.retcode, "bridge call");
            }
            Ok(r) => {
                tracing::warn!(
                    %id, cmd, duration_ms, ok = false, retcode = r.retcode,
                    error = r.error.as_deref().unwrap_or(""), "bridge call returned an error"
                );
            }
            Err(e) => {
                tracing::warn!(%id, cmd, duration_ms, ok = false, error = %e, "bridge call failed");
            }
        }
        outcome
    }

    /// One request/reply cycle. The socket is put back only after a fully valid reply;
    /// on any other outcome (including cancellation by the timeout) it is dropped.
    async fn exchange(
        &mut self,
        id: &str,
        payload: Vec<u8>,
        deadline: Instant,
        budget: Duration,
    ) -> Result<Reply, BridgeError> {
        let timeout_ms = u64::try_from(budget.as_millis()).unwrap_or(u64::MAX);
        if Instant::now() >= deadline {
            // Spent the whole budget waiting behind earlier calls. Nothing was sent.
            return Err(BridgeError::Timeout(timeout_ms));
        }
        let socket = self.socket.take();
        let round_trip = round_trip(&self.target, &self.key, socket, payload);
        match tokio::time::timeout_at(deadline, round_trip).await {
            Err(_elapsed) => Err(BridgeError::Timeout(timeout_ms)),
            Ok(Err(e)) => Err(e),
            Ok(Ok((reply, socket))) => {
                // A signed "authentication failed" reply to a request the EA could not
                // authenticate carries an empty id (it never echoes unauthenticated input).
                let refused = reply.id.is_empty() && reply.retcode == RETCODE_AUTH_FAILED;
                if reply.id != id && !refused {
                    return Err(BridgeError::Protocol(format!(
                        "reply id {:?} does not match request id {id:?}",
                        reply.id
                    )));
                }
                self.socket = Some(socket);
                Ok(reply)
            }
        }
    }
}

/// Sign and send `payload`, then wait for one signed reply, connecting first when there is no
/// live socket. If a reused socket turns out to be dead at send time, retries once on a fresh
/// socket (nothing has been delivered in that case, so this is safe for write commands too).
async fn round_trip(
    target: &Target,
    key: &BridgeKey,
    existing: Option<ReqSocket>,
    payload: Vec<u8>,
) -> Result<(Reply, ReqSocket), BridgeError> {
    let message = auth::seal(key, payload);
    let (mut socket, reused) = match existing {
        Some(socket) => (socket, true),
        None => (connect(target).await?, false),
    };
    if let Err(e) = socket.send(message.clone()).await {
        if !reused {
            return Err(BridgeError::Transport(format!("send failed: {e}")));
        }
        tracing::debug!(error = %e, "stale bridge socket, reconnecting");
        socket = connect(target).await?;
        socket
            .send(message)
            .await
            .map_err(|e| BridgeError::Transport(format!("send failed: {e}")))?;
    }
    let frames = socket
        .recv()
        .await
        .map_err(|e| BridgeError::Transport(format!("receive failed: {e}")))?;
    let payload = auth::open(key, &frames).map_err(|failure| match failure {
        AuthFailure::Frames => BridgeError::Protocol(format!(
            "expected a signed two-frame reply, got {} frame(s); is the EA older than protocol \
             version {PROTOCOL_VERSION}?",
            frames.len()
        )),
        _ => {
            tracing::warn!(
                "a reply failed authentication: either bridge.key differs between the API and the \
                 EA, or another program is answering on {}",
                target.endpoint
            );
            BridgeError::Protocol("reply authentication failed".into())
        }
    })?;
    let reply = serde_json::from_slice::<Reply>(payload)
        .map_err(|e| BridgeError::Protocol(format!("invalid reply JSON: {e}")))?;
    Ok((reply, socket))
}

/// Connect a fresh REQ socket.
///
/// `zeromq` 0.6 retries a refused connection forever (with a growing back-off of 1.4 s up to
/// 5 s) until its own 30 s connect timeout. That would hide "EA not running" behind a long
/// wait, so a plain TCP probe runs first and fails fast with `Transport`. The caller's deadline
/// still bounds the whole thing, including the ZeroMQ handshake.
async fn connect(target: &Target) -> Result<ReqSocket, BridgeError> {
    let unreachable = |e: &dyn std::fmt::Display| {
        BridgeError::Transport(format!("cannot connect to {}: {e}", target.endpoint))
    };
    drop(
        TcpStream::connect((target.host.as_str(), target.port))
            .await
            .map_err(|e| unreachable(&e))?,
    );
    let mut socket = ReqSocket::new();
    socket
        .connect(&target.endpoint)
        .await
        .map_err(|e| unreachable(&e))?;
    Ok(socket)
}
