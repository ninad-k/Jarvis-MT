# Threat model

Scope: Jarvis MT as of the security review of 2026-10-30 (see
[audit-2026-10-30.md](audit-2026-10-30.md) for the findings and their status). The product rules
being protected are in [SPEC.md](../../SPEC.md) (core rules 1 to 10); the reporting policy is
[SECURITY.md](../../SECURITY.md).

The goal of the design: **a trade cannot reach MetaTrader 5 unless the one user confirmed that
exact preview, within 15 seconds, once, with the kill switch off and every limit respected, even
if the language model is wrong, manipulated, or hostile.** Everything below is judged against that
sentence.

## 1. System and data flow

```
                         INTERNET                          THIS PC / VPS (one Windows user, one trader)
 +--------------------+      |      +--------------------------------------------------------------+
 | Anthropic / OpenAI |<-----+------|  jarvis-api (Axum)                                           |
 |  (HTTPS, API key)  |  TB4        |   routes  -> request_guard (Host/Origin) -> session check    |
 +--------------------+             |   chat turn loop - read + propose_* tools (closed enums)     |
                                    |        |                                                     |
 +--------------------+  TB4        |        v                                                     |
 | Ollama 127.0.0.1   |<------------|   Trading::propose -> gate::evaluate -> TokenStore::issue    |
 +--------------------+             |        (limits, kill switch)            (token -> browser)   |
                                    |   Trading::confirm  -> TokenStore::confirm -> execute()      |
 +--------------------+  TB1/TB2    |        (only place that sends a write)                       |
 | Browser / PWA /    |<----------->|   auth (Argon2id + TOTP + JWT cookie), rate limits, audit    |
 | Tauri window       |  HTTPS+WSS  |   voice: WAV decode, whisper (in-process), Piper (child)     |
 +--------------------+             |   static SPA, SQLite (audit, users, kill_switch)             |
        ^   |                       +---------------------------+----------------------------------+
        |   | mic (push-to-talk)                                | TB5  ZeroMQ REQ/REP, one JSON frame,
        |   +-> TB3 spoken input                                |      tcp://127.0.0.1:5555, no authentication
        |                                                       v
        |                                         +-----------------------------+
        |                                         | JarvisBridge EA (MQL5)      |   TB6
        |                                         |  own JSON parser, CTrade,   |-----------> MetaTrader 5 terminal
        |                                         |  InpAllowReal, reply cache  |             (broker login lives here)
        |                                         +-----------------------------+
        |
   TB7 build and release:  GitHub Actions (SHA-pinned) -> SHA256SUMS + provenance attestation -> user download
   TB8 desktop shell:      Tauri window -> bundled launcher page (IPC) -> navigates to the backend origin (no IPC)
```

Components: `crates/api`, `crates/gate`, `crates/bridge-client`, `crates/llm`, `crates/voice`,
`mql5/`, `web/`, `apps/desktop`, `.github/workflows`, `scripts/`.

## 2. Assets

| ID | Asset | Why it matters |
|---|---|---|
| A1 | Open positions, orders and the account balance reachable through the bridge | Direct financial loss. |
| A2 | Broker login (account number, password, server) | Stays in MT5. Never in this program (rule 8). |
| A3 | Session cookie, JWT signing secret (`JARVIS_JWT_SECRET`), password hashes, encrypted TOTP secrets | Whoever holds them can act as the user. |
| A4 | LLM API keys (`.env`) | Billing and data exposure. |
| A5 | Confirmation tokens (15 s, one use) | The only thing that turns a preview into a trade. |
| A6 | Limits, allowed symbols, kill switch state | The user's risk policy. |
| A7 | Audit log | The only evidence of what happened (rule 10). |
| A8 | Release artifacts and the build pipeline | A tampered installer owns the trading PC. |
| A9 | Voice audio and transcripts | Privacy. Audio is never stored; transcripts go to the audit log. |

## 3. Trust boundaries and actors

