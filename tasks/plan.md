# Implementation Plan: Sprint 1 — Bridge, read tools, text chat

## Overview

By the end of Sprint 1 a user can type "what's gold at?" or "show my positions" into a basic web
chat and get answers built from live data on an MT5 **demo** account. The data path is
LLM → read tool → Rust bridge client → ZeroMQ → MQL5 EA → MT5. No write commands exist anywhere
yet: the EA only implements read commands and the LLM only sees read tools.

Sprint 1 demo script (the end-of-sprint review checkpoint):
1. Attach `JarvisBridge` to a chart in the demo terminal.
2. `cargo run -p jarvis-bridge-client --example bridge_cli -- ping` prints JSON with `"trade_mode": "demo"`.
3. `cargo run -p jarvis-api` and `pnpm -C web dev`, then open the chat.
4. Ask "price of gold", "what's my equity", "list my open positions", "what did I trade yesterday".
   Answers match the MT5 terminal, and the audit table shows each utterance, tool call and bridge request.
5. Ask "buy gold". The assistant says it cannot place trades yet. No write tool exists.

## Architecture decisions

| # | Decision | Why |
|---|---|---|
| A1 | The EA is the ZMQ **REP** server, bound to `tcp://127.0.0.1:5555`. Rust is the REQ client. | Matches the spec. Binding to 127.0.0.1 keeps the bridge local only. |
| A2 | The EA polls with `ZMQ_DONTWAIT` in `OnTimer` (20 ms) and handles at most 10 messages per tick. | MQL5 is single-threaded. A blocking `recv` would freeze the EA, and `OnTick` doesn't fire on quiet markets. |
| A3 | Rust funnels every call through one actor task that keeps one request in flight. On timeout (default 3 s) it drops and recreates the socket. | REQ/REP is strictly lock-step, so one lost reply would wedge the socket forever. Every reply's `id` is checked against the request. |
| A4 | Protocol types live in `bridge-client` and serialise as `{"id","cmd","args"}`. Golden JSON fixtures in `docs/protocol.md` are also used by the Rust tests. | One source of truth for the wire format. |
| A5 | JSON in MQL5 uses our own small module (`Include/Jarvis/Json.mqh`, about 300 lines: a parser into a tagged tree plus a string builder). | No suitable licensed MQL5 JSON library ships with MT5. A small parser we own also makes input validation explicit. *(Question Q3)* |
| A6 | Sprint 1 crates are `bridge-client`, `llm` and `api`. `gate` arrives in Sprint 2 and `voice` in Sprint 3. | No empty placeholder crates. |
| A7 | The agent loop runs in `api`. Tools are registered in a `ToolRegistry` that, in Sprint 1, can only hold read tools. A test asserts that the tool specs sent to the LLM contain no write tool. | Rule 1 gets a regression test from day one. |
| A8 | Symbol aliases resolve in `api` before any bridge call. They are case-insensitive and load from `config/jarvis.toml`. An optional `symbol_suffix` covers broker names like `XAUUSD.m`. | Keeps the LLM's job small and lets aliases be tested without an LLM. |
| A9 | The SQLite `audit` table (`id, ts, session_id, kind, payload_json`) starts in Sprint 1 with an append-only writer. Sprint 4 adds hardening and a viewer. | Rule 10 is cheapest to keep if it's there from the start. |
| A10 | `get_account` drops `login`, `name` and `server` before the LLM sees it. The backend reads `trade_mode` from `ping` at startup and refuses to start in DEMO mode against a real account. | Rules 7 and 8. |
| A11 | Config is split in two: non-secret settings in `config/jarvis.toml`, secrets in `.env` (API keys, later the JWT secret). | Secrets never land in a committed file. |
| A12 | Chat is `POST /api/chat {session_id, message}` → `{reply, tool_calls[]}`, with history kept in memory per session. Streaming over WS arrives with the HUD in Sprint 3. | Smallest thing that proves the loop. |

## Dependency graph

```
protocol types (T2) ──► EA ping (T3) ──► EA read cmds (T5)
        │                    │
        └──► Rust client + mock (T4) ──► typed read methods (T6) ──┐
                                                                   ├──► tools + agent loop (T9) ──► HTTP + audit (T10) ──► web chat (T11)
llm trait + Anthropic (T7) ──► OpenAI + Ollama (T8) ──────────────┘
workspace (T1) underlies everything; docs/README (T12) last.
```

