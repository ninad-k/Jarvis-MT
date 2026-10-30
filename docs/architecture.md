# Architecture (Sprint 1)

Sprint 1 is read-only chat over live MT5 data. The product rules are in [SPEC.md](../SPEC.md). The Sprint 1 decisions (A1 to A12) are in [tasks/plan.md](../tasks/plan.md). The wire contracts are in [api.md](api.md) and [protocol.md](protocol.md).

## Components

```
 Browser (web/, SvelteKit HUD)
    |  HTTP + JSON on /api/*   (Vite dev server on :5173 proxies /api)
    v
 jarvis-api (crates/api, Axum, 127.0.0.1:8787)
    |
    |-- agent loop + read tool registry
    |     |                                   |
    |     v                                   v
    |   jarvis-llm (crates/llm)          jarvis-bridge-client (crates/bridge-client)
    |     - ollama     127.0.0.1:11434       | ZeroMQ REQ, one JSON frame per message
    |     - anthropic  HTTPS, key in .env    | tcp://127.0.0.1:5555 (loopback only)
    |     - openai     HTTPS, key in .env    v
    |                                    JarvisBridge EA (mql5/, ZeroMQ REP)
    |                                        |  MQL5 trade and market data API
    |                                        v
    |                                    MetaTrader 5 terminal (demo account)
    |
    |-- audit writer --> SQLite data/jarvis.db (table audit, append-only)
```

In Sprint 1 the web app runs on the Vite dev server only. `jarvis-api` does not serve `web/build` yet.

## Crates and modules

| Unit | Responsibility | Main files |
|---|---|---|
| `jarvis-bridge-client` (`crates/bridge-client`) | Wire types (`Command`, replies, DTOs). `BridgeClient` is an actor that keeps one request in flight, applies a timeout, resets the socket after a failure and checks reply ids. The `mock` feature provides an in-process REP server. | `src/protocol.rs`, `src/client.rs`, `src/mock.rs`, `src/error.rs`; examples `bridge_cli.rs`, `mock_server.rs` |
| `jarvis-llm` (`crates/llm`) | `LlmProvider` trait (chat with tool specs), `build_provider`, and the Anthropic, OpenAI and Ollama clients. | `src/lib.rs`, `src/anthropic.rs`, `src/openai.rs`, `src/ollama.rs` |
| `jarvis-api` (`crates/api`) | Config loading and validation, HTTP routes, `AppState` (startup check and mode mismatch), symbol resolver, read tool registry, agent loop, clock offset and the SQLite audit log. | `src/config.rs`, `routes.rs`, `state.rs`, `symbols.rs`, `tools.rs`, `agent.rs`, `clock.rs`, `audit.rs`; `migrations/0001_audit.sql` |
| `JarvisBridge` EA (`mql5/`) | ZeroMQ REP socket on loopback, polled from `OnTimer`. Validates each request and answers `ping` and the read commands. Write commands are rejected. | `Experts/JarvisMT/JarvisBridge.mq5`; `Include/Jarvis/{Json,Protocol,ReadHandlers}.mqh` |
| Vendored mql-zmq | ZeroMQ binding for MQL5, with a small local patch | `Include/Zmq/`, `Include/Mql/`, `Libraries/*.dll`; see [THIRD_PARTY.md](../mql5/THIRD_PARTY.md) |
| `web` | SvelteKit HUD: typed API client that mirrors `docs/api.md`, state stores and components. A mock mode replaces the network. | `src/lib/api.ts`, `src/lib/mock.ts`, `src/lib/components/` |
| `scripts/` | Install the EA and its DLLs (with SHA-256 checks), and compile it with MetaEditor. | `install-ea.ps1`, `compile-ea.ps1` |

Dependencies point one way: `jarvis-api` depends on `jarvis-bridge-client` and `jarvis-llm`. The EA and the web app depend only on the wire contracts.

## Request flow: one chat turn

A chat turn starts with `POST /api/chat` with `{session_id?, message, provider?}`. For example, the user asks "what's my equity?".

1. **Validate the request.** The route parses the JSON and checks that the message is not empty and is at most 2000 characters. It checks that `session_id` is a UUID when given, that the provider exists and is available, and that there is no mode mismatch. A mode mismatch returns `409 mode_mismatch`. Errors use the shape in `api.md`.
2. **Start the agent turn.** `Agent::run_turn` writes an `utterance` audit row (text, provider, model). It builds the system prompt, which holds the current UTC and broker server time, the alias list and the rule to use tools for any figure. It loads the session history, which is kept in memory and capped at 40 messages.
3. **Call the model.** The request carries the history and the five read tool specs. If the model returns text, go to step 6. If it returns tool calls, run each one. A message allows at most 5 model rounds. After that the loop stops with a fixed reply.
4. **Run the tool.** `ToolRegistry::execute` parses the arguments, resolves the symbol through `SymbolResolver` (alias lookup, cleanup and suffix), converts dates to MT5 server time, and makes one bridge call. The audit log gets a `bridge_request` and a `bridge_reply` row when a call was sent, and a `tool_call` row with the name, arguments, `ok`, duration and the result truncated to 2000 characters.
5. **Send the bridge request.** The client sends one JSON frame to the EA and waits for the reply, with a default timeout of 3 seconds. The EA answers from its timer. A failure does not produce an HTTP error inside chat. It becomes a tool result that tells the model to say MT5 is unreachable and not to guess. The model then answers from the tool results. Go back to step 3.
6. **Finish the turn.** The reply is saved to the session, with history trimmed to 40 messages. The `assistant_reply` audit row is written. The response is `{session_id, reply, provider, tool_calls[]}`.