| ID | Boundary | Who is on the other side | How the far side is authenticated |
|---|---|---|---|
| TB1 | Browser (or PWA) to API | The user, and any web page the user's browser opens | Session cookie (`HttpOnly`, `SameSite=Strict`, `Secure` with TLS), password + TOTP at login. |
| TB2 | Network to API | Anyone who can reach the port | Loopback bind by default. Non-loopback needs TLS and login (refused at startup otherwise). |
| TB3 | Microphone to speech-to-text | The user, and any sound in the room | Push-to-talk only: the recorder runs while the user holds the mic or Space. |
| TB4 | API to LLM providers | A cloud provider or local Ollama; and, through them, any text the model reads | TLS plus API key for cloud providers. Model output is treated as untrusted input. |
| TB5 | API to EA (loopback ZeroMQ) | The EA, and **any other process on this machine** | Every message is a two-frame message with an HMAC-SHA256 under a shared key, a `ts` within 30 s and a one-time `id` (ADR-0016, finding BR-1, fixed). A process without the key cannot execute, read or answer. |
| TB6 | EA to MT5 | The terminal and the broker | MT5 itself. |
| TB7 | Source to release | GitHub, contributors, dependencies, the user's download | Commit-pinned actions, lockfiles, SHA256SUMS, build provenance attestation. |
| TB8 | Desktop shell to backend UI | The backend origin the user typed | The remote page gets no IPC. Navigation is limited to the bundled pages and one origin. |

Attackers considered:

- **Remote web attacker**: controls a web page the user visits (cross-site, or a sibling origin on
  the same site), no code on the PC.
