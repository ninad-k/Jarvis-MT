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