Failure paths: if the LLM call itself fails, the turn writes an `error` row and the API returns `502 llm_error`. A failed audit write is logged loudly and does not fail the turn.

## Key design decisions (from tasks/plan.md)

| # | Decision | Why |
|---|---|---|
| A1 | The EA is the ZMQ REP side and binds `tcp://127.0.0.1:5555`. Rust is the REQ client. | Keeps the bridge local. |
| A2 | The EA polls with `ZMQ_DONTWAIT` from `OnTimer` (20 ms) and handles at most 10 requests per tick. | MQL5 is single-threaded. A blocking receive would freeze the EA. |
| A3 | Rust sends every call through one actor task with one request in flight. A timeout (default 3 s) drops and recreates the socket. Reply ids are checked. | REQ/REP is lock-step. One lost reply would wedge the socket. |
| A4 | Protocol types live in `bridge-client` and use `{"id","cmd","args"}`. The golden JSON fixtures are shared with the Rust tests. | One source of truth for the wire format. |
| A5 | JSON in MQL5 uses our own small module (`Json.mqh`). | No suitable licensed library ships with MT5. Our parser makes input validation explicit. |
| A6 | Sprint 1 has three crates: `bridge-client`, `llm` and `api`. | No empty placeholder crates. `gate` comes in Sprint 2 and `voice` in Sprint 3. |
| A7 | The agent loop runs in `api`. The tool registry holds read tools only, and a test asserts that no write tool is sent to the model. | Rule 1 has a regression test from day one. Implemented as a closed `ReadTool` enum with no `register` method, so a write tool cannot be added by accident. |
| A8 | Symbol aliases resolve in `api` before any bridge call. They are case-insensitive and come from config. `symbols.suffix` covers names such as `XAUUSD.m`. | Keeps the model's job small and lets aliases be tested without an LLM. |
| A9 | The SQLite `audit` table is append-only from Sprint 1. | Rule 10 is cheapest to keep when the table exists from the start. Triggers block `UPDATE` and `DELETE`. |
| A10 | `get_account` drops login, name and server, and the backend checks `trade_mode` at startup. | Rules 7 and 8. |
| A11 | Settings are split: `config/jarvis.toml` holds non-secret settings, and `.env` holds secrets. | Secrets stay out of files that might be committed. |
| A12 | Chat is `POST /api/chat`, with history kept in memory per session. Streaming over WebSocket comes with the HUD in Sprint 3. | Smallest thing that proves the loop. |

Other limits in Sprint 1: at most 100 sessions in memory (the least recently used one is dropped), chat messages of 2000 characters, and `get_history` returns up to 50 deals by default and at most 500.

## Safety rules: where each one is enforced

| Rule (SPEC) | Enforced in Sprint 1 | Added by |
|---|---|---|
| 1. The LLM can only read and propose | The model receives five read tools, from a closed enum. The EA answers write commands with `retcode -2` ("not implemented in this EA version"). | Sprint 2: `propose_*` tools through `gate` |
| 2. A preview with a 15 s one-time token | Not present. | Sprint 2 |
| 3. Only `POST /confirm/{token}` sends a write | No write endpoint or write command exists. | Sprint 2 |
| 4. Batch actions need a second confirmation | Not present. | Sprint 2 |
| 5. Limits checked before a preview | Not present. | Sprint 2 (`gate`) |
| 6. Kill switch | Not present. | Sprint 2 |
| 7. DEMO by default; refuse to run DEMO against a real account | Default is `demo`. Startup refuses to run when the bridge is up and reports a real account. A later real account returns `409 mode_mismatch` on chat. The LIVE banner is not built. | Sprint 3 (LIVE banner in the HUD) |
| 8. Credentials stay in MT5 | `get_account` has no login, name or server fields, so they cannot be returned. The EA never sends them. | Already in place |
| 9. Ambiguous intent: ask, never guess | Partly. Symbol errors tell the model to ask the user, and the system prompt forbids stating numbers that did not come from a tool. | Sprint 2 (intent parser and `gate` cross-check) |
| 10. Every step goes to the audit table | `utterance`, `tool_call`, `bridge_request`, `bridge_reply`, `assistant_reply` and `error` rows. | Sprint 2 adds preview and confirmation rows. Sprint 4 adds hardening and a viewer. |
| Loopback only | The EA refuses any non-loopback endpoint. Config validation refuses a non-loopback bridge or API bind address. | Sprint 4 adds JWT authentication |
| Secrets in `.env` only | `config/jarvis.toml` names the environment variable for each key. It does not hold the key. | Sprint 4 adds the JWT secret to `.env` |

## What changes in Sprints 2 to 5

- **Sprint 2:** the `gate` crate (limits, previews, one-time tokens, kill switch, intent parser), `propose_*` tools, `POST /api/confirm/{token}`, and the EA's write commands (`order_send`, `position_modify`, `position_close`, `position_close_partial`, `order_cancel`) in `TradeHandlers.mqh`.
- **Sprint 3:** the `voice` crate (whisper-rs speech-to-text and Piper text-to-speech), the full HUD (orb, watchlist, positions, confirm modal), WebSocket streaming, and the LIVE banner.
- **Sprint 4:** JWT authentication, hardening, and an audit viewer.
- **Sprint 5:** PWA, then a Tauri desktop app. (The sprint for these is inferred from the build order in SPEC.md.)
