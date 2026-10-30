# Spec: Jarvis MT

Voice + chat trading assistant on top of MetaTrader 5. The user talks or types; an LLM
reads market/account state through tools and can only *propose* trades. A trade reaches
MT5 only after the user confirms a short-lived preview.

Status: **draft, awaiting review.** Sprint 1 plan: [tasks/plan.md](tasks/plan.md).

---

## Capability map

| Module id | Responsibility | Depends on |
|---|---|---|
| `mql5-bridge` | MT5 Expert Advisor: ZeroMQ REP server on localhost, JSON protocol, validates input, executes via `CTrade`, never trades on its own | MT5 terminal |
| `bridge-client` | Rust crate: protocol types, ZeroMQ REQ client (timeouts, reconnect), mock bridge for tests | `mql5-bridge` (wire protocol only) |
| `llm` | Rust crate: provider-agnostic chat + tool-calling trait; Anthropic, OpenAI, Ollama | — |
| `gate` | Rust crate: limits, ActionPreview, one-time confirm tokens (15 s), batch double-confirm, kill switch, intent parser | `bridge-client` (types) |
| `voice` | Rust crate: whisper-rs STT with trading prompt, Piper TTS process | — |
| `api` | Rust binary: Axum HTTP/WS, tool registry, agent loop, auth, audit log (SQLite) | all crates above |
| `web` | SvelteKit HUD (orb, watchlist, positions, transcript, confirm modal), PWA, later Tauri | `api` (HTTP/WS contract) |

Build order: `mql5-bridge` + `bridge-client` → `llm` → `api` (read-only chat) → `gate` → `voice` + `web` HUD → auth/hardening → PWA/Tauri.
Crates are created in the sprint that first needs them, so there are no empty placeholder crates.

## Objective

A single trader runs MT5 on their Windows PC or VPS and wants to ask "what's gold at?",
"show my positions", "buy 0.1 gold with SL 2300", "close all nasdaq longs" by voice or
text, with a hard guarantee that nothing executes without their explicit, timed confirmation.

**Users:** one trader (single-tenant). Demo account by default.

## Core rules (non-negotiable, enforced in code and covered by tests)

1. The LLM can only call read tools and `propose_*` tools. No tool executes a trade.
2. `propose_*` returns `ActionPreview {id, description, symbols, tickets, total_volume, est_pnl}` plus a one-time token valid for 15 s.
3. Only `POST /confirm/{token}` (button or spoken "confirm") sends a write command to the bridge.
4. Batch actions (close all, close by symbol/side) need a second confirmation that repeats the count.
5. Limits are checked before a preview is issued: `max_lot`, `max_positions`, `max_daily_loss`, `allowed_symbols`, `trading_hours`.
6. Kill switch disables all write endpoints instantly.
7. Default mode is DEMO. LIVE needs an explicit setting plus a UI banner. The backend also refuses to start in DEMO mode if the account reports `trade_mode = real`.
8. Account credentials stay in MT5. The backend never sends them to the LLM or the browser. `get_account` returns no login, name or server to the LLM.
9. On ambiguous intent the assistant asks a clarifying question and never guesses.
10. Every step is written to the audit table: utterance, intent, preview, confirmation, bridge request, retcode.

## Tech stack

