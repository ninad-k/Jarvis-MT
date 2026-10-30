# HTTP API

Served by `jarvis-api` on `server.bind` (default `http://127.0.0.1:8787`, or `https://` when
`tls_cert`/`tls_key` are set). All bodies are JSON unless stated. The first part of this file is
the read-only chat API (Sprint 1); "Sprint 2–5 additions" below covers trading, login, voice,
events, static files and TLS. Every `/api/*` route except `GET /api/health` and
`POST /api/auth/login` needs a login session when `[auth] enabled = true` (the default).

Errors use one shape everywhere:

```json
{ "error": { "code": "bridge_unavailable", "message": "bridge did not reply within 3000 ms" } }
```

| code | HTTP | when |
|---|---|---|
| `bad_request` | 400 | invalid body, empty message, unknown or unavailable provider, `session_id` not a UUID |
| `mode_mismatch` | 409 | config `mode = "demo"` but the account reports `trade_mode = real` (chat refused) |
| `bridge_unavailable` | 503 | bridge timeout or transport error (also `/api/watchlist` when every symbol failed) |
| `bridge_error` | 502 | EA answered `ok:false` (message includes retcode) |
| `llm_error` | 502 | LLM provider failed |
| `internal` | 500 | anything else (details only in server logs) |

Added later (see below): `unauthorized` 401, `forbidden` 403, `not_found` 404 (also any
unknown `/api/...` path), `method_not_allowed` 405, `request_timeout` 408, `payload_too_large` 413.

## GET /api/status

Health of every part. This endpoint never fails because the bridge is down; it reports it instead.

```json
{
  "version": "0.1.0",
  "mode": "demo",
  "mode_mismatch": false,
  "bridge": {
    "connected": true,
    "latency_ms": 4,
    "ea_version": "0.3.1",
    "protocol_version": 2,
    "trade_mode": "demo",
    "terminal_connected": true,
    "error": null
  },
  "llm": {
    "default": "local",
    "providers": [
      { "name": "local",  "kind": "ollama",    "model": "qwen3.5:9b",       "available": true },
      { "name": "claude", "kind": "anthropic", "model": "claude-sonnet-5-5", "available": false }
    ]
  }
}
```

When the bridge is down: `"connected": false`, with `"error": "..."` set and the other bridge fields `null`.
`available` is false when a provider's API key env var is missing. Sprint 1 doesn't probe the network.

## GET /api/account

Account summary for the HUD (same redacted data the LLM sees):

```json
{ "currency": "USD", "balance": 10000.0, "equity": 10012.5, "profit": 12.5, "margin": 230.0,
  "free_margin": 9782.5, "margin_level": 4353.26, "leverage": 100, "trade_mode": "demo" }
```

## GET /api/positions

```json
{ "positions": [ { "ticket": 123, "symbol": "XAUUSD", "side": "buy", "volume": 0.1, "price_open": 2390.1,
  "price_current": 2391.2, "sl": null, "tp": 2420.0, "profit": 11.0, "swap": 0.0, "magic": 0,
  "comment": "", "time": 1793340000 } ] }
```

## GET /api/orders

Pending orders, in the bridge's `Order` shape (auth-protected like `/api/positions`):

```json
{ "orders": [ { "ticket": 2001001, "symbol": "EURUSD", "type": "buy_limit", "volume_initial": 0.2,
  "volume_current": 0.2, "price_open": 1.0820, "sl": null, "tp": null, "time_setup": 1793340000,
  "comment": "" } ] }
```

## GET /api/watchlist

Prices for `symbols.watchlist`. Symbols that fail are listed with an error, so one bad symbol doesn't fail the whole call.

```json
{ "prices": [ { "symbol": "XAUUSD", "bid": 2391.12, "ask": 2391.40, "last": 0.0, "spread_points": 28,
                "digits": 2, "time": 1793341200 } ],
  "errors": [ { "symbol": "US30", "message": "unknown symbol: US30" } ] }
```

## POST /api/chat

Request:

