# Bridge protocol (version 1)

Wire contract between the Rust backend (`jarvis-bridge-client`) and the MQL5 EA `JarvisBridge`.
Sprint 1 covers **read commands only**. Write commands (`order_send`, `position_modify`,
`position_close`, `position_close_partial`, `order_cancel`) are reserved for Sprint 2 and must
answer `retcode -2` ("unknown command") until then.

Source of truth in code: `crates/bridge-client/src/protocol.rs`. The JSON examples below are the
golden files in `crates/bridge-client/tests/fixtures/`; the Rust tests round-trip every one of them,
and the EA's parser/serialiser should be checked against the same files.

## Transport

- ZeroMQ **REQ/REP** over TCP. The EA is the REP side and **binds** `tcp://127.0.0.1:<port>`
  (default port `5555`). The backend is the REQ side and **connects**.
- **Loopback only.** The EA must bind `127.0.0.1` (never `*` or `0.0.0.0`). The Rust client refuses to
  start with any endpoint other than `tcp://127.0.0.1:<port>`, `tcp://localhost:<port>` or
  `tcp://[::1]:<port>` (`localhost` is connected as `127.0.0.1`). There is no authentication
  on the socket; being unreachable from the network is the access control.
- One request is one ZeroMQ message with **one frame** holding a UTF-8 JSON object. One reply is
  likewise one frame. No multipart messages. No ZeroMQ-specific extensions: plain REQ, plain REP,
  ZMTP 3.x with the NULL mechanism (this is what libzmq 4.x and the pure-Rust `zeromq` crate both speak).
- Maximum message size: no hard limit in the protocol, but `get_history` is capped at 500 deals
  (about 150 KB) to keep replies small.

## Envelope

Request:

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1001", "cmd": "ping", "args": {}}
```

| field | type | rules |
|---|---|---|
| `id` | string | Chosen by the client (UUID v4). Non-empty. The EA echoes it unchanged. |
| `cmd` | string | Lower-case command name. |
| `args` | object | **Always present**, `{}` when the command takes none. Unknown extra keys should be ignored by the EA. |

Reply, success:

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1001", "ok": true, "data": {}, "retcode": 0}
```

Reply, failure:

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1007", "ok": false, "error": "unknown symbol: XAUUS", "retcode": 4301}
```

| field | type | rules |
|---|---|---|
| `id` | string | Same as the request. If the request was so malformed that no `id` could be read, use `""`. |
| `ok` | bool | `true` when `data` is valid. |
| `data` | object | Present when `ok` is true; omitted when false. |
| `error` | string | Present when `ok` is false; a short human-readable message. It may be shown to the user and given to the LLM, so it must never contain credentials, login numbers or account names. |
| `retcode` | integer | See below. Always present. |

The client rejects a reply whose `id` differs from the request's (`BridgeError::Protocol`).

### Numbers, times, nulls

- Prices, volumes and money are JSON numbers (doubles). Integers where a double is expected
  (`10000` instead of `10000.0`) are accepted by the client. Use a decimal point anyway where convenient.
  Never send `NaN`/`Infinity` (not valid JSON); send `0.0` or `null` where a field allows it.
- Tickets, counts, `magic`, `leverage` and times are integers.
- All times are **Unix seconds as MT5 reports them, i.e. server time** (`TimeTradeServer`, deal and
  position times). They are not UTC unless the broker's server runs on UTC. Clients that need "now"
  or "N days ago" should anchor on `ping.server_time` rather than the local clock.
- `sl` and `tp` are `null` when unset (MT5 reports `0`; the EA converts it).
- Enum values are lower-case snake_case strings. An unexpected value is a protocol error.

## Retcodes

| retcode | meaning |
|---|---|
| `0` | success (`ok: true`) |
| `-1` | request invalid: not JSON, not an object, missing/ill-typed `id`, `cmd` or arguments, argument out of range (`limit`, `from > to`), request too large |
| `-2` | unknown command, or a command not implemented in this protocol version |
| `> 0` | MT5 `GetLastError()` value for read commands, for example `4301` (`ERR_MARKET_UNKNOWN_SYMBOL`). For write commands in Sprint 2 it will be the trade server retcode (`10009` done, `10013` invalid request, and so on). |

`ok: false` always comes with a non-empty `error`. The Rust client maps it to
`BridgeError::Remote { message, retcode }`.

**One reply per request.** The EA must send exactly one reply for every request it receives,
including malformed ones (answer `retcode -1`, `id` `""` if unknown) and unknown commands
(`retcode -2`). REP sockets are lock-step: a request that gets no reply wedges the socket until the
peer reconnects. The EA must never block inside a handler, and should answer from its `OnTimer`
poll loop within a few milliseconds. It must never send an unsolicited message.

## Commands (Sprint 1)

### `ping`

Args: none. Health check and environment probe. The backend calls it at startup and for the
status pill; it refuses to run in DEMO mode when `trade_mode` is `real`.

Request / reply (`ping.request.json`, `ping.reply.json`):

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1001", "cmd": "ping", "args": {}}
```

