# Changelog

All notable changes to this project are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html). While the version is 0.x, minor releases may contain breaking changes; each one is listed under its release.

## [Unreleased]

Sprints 2 to 5 are built in this repository and not yet released. The maintainer moves these entries under a version heading at release time.

### Added

- Action gate (`jarvis-gate`): limits for lot size, open positions, daily loss, allowed symbols and trading hours, checked before every preview.
- Previews with one-time confirmation tokens (15 seconds by default). Batch actions need a second confirmation that repeats the count.
- Write commands in the `JarvisBridge` EA, run only on request: market and pending orders, position modify, close and partial close, and order cancel. The EA refuses real accounts unless `InpAllowReal` is set, refuses writes while Algo Trading is off, and caches the last 64 write replies by request id.
- `propose_*` tools for the chat model. They create previews and never send an order.
- Confirm, cancel and read-back endpoints. The web UI has a confirm modal, close and cancel controls, and pending orders.
- Kill switch that blocks every write, revokes live previews and is checked before each write in a batch. Engaging needs a session. Releasing needs a fresh authenticator code when login is on.
- Deterministic intent parser for confirm, cancel and trade phrases in English and Hinglish, cross-checked against each proposal.
- Voice: Whisper speech-to-text (cargo feature `whisper`), Piper text-to-speech, read-back timing for confirmations, and `scripts/fetch-models.ps1`, which pins and verifies the voice models.
- Login with a password (Argon2id) and a TOTP code, a session cookie (JWT), login rate limits and per-account lockout. The `user` command adds, lists, resets and removes users.
- Audit API (`GET /api/audit`) and a web audit page. New audit kinds cover previews, confirmations, trade results, kill switch changes, logins and voice.
- HTTPS and WSS with PEM certificate files. Binds off loopback are refused unless TLS and login are on.
- WebSocket events at `/api/ws` (kill switch, preview expiry, trade results, position changes).
- Security headers on every response, including a Content Security Policy that allows only the hashes of the built inline scripts.
- The API serves the built web UI from `[server] static_dir`.
- Windows service: `service install`, `uninstall`, `start`, `stop` and `run`, with delayed automatic start, restart after failure, and daily log files in `logs/`.
- Installable web app (PWA) with a manifest and a service worker.
- Desktop app built with Tauri 2 (`apps/desktop`). It loads the backend and contains no trading logic.
- Continuous integration (Rust, web, desktop and documentation checks), security scanning (cargo-deny, cargo-audit, pnpm audit, gitleaks, semgrep, CodeQL), OpenSSF Scorecard, a Whisper build job, and a release workflow for version tags.
- `scripts/check.ps1` runs every local check.
- Documentation: configuration reference, Windows service guide, VPS guide, runbook and architecture.

### Changed

- The `JarvisBridge` EA reports `ea_version` 0.3.0 and accepts write commands. Version 0.1.0 was read-only.
- Bridge protocol version 2: signed two-frame messages. EA 0.1.x/0.2.x and clients older than this release cannot talk to each other; reinstall the EA with `scripts/install-ea.ps1` and set `[bridge] key_file`.
- `config/jarvis.example.toml` has new `[limits]`, `[trading]`, `[auth]` and `[voice]` sections. `.env.example` has `JARVIS_JWT_SECRET`.
- The HTTP API gained confirm, cancel, read-back, close, kill switch, audit, authentication, voice and WebSocket endpoints. See [docs/api.md](docs/api.md).

### Security

- Login: Argon2id password hashing, TOTP codes accepted once per 30-second step, TOTP secrets encrypted at rest, 5 login attempts per minute per client address, and a 15-minute lockout after 10 failed attempts on an account.
- Binds off loopback are refused unless TLS and login are both on.
- Account login, name and server are not part of any data sent to the model or the browser.
- Limits, tokens and the kill switch are checked before every bridge write. A write that gets no reply is never retried.
- A state-changing request to `/api` with a foreign `Origin` is refused (`403`). CORS is off unless `[server] dev_origins` lists origins; it used to allow `localhost:5173` and `127.0.0.1:5173` with credentials.
- While `[auth] enabled = false`, requests to a non-loopback `Host` are refused (DNS rebinding).
- The kill switch is saved in the database and stays engaged after a restart.
- Refused login floods write one audit row per client per minute, and names that belong to no account are not written to the audit log.
- Free text in MT5 comment fields is not passed to the model. Providers that send an API key must use `https` (or loopback).
- Dependencies: `time` 0.3.55 and `ctutils` 0.4.7 (RUSTSEC-2026-0009, RUSTSEC-2026-0385). The minimum Rust version is now 1.88.
- pnpm's minimum release age (one week) now takes effect: it is set in `pnpm-workspace.yaml`, where pnpm reads it.
- The bridge is authenticated (BR-1): every request and reply is signed with HMAC-SHA256 under a per-install key, requests older than 30 s or with a repeated id are refused, and the client rejects unsigned or forged replies. Other local processes can no longer send orders to the EA.
- The EA's ZeroMQ library is libzmq 4.3.5, built from the official source with a pinned checksum by `scripts/build-libzmq.ps1` (BR-2; fixes CVE-2019-6250 and CVE-2020-15166 in 4.2.1). `libsodium.dll` is no longer shipped.
- Security review: see [docs/security/threat-model.md](docs/security/threat-model.md) and [docs/security/audit-2026-10-30.md](docs/security/audit-2026-10-30.md).

## [0.1.0] - 2026-10-30

First release. Read-only chat over live MT5 data, on demo accounts.

### Added

- `JarvisBridge` EA: a ZeroMQ REP server on `127.0.0.1` with the read commands `ping`, `get_price`, `get_account`, `get_positions`, `get_orders` and `get_history`. The EA does not place or change trades.
- Bridge client in Rust, with a timeout on every call, a socket reset after a failure, reply-id checks, and a mock bridge for offline work.
- LLM providers: Anthropic, OpenAI and Ollama, all with tool calling. Ollama with `qwen3.5:9b` is the default.
- Five read tools for the chat: price, account, open positions, pending orders and deal history.
- Totals for deal history (net profit, commission and swap, overall and per symbol) computed by the backend, not by the model.
- Symbol aliases (for example `gold` to `XAUUSD`) and a broker symbol suffix, both set in config.
- Chat API (`POST /api/chat`) and status API (`GET /api/status`), with session history kept in memory.
- Append-only SQLite audit log that records each chat step.
- Web HUD with an account strip, watchlist, positions, chat transcript and bridge status. It runs with the Vite dev server.
- Startup check: the API refuses to run in demo mode against a real account, and chat returns `409 mode_mismatch` if the account changes to real while the API runs.
- Documentation: README with Windows setup, architecture, HTTP API, bridge protocol and runbook.

### Security

- The bridge binds to loopback only, and the EA refuses any other endpoint.
- Account login, name and server are never returned to the LLM or the browser.

[Unreleased]: https://github.com/ninad-k/Jarvis-MT/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ninad-k/Jarvis-MT/releases/tag/v0.1.0