```json
{ "session_id": "optional, omit to start a new session", "message": "what's gold at?", "provider": "optional provider name" }
```

Response:

```json
{
  "session_id": "4b0d…",
  "reply": "Gold (XAUUSD) is 2391.12 bid / 2391.40 ask.",
  "provider": "local",
  "tool_calls": [
    { "name": "get_price", "arguments": { "symbol": "XAUUSD" }, "ok": true, "duration_ms": 6 }
  ]
}
```

- `message` must be 1–2000 characters.
- At most `[server] chat_rate_limit_per_minute` (20) requests per minute for one login session
  (per client address when login is off): the next one gets `429 rate_limited` with
  `Retry-After`, before the body is read and without calling the model. A request that takes
  longer than `[server] long_request_timeout_secs` (200 s) is stopped with `408 request_timeout`.
- Session history stays in memory (lost on restart) and is capped at the last 40 messages.
- Every turn writes audit rows: `utterance`, `tool_call`, `bridge_request`, `bridge_reply`, `assistant_reply`.

---

# Sprint 2–5 additions

Everything above still holds. New error codes:

| code | HTTP | when |
|---|---|---|
| `unauthorized` | 401 | no or invalid session (when `auth.enabled`), a failed login, or a refused kill-switch release (see below) |
| `forbidden` | 403 | WebSocket upgrade from a foreign `Origin` |
| `not_found` | 404 | unknown `/api/...` path (after the session check, so an unauthenticated client gets 401) |
| `method_not_allowed` | 405 | wrong HTTP method for the path |
| `request_timeout` | 408 | the request took longer than `[server] request_timeout_secs` (30 s), or `long_request_timeout_secs` (200 s) for `/api/chat`, `/api/voice/*` and `/api/confirm/*`; the WebSocket has no limit |
| `payload_too_large` | 413 | body over the limit (64 KB for JSON routes, 5 MB for `/api/voice/transcribe`) |
| `gate_rejected` | 422 | the gate refused to build a preview (message says which limit or why), **or refused a confirmed order when the checks ran again at confirmation** (see `POST /api/confirm/{token}`) |
| `token_unknown` | 404 | confirmation token not found |
| `token_expired` | 410 | confirmation token expired |
| `token_used` | 409 | confirmation token already used or cancelled |
| `kill_switch` | 423 | kill switch engaged; all write endpoints refuse |
| `rate_limited` | 429 | too many requests (`Retry-After` header set): login, confirm, `POST /api/chat` and `POST /api/voice/transcribe` have limits |
| `voice_unavailable` | 503 | voice disabled or models missing |
| `trade_failed` | 502 | MT5 rejected a write (message includes the trade retcode) |
| `outcome_unknown` | 504 | bridge timed out after a write was sent; positions must be checked |

## Authentication (Sprint 4)

When `[auth] enabled = true` (the default), every `/api/*` route except `GET /api/health` and
`POST /api/auth/login` needs a session; so does an unknown `/api/...` path (401 before 404).
The web UI itself (static files) is public: it has to load to show the login page.

The session is a JWT (HS256; claims `sub`, `exp`, `iat`, `jti`; secret from env
`JARVIS_JWT_SECRET`, at least 32 bytes with at least 16 different byte values and no repeated
block — the server refuses to start with login enabled and no usable secret) in the cookie `jarvis_session`:
`Path=/; HttpOnly; SameSite=Strict; Max-Age=<session_hours>`, plus `Secure` when the server
serves TLS itself (or `[auth] cookie_secure = true`, for a TLS-terminating proxy in front of a
loopback port). The only case without `Secure` is plain HTTP on loopback during development.
Chrome and Firefox also accept `Secure` cookies from `http://localhost`, but not every browser does,
so in development prefer the Vite proxy (same origin) or run with TLS. WebSocket upgrades use the
same cookie. Nothing in the API is addressed with a bearer header.

Users are created on the PC that runs Jarvis, from the command line (there is no HTTP endpoint
for it):