- Backend: Rust stable (1.95, MSVC toolchain), Axum, Tokio, Serde, SQLx + SQLite, tracing, thiserror (libs) / anyhow (bin).
- Bridge: MQL5 EA + [mql-zmq](https://github.com/dingmaotu/mql-zmq) (Apache-2.0, needs "Allow DLL imports"), REQ/REP on `tcp://127.0.0.1:<port>`. Rust side: `zeromq` crate (pure Rust).
- STT: whisper-rs. TTS: Piper as a child process.
- LLM: `LlmProvider` trait with Anthropic, OpenAI, Ollama implementations (all with tool calling) over `reqwest`.
- Frontend: SvelteKit + TypeScript + Tailwind, Three.js orb, PWA; Tauri 2 later.

## Bridge protocol (summary; full contract in `docs/protocol.md`)

```json
// request
{"id": "c0a8…", "cmd": "get_price", "args": {"symbol": "XAUUSD"}}
// reply (success)
{"id": "c0a8…", "ok": true, "data": {"symbol": "XAUUSD", "bid": 2391.12, "ask": 2391.40, "time": 1793341200}, "retcode": 0}
// reply (failure)
{"id": "c0a8…", "ok": false, "error": "unknown symbol: XAUUS", "retcode": 4301}
```

Commands: `ping`, `get_price`, `get_account`, `get_positions`, `get_orders`, `get_history`,
`order_send`, `position_modify`, `position_close`, `position_close_partial`, `order_cancel`.
`retcode` is the MT5 trade retcode for write commands and `GetLastError()` (0 on success) for reads.
`ping` also returns `protocol_version`, `ea_version`, `trade_mode` (demo/real) and `terminal_connected`.

## LLM tools

Read: `get_price(symbol)`, `get_account()`, `get_positions(filter)`, `get_orders(filter)`, `get_history(range)`.
Propose (Sprint 2): `propose_order(symbol, side, volume, type, price?, sl?, tp?)`, `propose_modify(ticket, sl?, tp?)`,
`propose_close(selector)`, `propose_cancel(selector)`.
Symbol aliases come from config (`gold → XAUUSD`, `nasdaq → US100`, `dow → US30`). The STT prompt carries trading
vocabulary and common mishearings (`cold → gold`).

## Commands

```
Build:        cargo build --workspace
Test:         cargo test --workspace
Live tests:   cargo test --workspace -- --ignored      (needs MT5 demo + EA running)
Lint:         cargo clippy --workspace --all-targets -- -D warnings
Format:       cargo fmt --all
Run API:      cargo run -p jarvis-api
Bridge ping:  cargo run -p jarvis-bridge-client --example bridge_cli -- ping
EA compile:   pwsh scripts/compile-ea.ps1              (MetaEditor64 /compile, fails on errors)
EA install:   pwsh scripts/install-ea.ps1 -Terminal <data-folder-id>
Web dev:      pnpm -C web dev
Web test:     pnpm -C web test        (Playwright, Sprint 3+)
```

## Project structure

```
Cargo.toml              workspace
crates/
  api/                  Axum binary: config, routes, tool registry, agent loop, audit (SQLite)
  bridge-client/        protocol types, ZMQ client, mock bridge (feature "mock")
  llm/                  LlmProvider trait + anthropic / openai / ollama
  gate/                 (Sprint 2) limits, previews, tokens, kill switch, intent parser
  voice/                (Sprint 3) whisper-rs STT, Piper TTS
mql5/
  Experts/JarvisBridge.mq5
  Include/Jarvis/       Json.mqh, Protocol.mqh, ReadHandlers.mqh, (S2) TradeHandlers.mqh
  Include/Zmq/          vendored mql-zmq headers (Apache-2.0, with LICENSE)
web/                    SvelteKit app
docs/                   architecture.md, protocol.md, runbook.md
config/                 jarvis.example.toml (non-secret settings, aliases, limits)
scripts/                compile-ea.ps1, install-ea.ps1
tasks/                  plan.md, todo.md
.env.example            secrets only (API keys, JWT secret)
```

## Code style

- Library crates return typed errors (`thiserror`); only the `api` binary uses `anyhow`.
- No `unwrap()`/`expect()` outside tests and startup config.
- Money and volume are `f64` on the wire (MT5 is double-based) but are rounded to the symbol's `volume_step`/`digits` before any write.
- Every bridge call goes through one client that sets a timeout and logs `id`, `cmd`, duration, `ok`, `retcode` via `tracing`.

```rust
#[derive(Debug, Serialize)]
#[serde(tag = "cmd", content = "args", rename_all = "snake_case")]
pub enum Command {
    Ping,
    GetPrice { symbol: String },
    GetPositions { #[serde(skip_serializing_if = "Option::is_none")] symbol: Option<String> },
}

pub async fn get_price(&self, symbol: &str) -> Result<Price, BridgeError> {
    self.call(Command::GetPrice { symbol: symbol.to_owned() }).await
}
```

MQL5: one handler function per command, all input validated before use, no global trading state, no
`OnTick` trading logic.

## Testing strategy

| Level | What | Where |
|---|---|---|
| Unit | protocol serde, alias resolution, agent loop with fake LLM, provider request/response mapping (wiremock), gate rules, token expiry, limits | `crates/*/src` + `crates/*/tests` |
| Intent parser | 100+ phrases incl. Hinglish (Sprint 2) | `crates/gate/tests/intent_phrases.rs` |
| Bridge contract | Rust client vs in-process mock REP server: timeouts, reconnect, id mismatch | `crates/bridge-client/tests` |
| Live integration | Rust client vs real EA on an MT5 **demo** account (`#[ignore]`, run explicitly) | `crates/bridge-client/tests/live.rs` |
| EA build | MetaEditor CLI compile with zero errors | `scripts/compile-ea.ps1` |
| UI | Playwright smoke tests (Sprint 3+) | `web/tests` |

## Boundaries

- **Always:** run `cargo test` + clippy before calling a task done; validate every input on both sides of the bridge; write to the audit table for every step; keep the bridge bound to `127.0.0.1`.
- **Ask first:** adding a dependency not listed in this spec; changing the wire protocol; touching a live (non-demo) account; committing new or updated binaries (the mql-zmq DLLs were approved on 2026-10-30).
- **Never:** give the LLM a tool that writes; let the EA act without a request; send credentials or account login to the LLM/browser; commit secrets; run live-trading tests on a real account.

## Success criteria (whole project)

- Every core rule above has at least one automated test that fails if the rule is broken.
- On a demo account: voice "buy point one gold" → preview with limits checked → spoken "confirm" within 15 s → position opens; retcode and every step visible in the audit log.
- Kill switch blocks a confirm that is already in flight.
- Works on Windows 11 as a service; web UI usable on a phone.

## Open questions

See the "Open for later sprints" section of the Sprint 1 plan.
