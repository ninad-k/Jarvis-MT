# Sprint 1 tasks

Plan and decisions: [plan.md](plan.md). Spec: [../SPEC.md](../SPEC.md).
Every task is done only when `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`
and `cargo test --workspace` pass.

## Phase 1: Foundation and bridge spike

- [x] **T1 Workspace, tooling, config skeleton** (S)
  - Acceptance: workspace with `crates/{api,bridge-client,llm}`; `rust-toolchain.toml` (stable); `.gitignore`; `.env.example`; `config/jarvis.example.toml` (mode=demo, bridge endpoint/timeout, aliases, symbol_suffix); `tracing` initialised in `api`.
  - Verify: `cargo build --workspace` and `cargo test --workspace` pass; `cargo run -p jarvis-api` logs config and exits cleanly with no bridge.
  - Files: `Cargo.toml`, `crates/*/Cargo.toml`, `crates/*/src/lib.rs|main.rs`, `config/jarvis.example.toml`, `.env.example`

- [x] **T2 Protocol types and golden fixtures** (S)
  - Acceptance: `Request`, `Reply`, `Command` (read commands only), data DTOs (`Price`, `Account`, `Position`, `Order`, `Deal`, `PingInfo`); fixtures in `crates/bridge-client/tests/fixtures/*.json`.
  - Verify: serde round-trip tests for every fixture; unknown `cmd`/missing `id` rejected.
  - Files: `crates/bridge-client/src/protocol.rs`, `tests/protocol.rs`, `tests/fixtures/`, `docs/protocol.md` (draft)
  - Depends on: T1

- [x] **T3 EA skeleton: ZMQ REP + `ping`** (M, highest risk)
  - Acceptance: `JarvisBridge.mq5` binds `tcp://127.0.0.1:<InpPort>`, polls in `OnTimer` (A2), parses the request with `Json.mqh`, replies to `ping` with `protocol_version`, `ea_version`, `trade_mode`, `terminal_connected`; malformed JSON gets `{"ok":false,"error":...}`, never a crash; socket closed in `OnDeinit`. Vendored mql-zmq headers with LICENSE. `install-ea.ps1` copies files and fetches DLLs (per Q4); `compile-ea.ps1` runs MetaEditor CLI.
  - Verify: `compile-ea.ps1` reports 0 errors, 0 warnings; EA attaches and logs "listening on 127.0.0.1:5555".
  - Files: `mql5/Experts/JarvisBridge.mq5`, `mql5/Include/Jarvis/{Json,Protocol}.mqh`, `mql5/Include/Zmq/*`, `scripts/{install-ea,compile-ea}.ps1`
  - Depends on: T2

- [x] **T4 Rust REQ client + mock bridge** (M)
  - Acceptance: `BridgeClient` actor (A3) with `call(Command) -> Result<T, BridgeError>`; timeout recreates the socket; reply id mismatch is an error; `MockBridge` (feature `mock`) is an in-process REP server with canned data; `examples/bridge_cli.rs` with `ping`.
  - Verify: tests for happy path, timeout then recovery, id mismatch, remote error mapping to `BridgeError::Remote{message, retcode}`.
  - Files: `crates/bridge-client/src/{client,mock,error}.rs`, `tests/client.rs`, `examples/bridge_cli.rs`
  - Depends on: T2

### Checkpoint A
- [x] `bridge_cli ping` against the EA on the demo terminal reports `"trade_mode": "demo"` (2026-10-30, ICMarketsSC-Demo)
- [ ] Stopping the EA mid-session makes the CLI time out; restarting it lets the next call succeed without restarting the CLI (covered by mock tests; not yet tried against the real EA)

## Phase 2: Read path

- [x] **T5 EA read commands** (M)
  - Acceptance: `get_price`, `get_account` (no login/name/server), `get_positions{symbol?}`, `get_orders{symbol?}`, `get_history{from,to,limit≤500}`; symbols validated with `SymbolSelect`/`SymbolInfoInteger`; bad args return `ok:false` with a clear message and `GetLastError()` retcode.
  - Verify: compiles clean; each command exercised via `bridge_cli`, output matches the terminal's Trade/History tabs.
  - Files: `mql5/Include/Jarvis/ReadHandlers.mqh`, `mql5/Experts/JarvisBridge.mq5`
  - Depends on: T3

- [x] **T6 Typed Rust read methods + live tests** (S)
  - Acceptance: `get_price`, `get_account`, `get_positions`, `get_orders`, `get_history` on `BridgeClient`; `bridge_cli` subcommands for each; live tests marked `#[ignore]`.
  - Verify: mock-backed tests pass; `cargo test -p jarvis-bridge-client -- --ignored` passes against the demo EA.
  - Files: `crates/bridge-client/src/client.rs`, `tests/live.rs`, `examples/bridge_cli.rs`
  - Depends on: T4, T5

### Checkpoint B
- [x] All read commands return correct data from the demo account (5 live tests pass; IC Markets names the Nasdaq USTEC)

## Phase 3: LLM and chat