```text
jarvis-api user add <name>        # asks for the password twice (no echo), prints the otpauth:// URI and a QR code
jarvis-api user add <name> --password-stdin   # for scripts: the password is the first line of stdin
jarvis-api user reset-totp <name> # new authenticator secret; ends the user's sessions, clears the kill-switch release lock
jarvis-api user list
jarvis-api user remove <name>     # sessions stop working at once
```

These need `JARVIS_JWT_SECRET` (in `.env` or the environment) and use `database.url`. Passwords are
12 to 256 characters and hashed with Argon2id (19 MiB, 2 passes, 1 lane). The TOTP secret
(SHA-1, 6 digits, 30 s) is stored encrypted (ChaCha20-Poly1305, key derived from
`JARVIS_JWT_SECRET` with HKDF-SHA256, bound to the username). **Changing `JARVIS_JWT_SECRET`
logs everyone out and makes the stored TOTP secrets unreadable: run `user reset-totp` for each
user afterwards.** The database file holds password hashes and encrypted secrets, so protect it
like `.env` (readable only by the account that runs Jarvis).

- `POST /api/auth/login` `{username, password, totp}` → `200 {username, expires_at}` (`expires_at`
  is RFC 3339 UTC) and sets the cookie. Every failure — unknown user, wrong password, wrong,
  reused or malformed code, locked account — is the same `401 unauthorized` with the message
  `invalid username, password or code`; the real reason is only in the audit log
  (`login_failed`). A code is accepted for the current 30 s step and one step either side, and
  **never twice** (the last accepted step is remembered per user). Limits: 5 attempts per minute per
  client IP (every attempt counts; the sixth gets `429 rate_limited` with `Retry-After`).
  Lockout is per **(account, client IP)**: 10 failed attempts from one address lock that address
  out of that account for 15 minutes (answered with the same `401`), and nobody else is
  affected, so a stranger who knows the name cannot lock the owner out. Failures from all
  addresses together are counted per account as well; once 50 have been seen within 15 minutes,
  every attempt on that account waits 2 s before it is looked at (a delay, never a lock). The
  state is in memory and a restart clears it. The IP is the TCP peer address; `X-Forwarded-For`
  is not trusted, so behind a proxy every client shares one bucket (and one lockout key).
- `POST /api/auth/logout` → `204`, clears the cookie, and revokes the session's `jti` (kept in
  the database until the token would have expired, so a restart does not revive it).
- `GET /api/auth/me` → `200 {username, expires_at}`; `401` when there is no valid session. When
  auth is disabled: `200 {"username": null, "expires_at": null}`.
- `GET /api/health` → exactly `{"ok":true}` (no auth, no details; for service monitors and the
  desktop launcher).

Session checks on every request: signature and algorithm (`alg: none` and other algorithms are
refused), expiry, not logged out, the user still exists, and no TOTP reset since the token was
issued.

**401 on the kill-switch release** is an answer, not a lost session: a missing, wrong, reused or
rate-limited TOTP code gives `401 unauthorized` with the message `code not accepted`. An expired or
missing session gives `401 unauthorized` with the message `login required`. The UI tells the two
apart with `GET /api/auth/me`.

## Status additions

`GET /api/status` also returns:

```json
{
  "kill_switch": { "engaged": false, "reason": null, "changed_at_ms": null },
  "auth": { "enabled": true, "username": "ninad" },
  "voice": { "stt": true, "tts": true },
  "limits": { "max_lot": 1.0, "max_positions": 5, "max_daily_loss": 500.0,
              "allowed_symbols": ["XAUUSD", "EURUSD"], "trading_hours": null }
}
```

`auth.username` is the caller's name (`null` when auth is disabled). `voice.stt` / `voice.tts`
are true only when that engine loaded at startup.

## Previews and confirmation (Sprint 2)

A preview is created either by the LLM (`propose_*` tools during `/api/chat`) or by a
UI button (close endpoints below). Both go through the same gate. The token is only
ever sent to the browser, never to the LLM.

`PreviewDto`:

