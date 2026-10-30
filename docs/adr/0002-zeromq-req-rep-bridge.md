# ADR-0002: ZeroMQ REQ/REP bridge with the EA as a loopback REP server

- Status: Accepted
- Date: 2026-10-30

## Context

The Rust backend must talk to the MT5 terminal, which runs the `JarvisBridge` Expert Advisor (EA). MQL5 is single-threaded. An EA that blocks on a socket receive freezes its timers, and `OnTick` does not fire on quiet markets. The link must also be local only, because the bridge can read account data and, from Sprint 2, send trades.

## Decision

- The EA is the ZeroMQ **REP** side and binds `tcp://127.0.0.1:5555`. The Rust backend is the **REQ** side and connects to it. The EA refuses any endpoint that is not loopback.
- The EA polls its socket with `ZMQ_DONTWAIT` from `OnTimer`, every 20 ms, and handles at most 10 requests per tick. It never blocks.
- The Rust client sends every call through one actor task that keeps one request in flight. REQ/REP is lock-step, so the client applies a timeout (default 3 seconds). After a timeout, a transport error or a malformed reply, it drops the socket and creates a new one. Every reply's `id` is checked against its request.
- Messages are one JSON frame each, in the `{"id","cmd","args"}` envelope described in [docs/protocol.md](../protocol.md).

The EA is vendored through the mql-zmq binding (see ADR-0004).

## Consequences

- The bridge cannot be reached from the network, so the network is not an access path. Local processes can still reach it, which is why the EA validates every field.
- Polling keeps the EA responsive, but a request waits up to one timer tick (about 20 ms) before it is read.
- A lost reply does not wedge the link, because the client recreates the socket. A write that was sent but not answered has an unknown outcome (see ADR-0013).
- Only the EA can speak this protocol. Tests use the mock bridge in `crates/bridge-client`, so most of the code can be tested without MT5.

## Alternatives considered

- **EA as REQ, Rust as REP.** An MQL5 REQ call would have to wait for the reply, which blocks the EA. Polling a REP socket from `OnTimer` fits MQL5's single thread.
- **Python `MetaTrader5` package.** It talks to the terminal directly. Using it would move the backend to Python and bypass the EA interface that the safety rules, and the audit trail, are built around.
- **HTTP through MT5's `WebRequest`.** MT5 allows `WebRequest` only from URLs that are allow-listed, and the EA would then be a client only. It cannot serve requests from the backend.
- **Files or named pipes.** Files need polling and locking on both sides and are harder to make reliable. Named pipes would need extra native code on the EA side, and they offered no clear benefit over ZeroMQ.