```json
{
  "id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1001",
  "ok": true,
  "data": {
    "protocol_version": 1,
    "ea_version": "0.1.0",
    "trade_mode": "demo",
    "terminal_connected": true,
    "trade_allowed": true,
    "server_time": 1793341200
  },
  "retcode": 0
}
```

| data field | type | notes |
|---|---|---|
| `protocol_version` | integer | `1`. Bumped on breaking changes; the backend should refuse a mismatch. |
| `ea_version` | string | EA build, free text. |
| `trade_mode` | `"demo"` \| `"contest"` \| `"real"` | `ACCOUNT_TRADE_MODE`. |
| `terminal_connected` | bool | `TERMINAL_CONNECTED`. |
| `trade_allowed` | bool | `TERMINAL_TRADE_ALLOWED` and `MQL_TRADE_ALLOWED`. |
| `server_time` | integer | `TimeTradeServer()`. |

### `get_price`

Args: `symbol` (string, required, exact broker symbol such as `XAUUSD` or `XAUUSD.m`).
The EA validates the symbol (`SymbolSelect` / `SymbolInfoInteger`) and answers `4301` if unknown.
Symbol aliases (`gold` to `XAUUSD`) are resolved by the backend, never by the EA.

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1002", "cmd": "get_price", "args": {"symbol": "XAUUSD"}}
```

```json
{
  "id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1002",
  "ok": true,
  "data": {
    "symbol": "XAUUSD",
    "bid": 2391.12,
    "ask": 2391.4,
    "last": 0.0,
    "spread_points": 28,
    "digits": 2,
    "time": 1793341200
  },
  "retcode": 0
}
```

`last` is `0.0` for instruments without last-trade prices (most CFDs and forex). `time` is the last tick time.

### `get_account`

Args: none. **Never includes login, name, server or company** (SPEC rule 8).

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1003", "cmd": "get_account", "args": {}}
```

```json
{
  "id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1003",
  "ok": true,
  "data": {
    "currency": "USD",
    "balance": 10000.0,
    "equity": 10070.55,
    "profit": 70.55,
    "margin": 600.0,
    "free_margin": 9470.55,
    "margin_level": 1678.43,
    "leverage": 100,
    "trade_mode": "demo"
  },
  "retcode": 0
}
```

`margin_level` is a percentage and `0.0` when no margin is used.

### `get_positions`

Args: `symbol` (string, optional). Without it, all open positions; with it, only that symbol
(unknown symbol: `4301`). Order: by open time ascending.

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1004", "cmd": "get_positions", "args": {"symbol": "XAUUSD"}}
```

```json
{
  "id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1004",
  "ok": true,
  "data": {
    "positions": [
      {
        "ticket": 1001001,
        "symbol": "XAUUSD",
        "side": "buy",
        "volume": 0.1,
        "price_open": 2385.4,
        "price_current": 2391.12,
        "sl": null,
        "tp": 2410.0,
        "profit": 57.2,
        "swap": -0.35,
        "magic": 0,
        "comment": "",
        "time": 1793330400
      }
    ]
  },
  "retcode": 0
}
```

(The fixture `get_positions.reply.json` also contains a second, US100 sell position. The example
above was shortened.) `side` is `"buy"` or `"sell"`. `time` is the open time.

### `get_orders`

Args: `symbol` (string, optional), same rules as `get_positions`. Pending orders only.

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1005", "cmd": "get_orders", "args": {}}
```

```json
{
  "id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1005",
  "ok": true,
  "data": {
    "orders": [
      {
        "ticket": 2001001,
        "symbol": "EURUSD",
        "type": "buy_limit",
        "volume_initial": 0.2,
        "volume_current": 0.2,
        "price_open": 1.08,
        "sl": 1.075,
        "tp": 1.09,
        "time_setup": 1793254800,
        "comment": ""
      }
    ]
  },
  "retcode": 0
}
```

`type` is one of `buy_limit`, `sell_limit`, `buy_stop`, `sell_stop`, `buy_stop_limit`, `sell_stop_limit`.

### `get_history`

Args:

| arg | type | rules |
|---|---|---|
| `from` | integer | required, server-time Unix seconds, inclusive |
| `to` | integer | required, inclusive, must be `>= from` (else `-1`) |
| `symbol` | string | optional filter; unknown symbol is `4301`. Balance/credit deals have an empty symbol and only appear without a filter. |
| `limit` | integer | optional, `1..=500`, default `100`; anything else is `-1` (the EA must not clamp silently) |