```json
{
  "id": "6f1c…",
  "description": "Close 3 positions on XAUUSD (0.30 lots), floating P/L −42.10 USD",
  "symbols": ["XAUUSD"],
  "tickets": [101, 102, 103],
  "total_volume": 0.3,
  "est_pnl": { "current": -42.1, "at_sl": null, "at_tp": null },
  "is_batch": true,
  "count": 3,
  "warnings": [],
  "token": "9a0f…64 hex…",
  "stage": "first",
  "expires_at_ms": 1793341578000,
  "timing": "immediate"
}
```

- `POST /api/chat` response gains `"preview": PreviewDto | null`. The request gains
  optional `"voice": true`; previews created in a voice turn use timing `after_readback`.
- `POST /api/confirm/{token}` → one of:
  - `200 {"status":"executed","preview_id","results":[{"ticket","ok","retcode","retcode_description","order","deal","price","volume","error"}]}`
  - `200 {"status":"needs_second_confirmation","preview_id","token","stage":"second","expires_at_ms","count","message":"Confirm again to close 3 positions"}`
  - errors `token_unknown` / `token_expired` / `token_used` / `kill_switch` / `rate_limited` / `outcome_unknown` / `trade_failed`
    / `gate_rejected`.
  - **The checks run again at confirmation** (a preview is valid for 15 s, and the account and
    the market move). Confirmed actions are sent one at a time, and each is checked under that
    turn:
    - a **new order** goes through the gate again with a fresh snapshot (limits, trading hours,
      daily loss, `max_positions`, stops distance) and must produce the same order (symbol,
      side, type, volume, price, SL, TP). If it fails, the answer is `422 gate_rejected` with a
      message that starts "The checks were run again at confirmation", nothing is sent, the token
      stays spent (a second confirm is `409 token_used`), and the audit log gets a
      `gate_rejected` row with `reason: "re-check at confirm"` and the `rule` (for example
      `limit:max_positions`). Example: two sessions each hold a preview that fits `max_positions`;
      confirming the first succeeds and the second is refused. The order is never adjusted: if the
      market moved so that SL or TP no longer fits the broker's minimum distance, it is refused;
      if it still fits, the order that was confirmed is sent, with its own comment.
    - a **close, cancel or modify** is not held to the limits (it reduces or keeps risk), only to
      the kill switch and to its tickets still existing. If no ticket exists any more the answer
      is `422 gate_rejected` (reason `re-check at confirm`, rule `ticket_gone`). In a batch, the
      tickets that are gone are skipped and listed in `results` as `ok: false` with `error`
      "not sent: position 123 no longer exists"; the rest is sent.
    - If the checks cannot be run (the bridge fails while reading the account), the answer is the
      bridge error, nothing is sent, and the token is spent.
  Rate limit: 10/min **per login session** (keyed by the token's `jti`, so two sessions of the same
  user have separate allowances; with auth disabled it is per client IP). Batch commands run
  sequentially; one failure does not stop the rest; each result is reported.
  - **`trade_failed` (502) is only for a single-write action** (one order, one close) that MT5
    rejected; the message is the broker's error text with the retcode. A **batch** always
    answers `200 executed` with one entry per write, and failures appear as `ok: false` entries in
    `results`. The UI must read `results`, not the HTTP status, for batches.
  - **`aborted`**: when the kill switch is engaged part-way through a batch, the response is
    `200 {"status":"executed", ..., "aborted":"kill_switch"}`. The writes already sent are
    reported as usual; the writes not sent appear as `ok: false` with `error` "not sent: the kill
    switch was engaged". The field is absent otherwise. If the switch was already engaged
    before the first write, nothing is sent and the answer is `423 kill_switch`.
  - **`outcome_unknown` (504)**: a write was sent and no reply arrived (never retried, ADR-0013).
    The message states which write it was ("write 2 of 3"), that its outcome is unknown, **how
    many earlier writes completed**, that the remaining writes were not sent, and tells the
    user to check MetaTrader 5. The UI should refresh positions (the server also emits
    `positions_changed`).
  - **`TradeResultDto.ticket`**: the ticket the write was *about*: the position ticket for a
    close or modify, the order ticket for a cancel, and for a **new order** the order ticket MT5
    assigned (`null` when MT5 did not accept the order). `order` and `deal` are the MT5 order
    and deal numbers of the execution, which for a market order can differ from `ticket`.
