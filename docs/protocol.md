# Bridge protocol (version 2)

Wire contract between the Rust backend (`jarvis-bridge-client`) and the MQL5 EA `JarvisBridge`.
Read commands (`ping`, `get_price`, `get_account`, `get_positions`, `get_orders`, `get_history`,
`get_symbol`) and write commands (`order_send`, `position_modify`, `position_close`,
`position_close_partial`, `order_cancel`) are specified below. EA 0.3.0 implements both.

**Version 2 (EA 0.3.0) is not compatible with version 1.** Every message is now authenticated
(finding BR-1, [ADR-0016](adr/0016-authenticated-bridge-protocol.md)): it travels as two ZeroMQ
frames, a hex HMAC-SHA256 and the JSON, requests carry a timestamp, and only a holder of the shared
key can talk to the EA. The commands and their arguments did not change. The client refuses an EA
that reports another `protocol_version`, and an old EA answers a version 2 request with a single
plain frame, which the client reports as such.

Source of truth in code: `crates/bridge-client/src/protocol.rs` (JSON) and
`crates/bridge-client/src/auth.rs` (frames, MAC, replay rules). The JSON examples below are the
golden files in `crates/bridge-client/tests/fixtures/`; the Rust tests round-trip every one of them,
and the EA's parser/serialiser should be checked against the same files. The envelope files
(`*.request.json`, `*.reply.json`) show the JSON payload only (frame 2); `ts` is left out of them
so that they stay readable. `auth.golden.json` is the authentication vector (key, payload, MAC)
that the Rust tests and the MQL5 self-test both check.

## Transport

- ZeroMQ **REQ/REP** over TCP. The EA is the REP side and **binds** `tcp://127.0.0.1:<port>`
  (default port `5555`). The backend is the REQ side and **connects**.
- **Loopback only.** The EA must bind `127.0.0.1` (never `*` or `0.0.0.0`). The Rust client refuses to
  start with any endpoint other than `tcp://127.0.0.1:<port>`, `tcp://localhost:<port>` or
  `tcp://[::1]:<port>` (`localhost` is connected as `127.0.0.1`). Being unreachable from the
  network keeps remote hosts out; a program on the same PC is kept out by the message
  authentication below.