T7/T8 (LLM) don't depend on the bridge and can be built in parallel with T3–T6.

## Task list

The detailed tasks are in [todo.md](todo.md). The riskiest piece, the ZeroMQ round-trip between MQL5 and Rust, comes first.

### Phase 1: Foundation and bridge spike
- T1 Workspace, tooling, config skeleton
- T2 Protocol types and golden fixtures
- T3 EA skeleton: ZMQ REP + `ping` *(highest risk)*
- T4 Rust REQ client with actor, timeout/reconnect, and mock bridge
- **Checkpoint A:** live `ping` round-trip between Rust and the EA on the demo terminal

### Phase 2: Read path
- T5 EA read commands with input validation
- T6 Typed Rust read methods, plus live `#[ignore]` tests
- **Checkpoint B:** `bridge_cli` prints price, account, positions, orders and history that match the terminal

### Phase 3: LLM and chat
- T7 `llm` crate: trait, types, Anthropic provider
- T8 OpenAI and Ollama providers
- T9 Tool registry, aliases, agent loop (fake LLM + mock bridge tests)
- T10 Axum `/api/chat`, `/api/status`, SQLite audit
- T11 Minimal SvelteKit chat page with a bridge status pill
- T12 Docs (architecture, protocol, runbook) and README Windows setup
- **Checkpoint C (end of sprint):** the demo script above passes; Sprint 1 review before Sprint 2 starts.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| mql-zmq is unmaintained (last release 2019, bundles libzmq 4.x) and may not interoperate with the pure-Rust `zeromq` crate | High | T3/T4 spike this first. Fallback: the `zmq` crate (libzmq bindings), which would need cmake. |
| The EA needs "Allow DLL imports" enabled | Medium | Enable it for this EA only, never globally. Document it in the runbook. The DLLs are checksum-verified by `install-ea.ps1`. |
| REQ socket wedges after a lost reply | High | A3: the actor recreates the socket on timeout. A test kills the mock mid-request. |
| Broker symbol names differ (`XAUUSD.m`, `NAS100` vs `US100`) | Medium | Aliases plus `symbol_suffix` in config. The EA rejects unknown symbols with a clear error. |
| LLM invents numbers instead of calling tools | Medium | The system prompt requires tools for any market or account figure. Tests on the agent loop. Sprint 2's eval set extends this. |
| Large `get_history` replies | Low | `limit` argument capped at 500, with a date range required. |
| `cmake`/`clang` aren't installed (whisper-rs needs them in Sprint 3) | Low (S1) | Not needed in Sprint 1. Flagged now so it can be installed before Sprint 3. |

## Decisions (2026-10-30)

- **Q1 Development terminal:** the "C:\Program Files\MetaTrader 5" terminal on an ICMarketsSC-Demo account. Broker names: gold `XAUUSD`, Nasdaq `USTEC`, Dow `US30`, S&P `US500`.
- **Q2 LLM:** Anthropic `claude-sonnet-5-5` when a key is set, plus a local Ollama model (`qwen3.5:9b`, the config default). All three providers are built and unit-tested.
- **Q3 JSON in MQL5:** our own small `Json.mqh` (A5).
- **Q4 ZMQ DLLs:** committed to the repo, with SHA-256 checksums in `mql5/THIRD_PARTY.md`.
- **Q5 Sprint 1 UI:** a professional HUD limited to Sprint 1 features, including the orb.

## Open for later sprints

- **"Intent parser" (100+ phrases incl. Hinglish):** the LLM already does intent through tool calls. Proposed reading: a deterministic parser in `gate` that (a) detects confirm/cancel in speech, (b) extracts side, symbol, volume and selectors so it can cross-check every `propose_*` call against the user's words, and (c) is tested with the phrase corpus. To be confirmed before Sprint 2.
- **15-second tokens with voice:** reading a preview aloud can take 5–8 s, which leaves little time to say "confirm". Option: start the 15 s when TTS finishes, while keeping a hard cap of about 30 s from issue. Decide in Sprint 2.
- **Batch second confirmation:** does it get a fresh 15 s token, or must both confirmations fit inside the original 15 s? *[fresh token, bound to the first]*
- **`est_pnl` for new orders:** proposal: P&L at SL and at TP, or "n/a" without them. For closes, it's the current floating P&L.