- `POST /api/cancel/{token}` → `204` (also cancels the batch sibling).
- `POST /api/readback/{token}` → `200 {"expires_at_ms"}`: the UI calls this when TTS has
  finished reading the preview aloud; the 15 s window starts then (cap 30 s from issue).
- `POST /api/positions/{ticket}/close` `{"volume": null | 0.05}` → `200 PreviewDto`.
- `POST /api/positions/close` `{"symbol": null|"XAUUSD", "side": null|"buy"|"sell", "all": true}` → `200 PreviewDto` (batch).
- `POST /api/orders/{ticket}/cancel` → `200 PreviewDto`.
- UI-button previews (the `/close` and `/cancel` endpoints) are not part of any chat session: they
  live in one shared "ui" bucket, and a new UI preview replaces the previous UI preview. Each
  chat session keeps its own live preview; a chat message never replaces a UI preview and
  the other way round.
- `GET /api/killswitch` → `{engaged, reason, changed_at_ms}`. The state is saved in the database
  (table `kill_switch`): a restart or a service restart after a crash keeps it engaged (audit row
  `kill_switch` with `restored_after_restart`), and only a release with a session and a fresh
  TOTP code turns trading back on. If the saved state cannot be read, trading stays off.
- `POST /api/killswitch` `{"engaged": true, "reason": "manual"}` → same shape. Engaging
  revokes every live token immediately and needs only a session. Releasing needs a valid session
  **and** a fresh TOTP code in the body (`{"engaged": false, "totp": "123456"}`) when auth is
  enabled; a code works once, so the code that opened the session cannot be reused in the same
  30 s step (wait for the next one). Refusals are `401 unauthorized` (see "401 on the kill-switch
  release" above); 5 attempts per minute per user, and failed attempts count towards the account
  lockout. With auth disabled a release needs no code.
- `GET /api/audit?limit=100&before_id=` → `{"entries":[{"id","ts","session_id","kind","payload"}]}` newest first.

Audit kinds added: `intent`, `preview`, `gate_rejected`, `confirm`, `confirm_second`,
`cancel`, `trade_result`, `kill_switch`, `login`, `login_failed`, `voice_stt`, `voice_tts`.

- `intent` rows are written once per chat turn, **only for intents other than `Other`**
  (a confirm, cancel, order, close, modify or cancel-orders request); small talk and questions
  do not get one.
- `login` rows carry `{username, ip, jti}` (and `event: "logout"` for a logout); `login_failed`
  rows carry `{username, ip, reason, locked_now}` with `reason` one of `unknown_user`,
  `bad_password`, `bad_code`, `code_reused`, `locked` (this address is locked out of the account),
  `rate_limited`, `malformed`. Passwords and
  TOTP codes are never written. `username` is the stored spelling of a real account; a name that
  belongs to nobody (and every `rate_limited` and `malformed` row) shows `<not recorded>`, because
  people type passwords into the name field. A flood of refused logins writes one `rate_limited`
  row per client per minute. A refused kill-switch release is a `kill_switch` row with
  `release_refused`.
- `voice_stt` rows hold the transcript text, raw text, timings and the parsed intent, never the
  audio. `voice_tts` rows hold the spoken text and the size of the WAV.

## Voice (Sprint 3)

- `POST /api/voice/transcribe` is limited to `[server] transcribe_rate_limit_per_minute` (20)
  requests per minute per session (`429` with `Retry-After`, checked before the body is read).
- `POST /api/voice/transcribe` body `audio/wav` (`Content-Type: audio/wav`, 16-bit PCM, at most
  `voice.max_seconds` (30 s) and 5 MB) →
  `200 {"text","raw","audio_ms","elapsed_ms","intent":"confirm"|"cancel"|"other"}`.
  The UI uses `intent` to call `/api/confirm/{token}` or `/api/cancel/{token}` when a
  preview is pending; otherwise it sends `text` to `/api/chat` with `"voice": true`.
  `intent` comes from the same deterministic parser the gate uses. A wrong content type,
  an unreadable or too long recording is `400 bad_request`; over 5 MB is `413`. Only one
  transcription runs at a time; a request that waits more than 5 s for its turn gets
  `429 rate_limited`.
- `POST /api/voice/speak` `{"text"}` (1–600 characters) → `200 audio/wav` (`400` for empty or longer text).
- Both answer `503 voice_unavailable` when `[voice]` is off or that engine did not load. Engines
  are loaded at startup; a missing model or a binary built without the `whisper` cargo feature
  logs a warning and switches only that half off. `GET /api/status` shows `voice.stt` / `voice.tts`.
  Speech-to-text needs `cargo build -p jarvis-api --features whisper` (CMake and LLVM); the default
  build has text-to-speech only.

## Events (Sprint 3)

`GET /api/ws` (WebSocket, JSON text frames, server → client). It needs the session cookie
(401 otherwise) and an `Origin` that is the server's own origin or a dev origin (403
otherwise); a request with no `Origin` header is accepted. At most 16 sockets are open at
once (`429`). The server pings every 20 s and drops a socket that has been silent for 70 s; it also
closes (code 1008) when the session ends or expires, and (code 1001) when the server shuts down.
Client messages are ignored (keep them under 1 KB).

