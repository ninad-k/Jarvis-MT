# Implementation Plan: Sprints 2–5

Sprint 1 (read-only chat over live MT5 data) is complete. This plan covers the rest of
[SPEC.md](../SPEC.md): trading through the action gate, voice and the HUD, security and
operations, and the PWA and desktop builds, plus the open-source project setup.

Contracts that every part builds against:

- Wire protocol: `crates/bridge-client/src/protocol.rs` (write commands, `get_symbol`).
- Gate: `crates/gate/src/*.rs` (public types and signatures).
- Voice: `crates/voice/src/lib.rs`.
- HTTP: [docs/api.md](../docs/api.md) ("Sprint 2–5 additions").
- Config: [config/jarvis.example.toml](../config/jarvis.example.toml) (`[limits]`, `[trading]`, `[auth]`, `[voice]`).

## Decisions (recorded as ADRs in `docs/adr/`)

| ADR | Decision |
|---|---|
| 0005 | The LLM gets read tools plus `propose_order`, `propose_modify`, `propose_close`, `propose_cancel`. Propose tools return the preview text (never the token). Nothing the LLM can call writes to the bridge. |
| 0006 | The intent parser is deterministic (English + Hinglish). It detects confirm/cancel and extracts the side, symbol, volume and prices the user actually said. A `propose_*` call that contradicts them is rejected and the assistant asks again. A corpus of more than 100 phrases is tested. |
| 0007 | Voice timing: a preview made in a voice turn gets a token whose 15 s window starts when TTS finishes reading it (`POST /api/readback/{token}`), capped at 30 s from issue. |
| 0008 | Batch actions: the first confirmation returns a fresh 15 s second-stage token bound to the same preview, with a message that repeats the count. Cancelling either token cancels both. |
| 0009 | `est_pnl`: for closes, the floating P/L being realised; for new orders, the P/L at SL and at TP from tick value (null when not set). |
| 0010 | Rust computes every money total (history summaries, previews, daily loss). The LLM only quotes them. |
| 0011 | Auth: Argon2id password + TOTP, JWT (HS256) in an HttpOnly/Secure/SameSite=Strict cookie. Login is limited to 5/min per IP and confirm to 10/min per session. Binding off loopback needs TLS and auth. |
| 0012 | EA write safety: the EA refuses writes on real accounts unless input `InpAllowReal = true`. It caches the last 64 write replies by request `id`, so a resent id never executes twice. It only trades on request. |
| 0013 | A write that times out after sending has an unknown outcome. It is never retried; the API returns `outcome_unknown` and refreshes positions. |
| 0014 | Local-first models: Ollama (`qwen3.5:9b`), Whisper `base.en`, Piper `en_US-lessac-medium`. Cloud LLMs are optional. |
| 0015 | Desktop: a Tauri 2 shell bundles the static SPA and talks to the API over HTTPS. Mobile builds are configured and documented, not built on Windows. The PWA covers phones. |

Limits rules: `max_lot` applies per order and to the total of a batch. `max_positions`
counts open positions plus pending orders after the new order. `max_daily_loss` uses
today's realised P/L (from server midnight) plus floating P/L. `allowed_symbols` fails
closed when empty. `trading_hours` uses server time. Daily loss and trading hours only
block **new** orders: closing, modifying and cancelling stay allowed because they reduce
risk. The kill switch blocks everything (SPEC rule 6).

## Work split (parallel)

| Part | Owns | Sprint |
|---|---|---|
| Bridge writes | `mql5/**`, `crates/bridge-client/**`, `docs/protocol.md` | 2 |
| Gate | `crates/gate/**` | 2 |
| API trading | `crates/api/**` (tools, previews, confirm, kill switch, audit API) | 2 |
| Voice | `crates/voice/**`, `scripts/fetch-models.ps1` | 3 |
| Web HUD | `web/**` (confirm modal, close buttons, kill switch, voice, login, PWA, Playwright) | 2–5 |
| Desktop | `apps/desktop/**` | 5 |
| CI and repo hygiene | `.github/**`, `deny.toml`, `.editorconfig`, `SECURITY.md`, `scripts/check.ps1` | all |
| Community docs and ADRs | `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SUPPORT.md`, `CHANGELOG.md`, `docs/adr/**`, `docs/user-guide.md` | all |
| API platform (after API trading) | `crates/api/**` (auth, TLS, WebSocket, voice routes, static files, Windows service, user CLI) | 3–4 |
| Ops docs (last) | `README.md`, `docs/architecture.md`, `docs/runbook.md`, `docs/windows-service.md`, `docs/vps.md` | 4 |
| Security review (last) | findings, fixes, `docs/security/` reports | 4 |

## Definition of done

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features` clean.
- `pnpm -C web check`, `pnpm -C web build`, Playwright smoke tests pass.
- The EA compiles with 0 errors and 0 warnings. The live demo tests open, modify and close 0.01-lot positions and leave nothing open.
- Every SPEC core rule has an automated test.
- `cargo deny check`, `cargo audit`, `pnpm audit --prod`, `gitleaks` and `semgrep` report no unresolved high findings.