- **Network attacker**: can reach the API port (only when the user exposed it).
- **Prompt injector**: controls text that reaches the model (a comment on a position or order, a
  symbol description, ambient audio during push-to-talk, or the model's own mistakes).
- **Local unprivileged process or user**: another program or Windows user on the same PC or VPS.
- **Supply chain attacker**: a malicious dependency, action or release artifact.
- **Out of scope** (SECURITY.md): an attacker with administrator rights or a stolen unlocked device.

## 4. Where the safety rules are enforced

| SPEC rule | Enforced in | Tests that fail if it breaks |
|---|---|---|
| 1. The model can only read and propose | `ReadTool` and `ProposeTool` are closed enums (`crates/api/src/tools.rs:37`, `:189`). `execute_in` maps a tool name to one of them and nothing else. No tool takes a token or calls the bridge write methods. | `tool_specs_are_exactly_five_read_and_four_propose`, `executing_every_tool_never_sends_a_write` |
| 2. Preview plus a 15 s one-time token | `Trading::propose_inner` runs `gate::evaluate` then `TokenStore::issue` (`trading.rs:374`, `gate/src/tokens.rs:201`). The tool result for the model is built by `preview_content` (`tools.rs:873`) and leaves the token out. | `buy_gold_full_flow_preview_confirm_position_and_audit`, `expired_token_is_410_and_nothing_is_sent`, `executing_every_tool_never_sends_a_write` (asserts the token is not in the model text) |
| 3. Only `POST /confirm/{token}` writes | `Trading::confirm` (`trading.rs:562`) is the only caller of the bridge write methods. `execute` re-checks that every command `is_write`. | `bridge_write_methods_are_only_called_from_trading_rs` (scans every file under `src/`, the write `Command` variants and the generic `call`), `concurrent_confirms_of_one_token_execute_once`, `replayed_token_is_409_and_unknown_token_is_404` |
| 4. Batches need a second confirmation | `TokenStore::confirm` turns the first token of a batch into a fresh second-stage token (`gate/src/tokens.rs:249`). `is_batch` is `count > 1` (`gate/src/preview.rs`). | `batch_close_needs_a_second_confirmation_then_reports_each_ticket`, `cancelling_either_batch_token_cancels_both`, gate `batch_*` tests |
| 5. Limits before a preview | `gate::evaluate` / `eval_order` (`gate/src/preview.rs:152`, `:436`): `allowed_symbols` (fails closed when empty), volume, `max_lot`, `max_positions`, trading hours, `max_daily_loss`. If today's history is too long to total, new orders are refused. **They run again at confirm** (TRD-2): `Trading::recheck` rebuilds the snapshot and calls `evaluate` on the same proposal under the write lock, and refuses (422, token spent, audit `gate_rejected` "re-check at confirm") when it now fails or would build a different order; the confirmed commands are what is sent, never the new ones. | `max_lot_rejection_is_a_tool_error_without_a_token`, `allowed_symbols_rejection`, `max_positions_rejection_counts_positions_and_pending_orders`, `daily_loss_rejection_uses_todays_realised_pnl_summed_in_rust`, `trading_hours_use_server_time`, `missing_limits_section_fails_closed`, `two_previews_that_each_fit_max_positions_cannot_both_be_confirmed`, `a_daily_loss_that_arrives_after_the_preview_stops_the_order`, `trading_hours_that_end_before_the_confirmation_stop_the_order`, `a_market_that_moved_onto_the_stop_is_refused_not_adjusted`, `a_market_that_moved_but_still_fits_sends_the_confirmed_order_unchanged` |
| 6. Kill switch | Checked in `propose_inner`, `TokenStore::confirm`, `Trading::confirm`, after the write lock is taken, and **immediately before every single write** in `execute` (`trading.rs`, TRD-1). Engaging revokes every live token and never waits for the write lock. Confirmed actions are sent one at a time, and the bridge client serves its write lane before its read lane, so the last window is the one exchange already in flight (a read: at most `bridge.timeout_ms`, 3 s by default). **State is saved and restored** (`trading.rs:1002`, `:1010`, `audit.rs:171`). Release needs a session and a fresh TOTP code (`auth/mod.rs:562`). | `kill_switch_between_preview_and_confirm_blocks_with_423`, `kill_switch_is_rechecked_before_every_write_of_a_batch`, `kill_switch_survives_a_restart_and_needs_a_release`, `an_unreadable_kill_switch_state_keeps_trading_off`, `releasing_the_kill_switch_needs_a_fresh_totp_code`, `confirmed_actions_are_sent_one_at_a_time_and_the_kill_switch_is_seen_in_the_queue`, `a_slow_confirmation_does_not_block_the_kill_switch`, bridge-client `a_queued_write_goes_before_the_reads_queued_ahead_of_it` |
| 7. DEMO by default, refuse real | `ModeMonitor` (`state.rs:74`) is fed by every ping and account read; `propose`, `confirm` and `execute` all refuse on a mismatch. `startup_check` refuses to start. The EA refuses writes on a real account unless `InpAllowReal` (`TradeHandlers.mqh:162`). | `startup_refuses_demo_config_against_real_account`, `real_account_blocks_ui_proposals_and_demo_config_blocks_confirm`, `live_mode_on_a_real_account_is_allowed`, the mock-bridge write tests |
| 8. Credentials stay in MT5 | `Account` has no login, name or server field (`bridge-client/src/protocol.rs`), so there is nothing to redact. `comment` fields from MT5 are filtered before they reach the model (`tools.rs:1092`). | `account_and_positions`, `free_text_comments_from_mt5_never_reach_the_model`, `the_database_never_holds_the_plain_totp_secret_or_password` |
| 9. Ambiguity means ask | System prompt, plus the deterministic parser: `cross_check` rejects a proposal that contradicts side, symbol, volume, price, SL, TP or selector that the user said (`gate/src/intent.rs:1073`). A **new order** also needs the latest message to be an order with a side (LLM-1): after a question, chit-chat or a bare "yes" the model cannot open a trade, and a close that can hit several positions needs a close request. | `proposal_that_contradicts_the_utterance_is_rejected`, `wrong_symbol_or_volume_is_rejected_but_unstated_facts_are_not_checked`, `confirm_wording_in_chat_does_not_create_a_new_proposal`, `a_new_order_after_a_question_is_refused_and_the_model_is_told_to_ask`, `a_new_order_after_a_message_that_is_not_an_order_is_refused`, `spoken_and_hinglish_order_phrases_still_pass`, 100+ phrase corpus in `gate/tests` |
| 10. Audit everything | `Audit::record` (`audit.rs`), append-only by SQLite triggers (`migrations/0001_audit.sql`). Rows: utterance, intent, preview, confirm, bridge request and reply, trade result, kill switch, login. | `buy_gold_full_flow_preview_confirm_position_and_audit`, `audit_endpoint_pages_newest_first` |

## 5. STRIDE by boundary

`S` spoofing, `T` tampering, `R` repudiation, `I` information disclosure, `D` denial of service,
`E` elevation of privilege. "Residual" points to a finding in the audit report.

### TB1 and TB2: browser and network to API

| STRIDE | Threat | Mitigation | Where enforced | Residual |
|---|---|---|---|---|
| S | Password guessing, credential stuffing | Argon2id (19 MiB, 2 passes), TOTP as a second factor, 5 logins per minute per IP, a 15-minute lock after 10 failures **from one client address** on one account (AUTH-1: other addresses, the owner's included, are not affected), a 2 s delay per attempt once 50 failures from all addresses together hit one account within 15 min, one generic 401 for every failure, a dummy hash for unknown users | `auth/mod.rs` (`login`), `auth/lockout.rs`, `auth/crypto.rs`, `ratelimit.rs` | Guessing from very many addresses is slowed, not stopped (the password and the TOTP code are still needed). Lockout state is in memory: a restart clears it. The kill-switch release keeps a per-account lock, but it already needs a valid session. |
| S | TOTP replay | A step is accepted once (`UPDATE ... WHERE totp_last_step < ?`, atomic) | `auth/store.rs:149` | None. |
| S | Forged or downgraded JWT | HS256 pinned, `exp`/`sub` required, expiry checked against the server clock, revocation list in SQLite, `tokens_valid_after` for TOTP resets | `auth/mod.rs:469`, `:504`, `auth/crypto.rs` (`validate_secret`) | The signing secret must be 32 bytes with at least 16 different byte values and no repeated block (AUTH-2); a secret someone chose by hand and that passes can still be weak. |
| S | **Cross-origin request with the user's cookie** (a script on another port of `localhost`, a sibling sub-domain) | `SameSite=Strict`; **state-changing requests must come from the server's own origin**; CORS is off unless `dev_origins` lists an origin | `security.rs:226` (`request_guard`), `routes.rs:52` (CORS from config), `routes.rs:133` (layer) | `dev_origins` entries are fully trusted (documented). |
| S | **DNS rebinding** against an API with login off | With login off, the `Host` must be a loopback name | `security.rs:180`, `:226` | With login on, the host-bound cookie already stops it. |
| T | Body smuggling, oversize bodies | 64 KB JSON limit, 5 MB WAV limit, strict JSON parsing, standard error shape | `routes.rs`, `security.rs:246` | None. |
| T | Path traversal on the SPA | `tower-http` `ServeDir`; extension rule; tested with Windows separators, drive letters, double encoding | `static_site.rs`, test `static_files_cannot_escape_the_directory` | None. |
| R | Denying an action | Every step audited, with the session id and the token prefix (8 characters) | `audit.rs`, `trading.rs` | An administrator can edit the SQLite file (triggers are not a security boundary against file access). |
| I | Tokens, cookies or secrets in logs | Confirm/cancel/readback paths are redacted in request logs; audit stores a token prefix only; LLM keys are marked sensitive; unknown login names are not written | `routes.rs:162` (`redact_path`), `llm/src/util.rs`, `auth/mod.rs:60` | Typed-but-valid names of real accounts are logged (by design). |
| I | Script injection into the SPA | CSP without `unsafe-inline`/`unsafe-eval` for scripts (hash of the one inline bootstrap script), `connect-src 'self'` only (no bare `ws:`/`wss:`, CSP-1), `frame-ancestors 'none'`, `nosniff`, no `{@html}` or `innerHTML` in the SPA | `security.rs:51` | `style-src-attr 'unsafe-inline'` is needed by SvelteKit's wrapper element. |
| I | Stale offline data shown as live | The service worker never touches `/api/*`; API responses are `no-store` | `web/src/service-worker/index.ts`, `security.rs:246` | None. |
| D | Request floods | Login and confirm are rate limited; `/api/chat` and `/api/voice/transcribe` have a per-session budget (20 per minute, NET-2); WebSocket capped at 16; one STT job at a time; audit writes for refused logins are throttled to one per minute per client; **time and connection limits** (NET-1): 10 s to send headers (also the idle keep-alive limit), 30 s per request (200 s for chat, voice and confirm; none on the WebSocket), 256 connections, TLS handshake 10 s | `ratelimit.rs`, `ws.rs`, `voice.rs`, `auth/mod.rs`, `serve.rs`, `routes.rs` | Login-off mode, and rate limits keyed on the TCP peer, mean that behind a proxy all clients share a bucket. A client that opens 256 connections can still use them all up (the limit protects the process, not the availability): put a reverse proxy in front for internet use ([vps.md](../vps.md)). |
| E | Using a normal session to release the kill switch | Release needs a fresh TOTP code that works once | `auth/mod.rs:562` | None. |

### TB3: spoken input

| STRIDE | Threat | Mitigation | Where | Residual |
|---|---|---|---|---|
| S/E | Ambient audio ("confirm") confirms a trade | Push-to-talk only. A confirm phrase must be a whole short utterance (four words or fewer), so a sentence containing "yes" is not a confirmation. The preview is read aloud and the 15 s window starts after the read-back | `web/src/lib/state/voice.svelte.ts`, `gate/src/intent.rs`, `gate/src/tokens.rs:230` | Audio played into the microphone while the user holds the mic can confirm a pending preview. Headphones are advised. |
| T | Mishearing changes the trade ("cold" for "gold") | Alias table, transcript normalisation, preview shows the exact action and numbers, cross-check against the user's words | `voice/src/text.rs`, `gate` | The user must read or hear the preview. |
| D | Malformed WAV, oversized or hostile headers | `hound` decoder with explicit format checks (16-bit PCM or 32-bit float, 1 or 2 channels, 1 to 192 kHz), duration check before allocation, 5 MB cap, 30 s cap, one transcription at a time | `voice/src/audio.rs:20`, `api/src/voice.rs` | None found. |
| E | Command injection through text-to-speech | Text goes to Piper on stdin, never on a command line. Arguments are a fixed list. Control characters stripped, 600 characters, 20 s timeout, process killed on drop | `voice/src/tts.rs:63`, `:193` | The temporary WAV name is predictable but unguessable in practice (`jarvis-piper-<pid>-<n>-<ns>.wav`). |

### TB4: LLM providers

| STRIDE | Threat | Mitigation | Where | Residual |
|---|---|---|---|---|
| E | **Prompt injection makes the model trade** (OWASP LLM01, LLM06) | The model has no write tool. A proposal only creates a preview. The token goes to the browser, never into the model's context. Limits and the kill switch apply to proposals. The preview text comes from the gate, not from the model | `tools.rs`, `trading.rs`, `gate` | A model manipulated into a *proposal* still shows the user a preview they must confirm. A new order is refused unless the user's latest message was an order with a side (LLM-1); the parser reads "should I buy gold?" as an order, so a question phrased that way still passes the cross-check and only the user's confirmation stops it. |
| T | Injection through tool results (position, order or deal comments, OWASP LLM01) | Only `jarvis:<8 hex>` and the broker's `[sl ...]`, `[tp ...]`, `[so ...]` markers reach the model; all other comment text is dropped | `tools.rs:1092` | Symbol names are sanitised by the resolver. Other MT5 text is not sent. |
| I | Secrets in the model's context (LLM02/LLM06) | `Account` has no login, name or server. API keys are never put into prompts. The system prompt holds only time and the alias list | `bridge-client/src/protocol.rs`, `agent.rs:290` | Account balances and positions are sent to a cloud provider if the user picks one. Ollama keeps them local. |
| I | API key sent in clear text | Providers that carry a key must use `https` (loopback `http` is allowed) | `llm/src/util.rs:31` | None. |
| D | Unbounded model use (LLM10) | At most 5 model rounds per message, 40 messages of history, 100 sessions, 2000-character input, 64 KB body | `agent.rs`, `routes.rs` (`spend_budget`) | At most 20 chat requests per minute per session (NET-2). No per-session token budget. |
| T | Model output used unsafely (LLM05) | Output is rendered as text by Svelte (no HTML), never executed or placed into SQL or a shell. Tool arguments are parsed into typed structs and revalidated by the gate | `web/`, `tools.rs:572` | None. |

### TB5 and TB6: bridge and EA

| STRIDE | Threat | Mitigation | Where | Residual |
|---|---|---|---|---|
| S/E | **Another process on this machine sends writes straight to the EA**, skipping the gate | Message authentication (protocol 2, ADR-0016): the EA refuses, before it parses arguments or touches `CTrade`, anything that is not exactly two frames, has a wrong HMAC-SHA256 under the shared `bridge.key`, a `ts` more than 30 s off, or an `id` used in the last 60 s; it answers a signed `-4` and the audit log is unaffected. The EA does not start without the key, the API does not start without it, and the key file is created by `install-ea.ps1` from a CSPRNG and never printed or logged. Loopback bind, field validation, `InpAllowReal` and Algo Trading still apply | `Auth.mqh`, `Protocol.mqh` (`Jarvis_HandleSigned`), `JarvisBridge.mq5`, `bridge-client/src/auth.rs`, `client.rs` | **BR-1: fixed.** A program that can read `bridge.key` (same Windows account, administrator) is not stopped, and messages are not encrypted. Tests: `a_request_signed_with_the_wrong_key_is_refused_and_nothing_runs`, `an_unsigned_single_frame_request_is_refused_and_nothing_runs`, `a_replayed_request_id_is_refused`, `a_stale_or_missing_timestamp_is_refused`, MQL5 `JarvisSelfTest` `auth:` checks, `tests/live_auth.rs`. |
| T | Malformed or hostile JSON | Own parser with limits: 64 KB text, depth 32, 4096 nodes, 16 KB strings; one reply for every request; 64 KB socket message cap | `Json.mqh:16`, `JarvisBridge.mq5:87` | EA code is not fuzzed (see recommendations). |
| T | Double execution after a lost reply | The client never retries a write after sending (ADR-0013). The EA caches the last 64 write replies by request id | `trading.rs` (`outcome_unknown`), `TradeHandlers.mqh:49` | A resend after 64 other writes would execute again; the client never resends. |
| S | A process squats port 5555 while the EA is down | Every reply must carry a valid HMAC under the shared key and echo the request id, or the client returns `reply authentication failed` and drops the socket; account mode is checked on every call | `client.rs`, `auth.rs` | None without the key. A squatter can only make requests fail. Tests: `a_forged_reply_is_detected_and_the_socket_is_reset`, `a_tampered_reply_payload_is_detected`. |
| I | Known flaws in the vendored native libraries | Loopback only. The DLL is built from the pinned official libzmq 4.3.5 source, without CURVE or libsodium, by `scripts/build-libzmq.ps1`; its hash is checked on install | `scripts/build-libzmq.ps1`, `scripts/install-ea.ps1`, `mql5/THIRD_PARTY.md` | BR-2 (libzmq 4.2.1 and libsodium 1.0.11 had published advisories) is fixed: 4.3.5, libsodium removed. Residual: the committed binary is only as trustworthy as the maintainer's build; anyone can rebuild it and compare the hash. |
| D | Flood of requests at the EA | Handles at most 10 requests per 20 ms tick; oversize messages dropped by libzmq | `JarvisBridge.mq5:28` | A local process can keep the EA busy; every message it sends costs one HMAC and one `-4` reply, and nothing is executed. |

### TB7: build and release

| STRIDE | Threat | Mitigation | Where | Residual |
|---|---|---|---|---|
| T | Compromised third-party action | Every action is pinned to a commit SHA; Dependabot updates them with a 7-day cooldown | `.github/workflows/*.yml` (all `uses:` are 40-hex SHAs) | None. |
| E | Workflow token abuse | Default `contents: read`; write permissions only in the publish job; `persist-credentials: false`; no `pull_request_target`; no `${{ github.event.* }}` in shell | `.github/workflows/` | Repository settings (tag rules, required reviews, environments) are the maintainer's to set (see audit report). |
| T | Tampered dependency | Lockfiles committed; `cargo deny` (advisories, licences, sources), `cargo audit`, `pnpm audit`, gitleaks, semgrep, CodeQL, Scorecard in CI; pnpm refuses versions younger than a week (**fixed**: the rule was in the wrong file) | `deny.toml`, `security.yml`, `web/pnpm-workspace.yaml` | Unmaintained transitive crates in the Tauri dependency tree (Linux-only). |
| T | Swapped download | `SHA256SUMS` and a build provenance attestation (`gh attestation verify`) | `release.yml` | The Windows installer is not code-signed. |
| T | Unpinned tool installs in the release job | CMake and LLVM come from Chocolatey for the optional whisper build | `release.yml` | Version-pin them (REL-1). |

### TB8: desktop shell

| STRIDE | Threat | Mitigation | Where | Residual |
|---|---|---|---|---|
| E | The backend page calls Tauri commands | The capability lists no `remote` URLs, so a remote origin gets no IPC. Only two commands exist, and only the bundled launcher may call them | `apps/desktop/src-tauri/capabilities/default.json`, `build.rs` | None. |
| S | Navigation to an attacker page | `on_navigation` allows the bundled pages and the one configured origin; new windows are denied; `http` is accepted only for loopback; credentials in the URL are rejected | `src/backend.rs:51`, `src/lib.rs` | A user can still type a hostile `https` URL. |
| I | Script injection into the launcher | CSP `default-src 'none'`, `script-src 'self'`, `freezePrototype`, error text set with `textContent` | `tauri.conf.json`, `ui/launcher.js` | None. |

## 6. Out of scope and assumptions

- One trader, one machine. Multi-user isolation is not a goal.
- The Windows account that runs Jarvis MT, MT5 and the browser is trusted. Anything that can read
  `.env`, `data/jarvis.db` or process memory can act as the user.
- MT5, the broker and the LLM providers are trusted to do their own jobs.
- Demo accounts are the supported default. Live trading is a deliberate, documented opt-in.

## 7. Keeping this document true

Update it when a trust boundary changes (a new route, a new tool, a new native dependency, a new
way to run the server) and re-run the checks in [audit-2026-10-30.md](audit-2026-10-30.md#method).