```json
{"type":"kill_switch","engaged":true,"reason":"manual"}
{"type":"preview_expired","token":"…"}
{"type":"trade_executed","preview_id":"…","results":[…]}
{"type":"positions_changed"}
```

If the server could not keep up and dropped events for a socket, it sends
`{"type":"positions_changed"}` so the UI re-reads. The UI keeps polling as a fallback; events just
make it faster.

## Static files and TLS (Sprints 4–5)

- `[server] static_dir = "web/build"`: the API serves the built SPA at `/`. A path with a file
  extension is a file or a plain 404; any other path that is not a file gets `index.html`
  (client-side routes). `/api/...` is never served from there (unknown paths are a JSON 404, or
  401 without a session). Cache headers: `public, max-age=31536000, immutable` under
  `/_app/immutable/`, `no-cache` for everything else (`index.html`, the service worker, the
  manifest). API responses carry `Cache-Control: no-store`. Pre-compressed `.br`/`.gz` files next
  to the originals are used when the browser accepts them.
- `[server] tls_cert` / `tls_key` (PEM, key unencrypted): serve HTTPS/WSS (rustls). Both or neither.
  Binding a non-loopback address is refused at startup unless TLS **and** `[auth] enabled` are on.
  With TLS the server sends `Strict-Transport-Security: max-age=31536000; includeSubDomains`, the
  session cookie is `Secure`, and the CSP gets `upgrade-insecure-requests`.
