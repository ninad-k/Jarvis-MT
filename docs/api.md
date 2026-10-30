# HTTP API (Sprint 1)

Served by `jarvis-api` on `server.bind` (default `http://127.0.0.1:8787`). All bodies are JSON.
Sprint 1 has read-only endpoints. Write endpoints (`/api/confirm/{token}` and so on) arrive in Sprint 2.
Auth (JWT) arrives in Sprint 4. Until then the server binds to loopback only.

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
    "ea_version": "0.1.0",
    "protocol_version": 1,
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
- Session history stays in memory (lost on restart) and is capped at the last 40 messages.
- Every turn writes audit rows: `utterance`, `tool_call`, `bridge_request`, `bridge_reply`, `assistant_reply`.

---

# Sprint 2–5 additions

Everything above still holds. New error codes:

| code | HTTP | when |
|---|---|---|
| `unauthorized` | 401 | no or invalid session (when `auth.enabled`) |
| `gate_rejected` | 422 | the gate refused to build a preview (message says which limit or why) |
| `token_unknown` | 404 | confirmation token not found |
| `token_expired` | 410 | confirmation token expired |
| `token_used` | 409 | confirmation token already used or cancelled |
| `kill_switch` | 423 | kill switch engaged; all write endpoints refuse |
| `rate_limited` | 429 | too many requests (`Retry-After` header set) |
| `voice_unavailable` | 503 | voice disabled or models missing |
| `trade_failed` | 502 | MT5 rejected a write (message includes the trade retcode) |
| `outcome_unknown` | 504 | bridge timed out after a write was sent; positions must be checked |

## Authentication (Sprint 4)

When `[auth] enabled = true` (the default), every `/api/*` route except
`/api/auth/login` and `/api/health` needs a session. The session is a JWT (HS256,
secret from env `JARVIS_JWT_SECRET`, ≥ 32 bytes) in an `HttpOnly; Secure; SameSite=Strict`
cookie named `jarvis_session`. WebSocket upgrades use the same cookie.
Users are created from the CLI (`jarvis-api user add <name>`), which prints a TOTP
`otpauth://` URI (and an ASCII QR code) to add to an authenticator app. Passwords are hashed with
Argon2id.

- `POST /api/auth/login` `{username, password, totp}` → `200 {username, expires_at}` and sets the cookie.
  Wrong credentials → `401 unauthorized` (same message for every failure). Rate limit: 5/min per IP.
- `POST /api/auth/logout` → `204`, clears the cookie.
- `GET /api/auth/me` → `{username, expires_at}` or `401`.
- `GET /api/health` → `{"ok": true}` (no auth, no details; for service monitors).

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
  - errors `token_unknown` / `token_expired` / `token_used` / `kill_switch` / `rate_limited` / `outcome_unknown`.
  Rate limit: 10/min per session. Batch commands run sequentially; one failure does not
  stop the rest; each result is reported.
- `POST /api/cancel/{token}` → `204` (also cancels the batch sibling).
- `POST /api/readback/{token}` → `200 {"expires_at_ms"}`: the UI calls this when TTS has
  finished reading the preview aloud; the 15 s window starts then (cap 30 s from issue).
- `POST /api/positions/{ticket}/close` `{"volume": null | 0.05}` → `200 PreviewDto`.
- `POST /api/positions/close` `{"symbol": null|"XAUUSD", "side": null|"buy"|"sell", "all": true}` → `200 PreviewDto` (batch).
- `POST /api/orders/{ticket}/cancel` → `200 PreviewDto`.
- `GET /api/killswitch` → `{engaged, reason, changed_at_ms}`.
- `POST /api/killswitch` `{"engaged": true, "reason": "manual"}` → same shape. Engaging
  revokes every live token immediately. Releasing needs the TOTP code in the body
  (`{"engaged": false, "totp": "123456"}`) when auth is enabled.
- `GET /api/audit?limit=100&before_id=` → `{"entries":[{"id","ts","session_id","kind","payload"}]}` newest first.

Audit kinds added: `intent`, `preview`, `gate_rejected`, `confirm`, `confirm_second`,
`cancel`, `trade_result`, `kill_switch`, `login`, `login_failed`, `voice_stt`, `voice_tts`.

## Voice (Sprint 3)

- `POST /api/voice/transcribe` body `audio/wav` (16-bit PCM, ≤ 30 s, ≤ 5 MB) →
  `200 {"text","raw","audio_ms","elapsed_ms","intent":"confirm"|"cancel"|"other"}`.
  The UI uses `intent` to call `/api/confirm/{token}` or `/api/cancel/{token}` when a
  preview is pending; otherwise it sends `text` to `/api/chat` with `"voice": true`.
- `POST /api/voice/speak` `{"text"}` (≤ 600 chars) → `200 audio/wav`.

## Events (Sprint 3)

`GET /api/ws` (WebSocket, JSON text frames, server → client):

```json
{"type":"kill_switch","engaged":true,"reason":"manual"}
{"type":"preview_expired","token":"…"}
{"type":"trade_executed","preview_id":"…","results":[…]}
{"type":"positions_changed"}
```

The UI keeps polling as a fallback; events just make it faster.

## Static files and TLS (Sprints 4–5)

- `[server] static_dir = "web/build"`: the API serves the built SPA at `/` with
  `index.html` fallback, so one process serves everything.
- `[server] tls_cert` / `tls_key` (PEM): serve HTTPS/WSS. Binding a non-loopback
  address is refused unless TLS **and** auth are enabled.
- CORS: dev origins plus `tauri://localhost` and `http://tauri.localhost` for the desktop app.
