//! Async bridge client: one actor task owns the ZeroMQ REQ socket.
//!
//! REQ/REP is strictly lock-step: after a request is sent, the socket can only receive.
//! If a reply is lost the socket is wedged forever, so the actor
//!
//! - keeps exactly one request in flight (callers queue on an mpsc channel),
//! - drops the socket after any timeout, transport error or protocol error, and
//! - connects lazily, so a missing EA only affects the call that needs it.
use std::time::Duration;

use bytes::Bytes;
use serde::de::DeserializeOwned;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use uuid::Uuid;
use zeromq::{ReqSocket, Socket, SocketRecv, SocketSend, ZmqMessage};

use crate::error::BridgeError;
use crate::protocol::{
    Account, Command, History, HistoryQuery, OrderCancelArgs, OrderSendArgs, Orders, PingInfo,
    PositionCloseArgs, PositionClosePartialArgs, PositionModifyArgs, Positions, Price, Reply,
    Request, SymbolInfo, TradeResult,
};

/// Calls that may wait for the actor at the same time before `call` starts to back-pressure.
const QUEUE_DEPTH: usize = 64;

/// Connection settings for [`BridgeClient`].
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeConfig {
    /// ZeroMQ endpoint of the EA, e.g. `tcp://127.0.0.1:5555`. Must be loopback
    /// (`127.0.0.1`, `localhost` or `[::1]`); `localhost` is connected as `127.0.0.1`.
    pub endpoint: String,
    /// Per-request timeout. It covers the whole call: time spent waiting behind other
    /// calls, connecting, sending and waiting for the reply.
    pub timeout: Duration,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            endpoint: "tcp://127.0.0.1:5555".to_owned(),
            timeout: Duration::from_millis(3000),
        }
    }
}

/// Async client for the EA. Cheap to clone; all clones share one socket actor.
///
/// When the last clone is dropped the actor stops and the socket is closed.
#[derive(Debug, Clone)]
pub struct BridgeClient {
    tx: mpsc::Sender<Job>,
}

impl BridgeClient {
    /// Create the client and start its actor. Connecting is lazy: this succeeds even
    /// when the EA is not running; calls then fail with `Timeout`/`Transport`.
    /// Rejects non-loopback endpoints with `BridgeError::Transport`.
    ///
    /// The actor is spawned with the current Tokio runtime, so this must be called
    /// from inside one (otherwise it returns `BridgeError::Transport`, it does not panic).
    pub fn new(config: BridgeConfig) -> Result<Self, BridgeError> {
        let target = Target::parse(&config.endpoint)?;
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            BridgeError::Transport("BridgeClient::new must be called inside a Tokio runtime".into())
        })?;
        let (tx, rx) = mpsc::channel(QUEUE_DEPTH);
        let actor = Actor {
            target,
            timeout: config.timeout,
            socket: None,
        };
        runtime.spawn(actor.run(rx));
        Ok(Self { tx })
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
        let job = Job {
            command,
            enqueued: Instant::now(),
            reply: reply_tx,
        };
        self.tx.send(job).await.map_err(|_| actor_stopped())?;
        let reply = reply_rx.await.map_err(|_| actor_stopped())??;
        Ok(reply.decode()?)
    }

    pub async fn ping(&self) -> Result<PingInfo, BridgeError> {
        self.call(Command::Ping {}).await
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

    pub async fn get_symbol(&self, symbol: &str) -> Result<SymbolInfo, BridgeError> {
        self.call(Command::GetSymbol { symbol: symbol.to_owned() }).await
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

    pub async fn position_close(&self, args: PositionCloseArgs) -> Result<TradeResult, BridgeError> {
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
    timeout: Duration,
    /// Connected socket, kept between calls. `None` until the first call and after any failure.
    socket: Option<ReqSocket>,
}

impl Actor {
    async fn run(mut self, mut rx: mpsc::Receiver<Job>) {
        while let Some(job) = rx.recv().await {
            if job.reply.is_closed() {
                // Caller gave up while queued; nothing was sent, so skip it.
                continue;
            }
            let deadline = job.enqueued + self.timeout;
            let result = self.handle(job.command, deadline).await;
            // The caller may have been cancelled meanwhile; the exchange still ran to completion
            // (or reset the socket), so lock-step stays intact.
            let _ = job.reply.send(result);
        }
        tracing::debug!(endpoint = %self.target.endpoint, "bridge client stopped");
    }

    async fn handle(&mut self, command: Command, deadline: Instant) -> Result<Reply, BridgeError> {
        let id = Uuid::new_v4().to_string();
        let cmd = command.name();
        let started = Instant::now();
        let outcome = match serde_json::to_vec(&Request {
            id: id.clone(),
            command,
        }) {
            Ok(payload) => self.exchange(&id, payload, deadline).await,
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
    ) -> Result<Reply, BridgeError> {
        let timeout_ms = u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX);
        if Instant::now() >= deadline {
            // Spent the whole budget waiting behind earlier calls. Nothing was sent.
            return Err(BridgeError::Timeout(timeout_ms));
        }
        let socket = self.socket.take();
        let round_trip = round_trip(&self.target, socket, payload);
        match tokio::time::timeout_at(deadline, round_trip).await {
            Err(_elapsed) => Err(BridgeError::Timeout(timeout_ms)),
            Ok(Err(e)) => Err(e),
            Ok(Ok((reply, socket))) => {
                if reply.id != id {
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

/// Send `payload` and wait for one reply, connecting first when there is no live socket.
/// If a reused socket turns out to be dead at send time, retries once on a fresh socket
/// (nothing has been delivered in that case, so this is safe for write commands too).
async fn round_trip(
    target: &Target,
    existing: Option<ReqSocket>,
    payload: Vec<u8>,
) -> Result<(Reply, ReqSocket), BridgeError> {
    let message = ZmqMessage::from(Bytes::from(payload));
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
    if frames.len() != 1 {
        return Err(BridgeError::Protocol(format!(
            "expected a single-frame reply, got {} frames",
            frames.len()
        )));
    }
    let reply = frames
        .get(0)
        .ok_or_else(|| BridgeError::Protocol("empty reply".into()))
        .and_then(|frame| {
            serde_json::from_slice::<Reply>(frame)
                .map_err(|e| BridgeError::Protocol(format!("invalid reply JSON: {e}")))
        })?;
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