- Security headers on **every** response (API, static files, errors):
  - `Content-Security-Policy: default-src 'self'; script-src 'self' 'sha256-<inline bootstrap>';
    style-src 'self'; style-src-attr 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self'
    blob:; font-src 'self'; connect-src 'self'; worker-src 'self'; manifest-src 'self';
    object-src 'none'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'`.
    - SvelteKit's static build writes one inline `<script>` into `index.html`. At startup the
      server hashes every inline script in the HTML files under `static_dir` and lists the
      hashes in `script-src`; there is no `'unsafe-inline'` or `'unsafe-eval'` for scripts, so
      **rebuild the web UI, then restart the server** (the hashes are read once).
    - `style-src-attr 'unsafe-inline'` exists only because the SvelteKit wrapper element has a
      `style="display: contents"` attribute; `<style>` blocks are not allowed.
    - `connect-src` is `'self'` and nothing else (CSP-1). In CSP level 3 `'self'` covers a
      WebSocket to the same host and port (`ws:` from an `http:` page, `wss:` from an `https:`
      one), which is what the live feed uses, in Chrome, Edge (so the desktop shell's WebView2)
      and Firefox. The bare `ws:`/`wss:` that used to be listed let a script open a WebSocket to
      any host; they were only for old Safari, which is not a supported browser. The header is not
      built from the request's `Host`: it would only repeat what `'self'` already says.
  - `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`,
    `Permissions-Policy: microphone=(self), camera=(), geolocation=()`,
    `Cross-Origin-Opener-Policy: same-origin`, `X-Frame-Options: DENY`.
- Request bodies: 64 KB for JSON routes (including login), 5 MB for `/api/voice/transcribe`;
  larger ones get `413 payload_too_large`.
- Time and connection limits (NET-1, all in `[server]`, see [configuration.md](configuration.md#server)):
  - `header_read_timeout_secs` (10): a client has this long to send the headers of a request. The
    same clock closes a keep-alive connection that sits idle, and a TLS handshake has its own 10 s.
    HTTP/2 (TLS only) is pinged every 20 s and a peer that does not answer within 10 s is dropped,
    with at most 64 streams per connection.
  - `request_timeout_secs` (30) for every route, and `long_request_timeout_secs` (200) for
    `/api/chat`, `/api/voice/*` and `/api/confirm/*`. The limit covers the whole exchange up to the
    response, a slow body upload included. A request that runs out is answered `408` in the
    standard error shape (`request_timeout`). `GET /api/ws` has no limit: the upgrade is
    answered at once, and an open socket is kept by pings (see Events). A confirmed trade keeps
    running when its request is stopped; read `/api/positions` and the audit log.
  - `max_connections` (256): another connection is closed before anything is read from it.
- CORS is off. `[server] dev_origins` (default empty; for example `["http://localhost:5173"]`
  when a dev server calls the API directly) lists browser origins that may call the API with the
  session cookie. Anything listed can drive the whole API, so list only origins you control. The
  Vite proxy (`pnpm dev`) and the desktop shell are same-origin and need no entry.
- Request guard on every `/api/*` route (`403 forbidden`):
  - a state-changing request (not GET, HEAD or OPTIONS) with an `Origin` header must come from
    the server's own origin (the `Origin` equals the `Host`) or a listed dev origin. A request with
    no `Origin` (curl, scripts) passes: it still needs the session cookie. `SameSite=Strict`
    already stops other sites; this also stops other *origins of the same site* (another port on
    `localhost`, a sibling sub-domain);
  - while `[auth] enabled = false`, the `Host` must be a loopback name (`localhost`,
    `*.localhost`, `127.x.x.x`, `[::1]`). Without a session cookie, a web page whose DNS name is
    switched to 127.0.0.1 ("DNS rebinding") would otherwise be same-origin with the API. With
    login on, the cookie is bound to the real host name and no `Host` rule applies, so a reverse
    proxy keeps working.

## Running it

- Console: `jarvis-api` (or `jarvis-api serve`), `--config <file>` or env `JARVIS_CONFIG`. Ctrl+C
  stops it; open WebSockets are closed and other connections get 10 s to finish.
- Windows service (an elevated prompt for install/uninstall/start/stop):
  `jarvis-api service install` registers the current executable as the service `JarvisMT`
  (automatic, delayed start; restarts after a failure after 5 s, 15 s, then 60 s) with the arguments
  `--config <absolute config path> service run --workdir <current directory>`, so install it
  from the directory that holds `config/`, `data/`, `models/` and `.env`. `service uninstall`,
  `service start` and `service stop` do what they say. The service writes a daily rolling log
  to `logs/jarvis-api.log.<date>` under the working directory. It runs as the virtual account
  `NT SERVICE\JarvisMT` (SVC-1) and `install` prints the `icacls` commands that give it its
  files; `service install --account <name>` picks another account (`--password-stdin` reads its
  password) and `--local-system` restores LocalSystem, with a warning. See
  [windows-service.md](windows-service.md).
- Startup refuses: DEMO mode on a real account, login enabled with no usable `JARVIS_JWT_SECRET`,
  a non-loopback bind without TLS and login, and an unreadable TLS certificate or key. A bridge that
  is down, a missing voice model or a missing LLM key only log a warning.