- One request is one ZeroMQ **multipart message of exactly two frames**: frame 1 is the MAC,
  frame 2 is a UTF-8 JSON object. One reply is likewise two frames. See
  [Authentication](#authentication). No ZeroMQ-specific extensions: plain REQ, plain REP,
  ZMTP 3.x with the NULL mechanism (this is what libzmq 4.x and the pure-Rust `zeromq` crate both speak).
- Maximum message size: no hard limit in the protocol, but `get_history` is capped at 500 deals
  (about 150 KB) to keep replies small.

## Authentication

Finding BR-1: without this, any program on the PC (also another Windows user on a shared PC or VPS)
could connect to `127.0.0.1:5555` and send `order_send` straight to the EA, skipping the gate, the
limits, the confirmation and the audit log. Now only a holder of a shared secret can talk to the EA,
and only the EA (or a holder of the secret) can answer the API.

**The key.** 32 random bytes, written as 64 hexadecimal characters in a text file, by
`scripts/install-ea.ps1` (CSPRNG, never overwritten, never printed). The EA reads it from
`MQL5\Files\JarvisMT\bridge.key` (its file sandbox, inside the user's profile) when it starts and
does not start without it. The API reads the same file (`[bridge] key_file`) or `JARVIS_BRIDGE_KEY`
and does not start without it. See [configuration.md](configuration.md) and the
[runbook](runbook.md#the-bridge-key).

**Frames.** Every message, in both directions:

| frame | content |
|---|---|
| 1 | lowercase hex of `HMAC-SHA256(key, frame 2)`, exactly 64 ASCII characters |
| 2 | the JSON payload, as the exact bytes that were sent |

The MAC covers the raw bytes of frame 2, so there is no JSON canonicalisation to get wrong. The key
is the 32 bytes the hex stands for. Receivers compare the MAC in constant time and accept only
lowercase hex. Golden vector (`auth.golden.json`): key `000102...1f`, payload
`{"id":"6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1001","ts":1793341200000,"cmd":"ping","args":{}}`, MAC
`bc45b3fc8bc3cef210abe4efff32897c8ca52cf7a0da1d0366707923cd1ca581`. Both implementations are also
checked against the RFC 4231 HMAC-SHA256 test vectors.

**What the EA checks, in this order, before it parses or executes anything:**

1. The message has exactly two frames (more frames are read and dropped; the EA still answers once).
2. Frame 1 is the correct MAC of frame 2.
3. Frame 2 is a JSON object with a non-empty string `id` and an integer `ts` (Unix milliseconds on
   the client's clock). A payload with a zero byte is rejected as malformed.
4. `ts` is within 30 000 ms of the EA's clock (either direction).
5. `id` was not accepted in the last 60 s. Accepted ids are remembered in a table of at most 4096
   entries, expired by time (a monotonic clock, so a clock change cannot forget one early). A full
   table refuses new requests until entries expire (fail closed).

Then the usual checks follow (command, arguments, write cache, safety gate). A request that fails
step 1, 2, 3, 4 or 5 is **not executed and not cached**. The EA's clock is `TimeGMT()` (one-second
resolution) plus milliseconds from `GetTickCount64()` since it last saw the second change: about
25 ms accurate while the 20 ms timer runs, at worst 1 s. The 30 s window is far wider than that.
The API's clock is the system clock. Both normally belong to the same PC.

**The reply to a refused request.** REP is lock-step, so the EA answers it exactly once, with a
signed reply like any other, retcode `-4` and always the same text:

```json
{"id": "", "ok": false, "error": "authentication failed", "retcode": -4, "ts": 1793341200012}
```

`id` is `""` when the request was not authentic (wrong or missing MAC, wrong frame count): the EA
never echoes unauthenticated input, so a peer without the key cannot get text of its choosing
signed. When the MAC was valid (the sender holds the key) but `ts` or `id` was refused, the `id` is
echoed. The error text never says which check failed; the EA logs the reason (throttled to one line
per 10 s) in the Experts journal. A peer without the key learns nothing from the reply, which is
signed with a key it does not have.

**Replies.** The reply JSON is unchanged and has an added `"ts"` (the EA's clock, informational).
The client verifies the MAC of every reply in constant time, and that the reply `id` equals the
request `id` (the one exception: a signed `-4` with `id` `""` is reported as an authentication
refusal). A reply that fails is a `BridgeError::Protocol("reply authentication failed")`: either the
keys differ or another program answers on the port. The client then drops the socket. A reply that
is not exactly two frames is also a protocol error, with a hint that the EA may be older than
version 2.

**What this does not do.** It does not encrypt: anyone who can read loopback traffic (an
administrator) sees the messages. It does not stop a program that can read `bridge.key` (the same
Windows account, or an administrator), and it does not rate-limit a peer that floods the socket
with garbage (each message still gets one cheap reply). The threat model has the details.

## Envelope

Request (frame 2 of the message; the examples further down omit `ts` for readability):

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1001", "ts": 1793341200000, "cmd": "ping", "args": {}}
```

| field | type | rules |
|---|---|---|
| `id` | string | Chosen by the client (UUID v4). Non-empty, at most 128 characters. The EA echoes it unchanged. A new id for every request: an id used in the last 60 s is refused. |
| `ts` | integer | Unix **milliseconds** on the client's clock. Required. Refused when more than 30 s from the EA's clock. |
| `cmd` | string | Lower-case command name. |
| `args` | object | **Always present**, `{}` when the command takes none. Unknown extra keys should be ignored by the EA. |

Reply, success:

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1001", "ok": true, "data": {}, "retcode": 0, "ts": 1793341200012}
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
| `-3` | write refused by the EA safety gate (real account without `InpAllowReal`, Algo Trading off); nothing was sent to the trade server |
| `-4` | **authentication failed**: not a two-frame message, wrong or missing MAC, `ts` outside 30 s, `ts` missing, `id` already used in the last 60 s, or too many requests in the last 60 s. The text is always `authentication failed`. Nothing beyond the envelope was parsed, and nothing was cached or executed. See [Authentication](#authentication) |
| `> 0` | For read commands the MT5 `GetLastError()` value, for example `4301` (`ERR_MARKET_UNKNOWN_SYMBOL`). For write commands the trade server retcode: `10009` done, `10008` placed, `10010` done partially (success), `10018` market closed, `10019` no money, `10013` invalid request, and so on. |

`ok: false` always comes with a non-empty `error`. The Rust client maps it to
`BridgeError::Remote { message, retcode }`.

**One reply per request.** The EA must send exactly one reply for every request it receives,
including malformed ones (answer `retcode -1`, `id` `""` if unknown) and unknown commands
(`retcode -2`). REP sockets are lock-step: a request that gets no reply wedges the socket until the
peer reconnects. The EA must never block inside a handler, and should answer from its `OnTimer`
poll loop within a few milliseconds. It must never send an unsolicited message.

## Commands

Read commands first, then `get_symbol`, then the write commands.

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
    "protocol_version": 2,
    "ea_version": "0.3.0",
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
| `protocol_version` | integer | `2`. Bumped on breaking changes. `BridgeClient::ping` returns a `Protocol` error that names both versions when it differs. |
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


### `get_symbol`

Args: `symbol` (string, required, exact broker symbol). Trading specification, read from MT5
(`SymbolInfoInteger/Double/String`). The backend uses it to round volumes and prices and to size
risk. The symbol is added to Market Watch if needed. Unknown symbol: `4301`.

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1009", "cmd": "get_symbol", "args": {"symbol": "XAUUSD"}}
```

```json
{
  "id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1009",
  "ok": true,
  "data": {
    "symbol": "XAUUSD",
    "description": "Gold vs US Dollar",
    "digits": 2,
    "point": 0.01,
    "volume_min": 0.01,
    "volume_max": 100.0,
    "volume_step": 0.01,
    "contract_size": 100.0,
    "tick_value": 1.0,
    "tick_size": 0.01,
    "stops_level": 10,
    "trade_allowed": true,
    "currency_profit": "USD"
  },
  "retcode": 0
}
```

| data field | MT5 property | notes |
|---|---|---|
| `digits`, `point` | `SYMBOL_DIGITS`, `SYMBOL_POINT` | |
| `volume_min/max/step` | `SYMBOL_VOLUME_MIN/MAX/STEP` | lots |
| `contract_size` | `SYMBOL_TRADE_CONTRACT_SIZE` | |
| `tick_value`, `tick_size` | `SYMBOL_TRADE_TICK_VALUE`, `SYMBOL_TRADE_TICK_SIZE` | account-currency value of one `tick_size` move for 1.0 lot |
| `stops_level` | `SYMBOL_TRADE_STOPS_LEVEL` | minimum SL/TP/pending distance, in points |
| `trade_allowed` | `SYMBOL_TRADE_MODE` | `true` for full, long-only and short-only symbols; `false` for close-only and disabled ones. `order_send` enforces the direction of long-only and short-only symbols. |
| `currency_profit` | `SYMBOL_CURRENCY_PROFIT` | |

## Write commands

Write commands change the account. Only a user-confirmed, gate-approved request may produce them
(SPEC core rules 1 to 4). The EA never trades on its own: it executes a write only as the answer to
a request, through `CTrade`.

### Order of checks

The EA handles every write in the same order. The first failing step answers the request and
nothing reaches the trade server:

1. **Idempotency**: a request `id` that is already in the reply cache gets the cached reply
   (see below). Nothing else runs.
2. **Shape**: argument types, required arguments, `comment` length, price rules per order type,
   negative sl/tp, deviation range. Failure: `retcode -1`.
3. **Safety gate**: failure: `retcode -3`.
4. **Market state and ticket checks**: symbol, volume, price format, stops, ticket exists.
   Failure: `retcode -1`.
5. **Execution** through `CTrade`. The reply carries the trade server retcode.

Because step 2 comes before step 3, a malformed request reports `-1` even on an account the EA
would refuse; a well-formed one reports `-3` before any market-dependent validation.

### Safety gate (`retcode -3`)

The EA refuses with `retcode -3` and `ok:false` (nothing is sent to the trade server) when:

| condition | error text |
|---|---|
| `ACCOUNT_TRADE_MODE` is real and the EA input `InpAllowReal` is `false` (the default) | `writes refused on a real account (InpAllowReal is false)` |
| `TERMINAL_TRADE_ALLOWED` is false | `writes refused: Algo Trading is disabled in the terminal (enable Algo Trading)` |
| `MQL_TRADE_ALLOWED` is false (Algo Trading is off for this EA) | `writes refused: trading is not allowed for this EA (enable Algo Trading for the EA)` |
| the account does not allow trading or automated trading (`ACCOUNT_TRADE_ALLOWED`, `ACCOUNT_TRADE_EXPERT`) | `writes refused: trading is disabled for this account` or `writes refused: automated trading is disabled for this account` |
| no connected trading account, or the strategy tester | `writes refused: ...` |

Reads are never refused.

### Validation (`retcode -1`)

An invalid request answers `ok:false`, `retcode -1` and a clear `error`; nothing is sent. The
EA **rejects, never rounds**: the caller rounds volumes and prices first (using `get_symbol`).

| rule | detail |
|---|---|
| symbol | must exist and have quotes (`unknown symbol: X`); `trade_allowed` must be true; a long-only symbol rejects sells and a short-only symbol rejects buys |
| volume | finite, within `volume_min..=volume_max`, and a whole multiple of `volume_step` (tolerance `1e-8`) |
| prices | `price`, `sl`, `tp` finite, positive and expressed in at most `digits` decimals |
| stops | buy: `sl` below and `tp` above the reference price; sell: the reverse; each at least `max(stops_level, 1)` points away. Reference price: market order: the close-side price (bid for a buy, ask for a sell); pending order: its own `price` |
| pending price | buy limit below the ask, buy stop above the ask, sell limit above the bid, sell stop below the bid, each at least `max(stops_level, 1)` points away |
| `price` | required for `limit` and `stop`; must be absent for `market` |
| `comment` | at most 31 characters |
| `deviation_points` | `0..=10000`, default `20` |
| `magic` | required, not negative |
| tickets | positive integers; the position or pending order must exist |

### Reply

A successful write answers `ok:true` with `data` = **TradeResult**, and the envelope `retcode`
equals `data.retcode`. `ok` is true exactly for the success retcodes `10008` (placed), `10009`
(done) and `10010` (done partially).

| data field | type | notes |
|---|---|---|
| `retcode` | integer | trade server retcode |
| `retcode_description` | string | text from `CTrade`, e.g. `done`, `done at 2391.40` |
| `order` | integer | order ticket created or affected; `0` for `position_modify` |
| `deal` | integer | deal ticket; `0` when there is none (pending order placed or cancelled, modify) |
| `volume` | number | executed volume; for a pending order the requested volume; `0` for modify and cancel |
| `price` | number | execution price; `0` when there is none |
| `comment` | string | trade server comment, usually `Request executed` |

If the trade server rejects the request, the reply is `ok:false`, `retcode` = the trade retcode
and `error` = `trade failed: <description> (<retcode>)`. **Error replies omit `data`**; the
description is in `error`. For example market closed is `10018`, not enough margin is `10019`.

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1010", "ok": false, "error": "trade failed: no money (10019)", "retcode": 10019}
```

### `order_send`

Opens a position (`market`) or places a pending order (`limit`, `stop`).

| arg | type | rules |
|---|---|---|
| `symbol` | string | required |
| `side` | `"buy"` or `"sell"` | required |
| `volume` | number | required |
| `type` | `"market"`, `"limit"` or `"stop"` | required |
| `price` | number | required for `limit`/`stop`, absent for `market` |
| `sl`, `tp` | number | optional; `0` or absent means not set |
| `deviation_points` | integer | optional, max slippage for market orders |
| `magic` | integer | required; the backend sends `770077` (`JARVIS_MAGIC`) |
| `comment` | string | at most 31 characters, may be empty |

The filling mode is chosen from `SYMBOL_FILLING_MODE` (FOK, then IOC, then RETURN).

Market order (`order_send.request.json`, `order_send.reply.json`):

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1010", "cmd": "order_send", "args": {"symbol": "XAUUSD", "side": "buy", "volume": 0.1, "type": "market", "sl": 2380.0, "tp": 2410.0, "deviation_points": 20, "magic": 770077, "comment": "jarvis:3f9a2c1d"}}
```

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1010", "ok": true, "data": {"retcode": 10009, "retcode_description": "done at 2391.40", "order": 4000001, "deal": 6000001, "volume": 0.1, "price": 2391.4, "comment": "Request executed"}, "retcode": 10009}
```

Pending order (`order_send_limit.request.json`, `order_send_limit.reply.json`):

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1011", "cmd": "order_send", "args": {"symbol": "EURUSD", "side": "buy", "volume": 0.2, "type": "limit", "price": 1.07, "sl": 1.06, "tp": 1.09, "magic": 770077, "comment": "jarvis:7b2e90aa"}}
```

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1011", "ok": true, "data": {"retcode": 10009, "retcode_description": "done", "order": 4000001, "deal": 0, "volume": 0.2, "price": 0.0, "comment": "Request executed"}, "retcode": 10009}
```

Rejected volume (`order_send_invalid_volume.*`):

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1016", "ok": false, "error": "volume 0.015 is not a multiple of the volume step 0.01", "retcode": -1}
```

Refused (`order_send_refused.reply.json`, the answer to `order_send.request.json` on a real account):

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1010", "ok": false, "error": "writes refused on a real account (InpAllowReal is false)", "retcode": -3}
```

### `position_modify`

Changes the stops of an open position. **Absent keeps the current value, `0` removes it.** At least
one of `sl` and `tp` is required. A value equal to the current one is not re-validated against the
moving market. Args: `ticket` (required), `sl`, `tp`. The same stops rules as `order_send` apply,
with the close-side price as reference.

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1012", "cmd": "position_modify", "args": {"ticket": 1001001, "sl": 2380.0}}
```

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1012", "ok": true, "data": {"retcode": 10009, "retcode_description": "done", "order": 0, "deal": 0, "volume": 0.0, "price": 0.0, "comment": "Request executed"}, "retcode": 10009}
```

### `position_close`

Closes the whole position at market. Args: `ticket` (required), `deviation_points` (optional).
The reply carries the closing order and deal and the closing price.

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1013", "cmd": "position_close", "args": {"ticket": 1001001, "deviation_points": 30}}
```

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1013", "ok": true, "data": {"retcode": 10009, "retcode_description": "done at 2391.12", "order": 4000001, "deal": 6000001, "volume": 0.1, "price": 2391.12, "comment": "Request executed"}, "retcode": 10009}
```

### `position_close_partial`

Closes part of a position. Args: `ticket`, `volume`, `deviation_points` (optional). `volume` must be
valid for the symbol (min, max, step), **strictly less than the position volume** (use
`position_close` to close it fully), and the remaining volume must be at least `volume_min`. On
hedging accounts the position keeps its ticket with a smaller volume; netting accounts reduce the
net position with an opposite deal.

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1014", "cmd": "position_close_partial", "args": {"ticket": 1001002, "volume": 0.2}}
```

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1014", "ok": true, "data": {"retcode": 10009, "retcode_description": "done at 19823.3", "order": 4000001, "deal": 6000001, "volume": 0.2, "price": 19823.3, "comment": "Request executed"}, "retcode": 10009}
```

Some brokers blank the comment of the remaining position after a partial close (observed on the
demo server used for the live tests). Identify a position by its ticket and `magic`, not by its
comment.

### `order_cancel`

Deletes a pending order. Args: `ticket` (required). Unknown ticket: `-1`
(`pending order not found: <ticket>`).

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1015", "cmd": "order_cancel", "args": {"ticket": 2001001}}
```

```json
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1015", "ok": true, "data": {"retcode": 10009, "retcode_description": "done", "order": 2001001, "deal": 0, "volume": 0.0, "price": 0.0, "comment": "Request executed"}, "retcode": 10009}
```

### Idempotency

The EA keeps a ring cache of the **last 64 write replies**, keyed by request `id`. A write whose
`id` is already in the cache returns the cached reply **verbatim** and is **not executed again**
(the EA logs this). This holds for every write reply, including validation errors (`-1`), refusals
(`-3`) and trade failures. Reads are never cached. The cache lives in the EA process: it is empty
after the EA restarts or is reloaded. Entries are evicted oldest first, so only the 64 most recent
writes are protected.

Since protocol 2 the authentication step runs first: a request whose `id` was accepted in the last
60 s is refused with `-4` before the cache is consulted, and after 60 s its `ts` is stale anyway.
So the cache cannot be reached through the socket any more. It stays as defence in depth (the
EA code path is unchanged and still tested).

The Rust client generates a fresh UUID for every call and never resends a write.

### A timeout after sending means the outcome is unknown

A write is a request the EA may already have executed even though no reply arrived (slow trade
server, lost reply, client timeout). Therefore:

- The client returns `BridgeError::Timeout` and **never retries a write after it was sent**. The
  only automatic re-send is for a reused socket that failed at send time, when nothing was
  delivered.
- The caller must treat the outcome as **unknown** (the API reports `outcome_unknown`) and
  reconcile by reading `get_positions`, `get_orders` and `get_history` before deciding anything.
- A trade round trip can take seconds, so give the client that sends writes a generous timeout
  (the live tests and `bridge_cli` use 15 s; the default 3 s is meant for reads).
- Sending the same request again with the **same `id`** is refused with `-4` (replay protection):
  safe, but it does not tell you what happened. Sending a new request with a new `id` is a new
  trade. Reconcile by reading positions, orders and history.

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
{"id": "6f1c2c1e-3b5a-4d0e-9a53-0d2a6f8d1008", "ok": false, "error": "unknown command: launch_missiles", "retcode": -2}
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
  this is safe even for write commands. A failure after the request was sent (including a
  timeout) is never retried; for a write the outcome is then unknown, see above.
- **Logging** (`tracing`): per call `id`, `cmd`, `duration_ms`, `ok`, `retcode` at `debug`;
  failures and `ok: false` replies at `warn`.

| `BridgeError` | when |
|---|---|
| `Timeout(ms)` | no reply within the budget; socket reset |
| `Transport(msg)` | bad endpoint, EA unreachable, socket send/receive failed |
| `Protocol(msg)` | reply not valid JSON or not the expected shape, `id` mismatch, reply not a signed two-frame message, reply MAC wrong (`reply authentication failed`), `ping` reports another protocol version |
| `Remote { message, retcode }` | EA answered `ok: false`. Retcode `-4` means the EA refused the request (key, clock or id), and the message says so |

## Requirements for the EA

1. `bind` `tcp://127.0.0.1:<port>` only. Log the endpoint on start (including whether real accounts are allowed).
2. Poll with `ZMQ_DONTWAIT` from `OnTimer` (about 20 ms); handle at most 10 requests per tick.
3. Exactly one signed two-frame reply per request, always, including errors and authentication
   failures. Echo `id` (empty for an unauthenticated request).
4. Authenticate first (frames, MAC, `ts`, replay; see [Authentication](#authentication)) and do
   nothing else for a request that fails. Validate every field (types, ranges, symbol) before using
   it. Never crash or hang on bad input.
5. Never include login, name, server or company in any reply.
6. Frames larger than 64 KB are dropped by ZeroMQ (`ZMQ_MAXMSGSIZE`), so the client sees a timeout, not a reply.
7. Answer every unknown command with `retcode -2`.
8. Write commands: validate before touching `CTrade`, apply the safety gate (`-3`), execute only on
   request, cache the last 64 replies by `id`, log every write (command, id, retcode) and every
   cache hit, and never trade from `OnTick` or on a timer.

## Mock for tests and offline work

`jarvis_bridge_client::mock::MockBridge` (cargo feature `mock`) is an in-process REP server that follows
this document, **including the authentication**: two frames, MAC, `ts` window, replay table, signed
replies, `-4` refusals. Every mock has its own random key (`mock.key()`, `mock.config(timeout)`), or
one you give it (`start_with_key`, `start_at_with_key`), so every test goes through the real
authenticated code path. `cargo run -p jarvis-bridge-client --features mock --example mock_server`
serves it on `tcp://127.0.0.1:5555` so `bridge_cli` and the API can be exercised without MT5; it
writes its key to `target/mock-bridge.key` (or uses `JARVIS_BRIDGE_KEY`) and prints the path.

The mock implements `get_symbol` and every write command with real state changes: `order_send`
adds a position (filled at the ask or bid, new ticket) or a pending order, `position_modify` updates
sl/tp, closes remove or reduce positions and realise profit into the account and the deal history,
`order_cancel` removes an order. It applies the same validation, safety gate, error texts and
64-entry reply cache as the EA. Test hooks on `MockState` and `MockBridge`:

| hook | effect |
|---|---|
| `refuse_writes(true)`, `MockState::refuse_writes` | every write answers `-3` like a real account without `InpAllowReal` |
| `MockState::allow_real` | mirrors `InpAllowReal`; a `Real` `ping.trade_mode` refuses writes unless set |
| `fail_next_write(10019)`, `MockState::fail_next_write` | the next write that passes validation fails with that trade retcode and changes nothing |
| `drop_next(n)` | swallow the next `n` requests without replying (timeout tests) |
| `state()`, `update_state(f)` | read back or change positions, orders, deals, prices, symbols |
| `executed_writes()`, `cache_hits()` | how many writes ran, and how many were answered from the cache |
| `auth_failures()` | requests refused with `-4` |
| `allow_replays(true)` | stops refusing a repeated id (the `ts` window still applies); only to exercise the write cache, which the replay guard otherwise hides |

`MockState::symbols` holds realistic trading specifications for the six mock symbols.
`bridge_cli` offers `symbol`, `buy`, `sell`, `modify`, `close` and `cancel`; each write needs `--yes`
and refuses unless `ping` reports a demo account.