The reply lists deals (`HistorySelect` + `HistoryDealGet*`) **newest first** (time descending). When more
deals match than `limit`, the **`limit` most recent are returned** and `truncated` is `true`; callers narrow
the range to see older deals.

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1006", "cmd": "get_history", "args": {"from": 1793254800, "to": 1793341200, "limit": 50}}
```

```json
{
  "id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1006",
  "ok": true,
  "data": {
    "deals": [
      {
        "ticket": 3000001, "order": 0, "position_id": 0, "symbol": "",
        "type": "balance", "entry": "in", "volume": 0.0, "price": 0.0,
        "profit": 10000.0, "commission": 0.0, "swap": 0.0,
        "time": 1793256000, "comment": "Deposit"
      },
      {
        "ticket": 3000003, "order": 3001003, "position_id": 900001, "symbol": "EURUSD",
        "type": "sell", "entry": "out", "volume": 0.1, "price": 1.0846,
        "profit": 25.0, "commission": -0.35, "swap": 0.0,
        "time": 1793300000, "comment": ""
      }
    ],
    "truncated": false
  },
  "retcode": 0
}
```

(Shortened; the fixture has three deals.) Deal `type`: `buy`, `sell`, `balance`, `credit`, `other`
(anything else the EA does not model, such as commission or bonus deals). Deal `entry`: `in`, `out`,
`inout`, `out_by`.

## Error example

Unknown symbol (`error.reply.json`):

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1007", "ok": false, "error": "unknown symbol: XAUUS", "retcode": 4301}
```

Malformed request (not JSON), where the EA cannot know the id:

```json
{"id": "", "ok": false, "error": "malformed request: not valid JSON", "retcode": -1}
```

Unknown command:

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1008", "ok": false, "error": "unknown command: order_send", "retcode": -2}
```

## Client behaviour (`BridgeClient`)

- **One actor, one request in flight.** All clones of `BridgeClient` send commands to a single task
  that owns the REQ socket. Calls are queued and executed strictly one after another.
- **Lazy connect.** `BridgeClient::new` never touches the network and succeeds when the EA is not
  running. The socket is connected on the first call that needs it, and again after any reset.
  `new` must be called inside a Tokio runtime (it spawns the actor).
- **Timeout** (default 3000 ms, `BridgeConfig::timeout`). The budget covers the whole call: waiting
  behind earlier calls, connecting, sending, and waiting for the reply. On expiry the call returns
  `BridgeError::Timeout(ms)`. A call that spent its entire budget in the queue fails without
  being sent, so a stalled EA does not make queued callers wait one timeout each.
- **Socket reset.** After any timeout, transport error, malformed reply or `id` mismatch, the socket is
  dropped (closing the TCP connection) and a fresh one is created on the next call. This is required:
  with plain REQ/REP a lost reply would otherwise block the socket forever. The EA's REP socket
  sees a new peer and carries on; it must tolerate peers disconnecting with a request unanswered.
- **EA not running.** The client first probes the TCP port; connection refused returns
  `BridgeError::Transport` immediately (on Windows a refused loopback connect itself takes about
  2 s). Once the EA starts, the next call succeeds without restarting anything.
- **Dead reused socket.** If sending on a reused socket fails (the EA restarted), the client
  reconnects once and re-sends within the same call. Nothing was delivered in that case, so
  this is safe even for the write commands of Sprint 2. A failure after the request was sent
  is never retried.
- **Logging** (`tracing`): per call `id`, `cmd`, `duration_ms`, `ok`, `retcode` at `debug`;
  failures and `ok: false` replies at `warn`.

| `BridgeError` | when |
|---|---|
| `Timeout(ms)` | no reply within the budget; socket reset |
| `Transport(msg)` | bad endpoint, EA unreachable, socket send/receive failed |
| `Protocol(msg)` | reply not valid JSON or not the expected shape, `id` mismatch, multi-frame reply |
| `Remote { message, retcode }` | EA answered `ok: false` |

## Requirements for the EA

1. `bind` `tcp://127.0.0.1:<port>` only. Log the endpoint on start.
2. Poll with `ZMQ_DONTWAIT` from `OnTimer` (about 20 ms); handle at most 10 requests per tick.
3. Exactly one single-frame JSON reply per request, always, including errors. Echo `id`.
4. Validate every field (types, ranges, symbol) before using it. Never crash or hang on bad input.
5. Never include login, name, server or company in any reply.
6. Requests larger than 64 KB are dropped by ZeroMQ (`ZMQ_MAXMSGSIZE`), so the client sees a timeout, not a reply.
7. In Sprint 1 answer every command other than the six above with `retcode -2`.

## Mock for tests and offline work

`jarvis_bridge_client::mock::MockBridge` (cargo feature `mock`) is an in-process REP server that follows
this document. `cargo run -p jarvis-bridge-client --features mock --example mock_server`
serves it on `tcp://127.0.0.1:5555` so `bridge_cli` and the API can be exercised without MT5.