- [x] **T7 `llm` crate: trait + Anthropic** (M)
  - Acceptance: `LlmProvider::chat(ChatRequest) -> ChatResponse` with neutral `Message`, `ToolSpec`, `ToolCall`, `ToolResult` types; Anthropic Messages API with tool use; model and key from config/env.
  - Verify: wiremock tests on request shape and on tool_use/text response parsing; error mapping for 4xx/5xx/timeouts.
  - Files: `crates/llm/src/{lib,types,anthropic,error}.rs`, `crates/llm/tests/anthropic.rs`
  - Depends on: T1 (parallel with T3–T6)

- [x] **T8 OpenAI + Ollama providers** (M)
  - Acceptance: same trait over OpenAI Chat Completions tools and Ollama `/api/chat` tools; provider chosen in config.
  - Verify: wiremock tests per provider; one live smoke test per provider that has a key/model (`#[ignore]`).
  - Files: `crates/llm/src/{openai,ollama}.rs`, `crates/llm/tests/{openai,ollama}.rs`
  - Depends on: T7

- [x] **T9 Tool registry, aliases, agent loop** (M)
  - Acceptance: five read tools with JSON schemas; alias + suffix resolution; agent loop (max 5 tool rounds) with system prompt covering rules 1 and 9; account data redacted (A10); the registry structurally holds read tools only.
  - Verify: tests with a scripted fake LLM + `MockBridge`: "price of gold" → `get_price(XAUUSD)`; "Gold", "GOLD", "cold" alias cases; test asserting no write tool in specs sent to the LLM; tool error surfaced as text, not a panic.
  - Files: `crates/api/src/{tools,symbols,agent}.rs`, `crates/api/tests/agent.rs`
  - Depends on: T6, T7

- [x] **T10 HTTP API + SQLite audit** (M)
  - Acceptance: `POST /api/chat`, `GET /api/status` (bridge up/down, mode, account trade_mode); startup refuses DEMO mode against a real account (A10); SQLx migration for `audit`; utterance, tool call, bridge request/reply and LLM reply each audited.
  - Verify: `axum-test`/`tower::ServiceExt` tests on routes with fake LLM + mock bridge; audit rows asserted per chat turn.
  - Files: `crates/api/src/{main,routes,audit,config}.rs`, `crates/api/migrations/0001_audit.sql`, `crates/api/tests/http.rs`
  - Depends on: T9

- [x] **T11 Minimal web chat** (S)
  - Acceptance: SvelteKit + TS + Tailwind, dark theme; transcript, text box, bridge status pill polling `/api/status`; Vite proxy to the API.
  - Verify: `pnpm -C web check` and `pnpm -C web build` pass; manual chat round-trip in browser.
  - Files: `web/` scaffold, `web/src/routes/+page.svelte`, `web/src/lib/api.ts`
  - Depends on: T10

- [x] **T12 Docs + README** (S)
  - Acceptance: `docs/architecture.md` (diagram + crate roles), `docs/protocol.md` (final for read commands), `docs/runbook.md` (install EA, enable DLL imports for this EA only, start API/web, troubleshooting), README Windows setup steps.
  - Verify: follow README on a clean shell from step 1 to the chat working.
  - Files: `docs/*.md`, `README.md`
  - Depends on: T11

### Checkpoint C — end of Sprint 1 (stop for review)
- [x] Demo script in plan.md passes end to end on the demo account
- [x] fmt, clippy, tests all clean
- [x] Questions for Sprint 2 (intent parser scope, voice token timing) answered (defaults accepted 2026-10-30; ADR-0006, 0007, 0008, 0009)

---

# Sprints 2–5 status

Plan and decisions: [plan-sprints-2-5.md](plan-sprints-2-5.md).

- [x] Sprint 2: EA write commands with validation and reply cache; action gate (limits, previews, tokens, batch double confirmation, kill switch, intent parser); propose tools; confirm, cancel and read-back endpoints; close and cancel previews from the UI; audit API.
- [x] Sprint 3: Whisper speech-to-text and Piper text-to-speech; HUD confirmation modal with countdown; voice turns with read-back timing; WebSocket events.
- [x] Sprint 4: password + TOTP login, JWT session cookie, rate limits, HTTPS, security headers, persisted kill switch, Windows service, operations docs.
- [x] Sprint 5: installable PWA; Tauri 2 desktop shell (Windows installers); mobile build configuration (not built on Windows).
- [x] Open-source project setup: CI, security scanning, release workflow, Dependabot, templates, community docs, ADRs.
- [x] Security review and fixes (docs/security/audit-2026-10-30.md), including BR-1 (authenticated bridge, protocol 2) and BR-2 (libzmq 4.3.5).
- [x] Live verification on a demo account: read, propose, confirm, close and kill switch through the full stack over the signed bridge.

## Open

- [ ] Remaining Low/Info audit findings (see the audit's Findings table for status).
- [ ] Code-sign the Windows installer (needs a signing certificate).
- [ ] GitHub settings: private vulnerability reporting, branch protection on `main`, tag protection for `v*`, a `release` environment with a required reviewer, secret scanning with push protection.
- [ ] Android and iOS builds (scaffolded; need the Android SDK or macOS).
- [ ] First tagged release (`v0.2.0`) once the above are settled.
