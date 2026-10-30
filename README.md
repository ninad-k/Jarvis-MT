# Arc MT

[![CI](https://github.com/ninad-k/Jarvis-MT/actions/workflows/ci.yml/badge.svg)](https://github.com/ninad-k/Jarvis-MT/actions/workflows/ci.yml)
[![Security](https://github.com/ninad-k/Jarvis-MT/actions/workflows/security.yml/badge.svg)](https://github.com/ninad-k/Jarvis-MT/actions/workflows/security.yml)
[![OpenSSF Scorecard](https://github.com/ninad-k/Jarvis-MT/actions/workflows/scorecard.yml/badge.svg)](https://github.com/ninad-k/Jarvis-MT/actions/workflows/scorecard.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)

Arc MT is a voice and chat trading tool for MetaTrader 5 (MT5). You type or say a request such as "what's gold at?"
or "buy 0.01 gold", and a language model reads live data from your terminal or proposes an order. Nothing is sent to MT5 until
you confirm a short-lived preview, by button or by voice. It is built for one trader running MT5 on a Windows PC or a VPS.
You can talk to it as **Jarvis** or **Friday**, two personas that differ in voice and manner but not in what they are allowed to do.

The repository, the crates and the binaries keep their `jarvis-*` names.

> **Disclaimer.** Arc MT is not financial advice and comes without warranty (see [LICENSE](LICENSE)). Trade only on demo accounts
> until you have tested the whole flow. You are responsible for every trade you confirm.

## Safety model

The rules below are the core rules of [SPEC.md](SPEC.md). Each one is enforced in code and covered by tests.

1. The language model can only call read tools and `propose_*` tools. No tool sends a trade.
2. Every proposal is a preview with a one-time token that expires after 15 seconds.
3. Only a confirmation (the button, or a spoken "confirm") sends a write to MT5.
4. Batch actions, such as close all, need a second confirmation that repeats the count.
5. Limits are checked before a preview is shown: lot size, open positions, daily loss, allowed symbols and trading hours.
6. A kill switch blocks every write at once.
7. The default mode is DEMO. LIVE needs an explicit setting, and the backend refuses to start in DEMO against a real account.
8. Account login, name and server stay in MT5. They never reach the language model or the browser.
9. On an unclear request, the app asks instead of guessing.
10. Every step, from the utterance to the trade result, is written to an append-only audit log.

The bridge to MT5 listens on loopback only, and remote access requires HTTPS and login.

## Features

Marked **(local)** when the feature runs entirely on your PC, with no cloud service involved.

- **Market data and account (local):** bid and ask prices, account summary, open positions, pending orders and closed deals with totals
- **Broker symbol mapping (local):** you say "gold" or "nasdaq" and Arc MT uses your broker's own symbol names. The map is detected from the broker's full symbol list when you connect, and again when you switch broker in MT5. You can review and override it in Settings. See [docs/symbol-mapping.md](docs/symbol-mapping.md).
  that the backend computes. Totals are never estimated by the model.
- **Chat (Ollama is local; Anthropic and OpenAI are cloud):** typed questions in English and Hinglish, with tool calls shown in the transcript.
- **Personas (local):** two built-in personas. **Jarvis** has a male British voice and speaks calmly and formally. **Friday** has a female US voice and is warm and casual.
  Switch with the persona toggle, or say "Hey Jarvis" or "Hey Friday" in hands-free mode. A persona changes the name, the wording, the voice, the wake phrase and the colours. Tools, limits, confirmations and safety rules are the same for both. See [ADR-0017](docs/adr/0017-personas.md) and the [user guide](docs/user-guide.md#personas-available-from-v020).
- **Settings page (local):** persona, language model, voice input and output, watchlist, symbol aliases and trading limits. The UI can only make the limits stricter than the config file, and changing them needs an authenticator code. See the [user guide](docs/user-guide.md#settings-available-from-v020).
- **Trading with confirmation (local):** market and pending orders, stop loss and take profit changes, closes (full or partial) and cancels.
  Each one is a preview with a 15-second token. Batch actions need two confirmations.
- **Limits and kill switch (local):** `max_lot`, `max_positions`, `max_daily_loss`, `allowed_symbols`, trading hours, and a kill switch that blocks writes.
- **Login (local):** password (Argon2id) with an authenticator code (TOTP), a session cookie, rate limits and lockout. Users are created on the machine itself.
- **Voice (local):** speech-to-text with Whisper and text-to-speech with Piper, with one Piper voice per persona. Confirmations are read back aloud, and the 15-second window starts when reading finishes. Optional hands-free wake-word mode listens for a persona's wake phrase.
- **Audit (local):** a SQLite log of every step, with an audit page in the web UI and an API.
- **Web UI and PWA:** a HUD with an orb, watchlist, positions, transcript, confirm modal, kill switch and audit page. Installable on a phone.
- **Desktop app (local):** a Tauri 2 window around the same web UI. It contains no trading logic.
- **Windows service (local):** runs the backend at boot, restarts it after a failure, and writes daily log files.
- **Remote access (VPS):** HTTPS and WSS, with a certificate you provide or a reverse proxy. See [docs/vps.md](docs/vps.md).

## Quick start (Windows)

### Prerequisites

1. **Rust** (stable) from [rustup](https://rustup.rs). The crates need Rust 1.88 or newer.
2. **Node.js 22 or newer** and **pnpm**.
3. **MetaTrader 5** with a **demo** account, logged in. Open the terminal once so its data folder has an `MQL5` subfolder.
4. **PowerShell 7** (`pwsh`) for the scripts. Windows PowerShell 5.1 also works if you use `powershell` in place of `pwsh`.
5. **A language model**, one of:
   - **Ollama** with the default model: `ollama pull qwen3.5:9b`. This is the `local` provider.
   - or an **Anthropic** or **OpenAI** API key, kept in `.env` and selected in `config\jarvis.toml` or in the web UI.

### Fast path: the scripts

`scripts\setup.ps1` does steps 1, 2 and 5 below for you, and `scripts\run.ps1` starts everything. From the repository root:

```powershell
scripts\setup.ps1 -WhatIf              # preview: toolchain checks, config, .env, models, web build, release build, EA, Ollama
scripts\setup.ps1                      # run it (add -DataFolder <32-character-id> to install the EA; see below)
target\release\jarvis-api.exe user add <name>      # the first login user (asks for a password, shows the QR code)
scripts\run.ps1                        # start the API and web UI; Ctrl+C stops everything
scripts\status.ps1                     # health check (exit code 0 = the API is healthy)
scripts\stop.ps1                       # fallback: stop this repository's processes if run.ps1 did not clean up
```

`setup.ps1` is safe to run twice: it never overwrites `config\jarvis.toml` or `.env`, fills `JARVIS_JWT_SECRET` only when it is empty (the value is never printed),
and skips what is already done. Skip parts with `-SkipModels`, `-SkipEa`, `-SkipWeb` or `-SkipBuild`; `-Whisper` builds speech-to-text (needs the MSVC build tools, CMake and LLVM).
It installs the EA only when you pass `-DataFolder`, or when exactly one MT5 terminal is running and its data folder can be found. `run.ps1 -Dev` adds the Vite dev server, and `-StartOllama` starts `ollama serve`.
To start Arc MT at Windows logon, use `scripts\install-autostart.ps1`. For phone access and firewall rules see [docs/vps.md](docs/vps.md), and for all the scripts the [runbook](docs/runbook.md#scripts).

The steps below are the same thing done by hand. Step 3 (attach JarvisBridge in MT5) and step 4 (the first user) are yours in either case.

### 1. Create your config and secrets

From the repository root:

```powershell
Copy-Item config\jarvis.example.toml config\jarvis.toml
Copy-Item .env.example .env
```

In `config\jarvis.toml`, set `[symbols] suffix` and the aliases to your broker's names (for example `.m` if the broker appends it).
In `.env`, set `JARVIS_JWT_SECRET` to a random value of at least 32 bytes. Generate one with:

```powershell
$r = [Security.Cryptography.RandomNumberGenerator]::Create(); $b = New-Object byte[] 48; $r.GetBytes($b); [Convert]::ToBase64String($b)
```

Put any provider keys in `.env` too. Both files are gitignored. Every setting is in [docs/configuration.md](docs/configuration.md).

### 2. Install and compile the EA

Find your terminal's data folder in MT5 under **File > Open Data Folder**. Its name is a 32-character ID. Then:

```powershell
pwsh scripts/install-ea.ps1 -DataFolder <32-character-id>
pwsh scripts/compile-ea.ps1 -DataFolder <32-character-id>
```

`-DataFolder` is required. If MT5 is not in `C:\Program Files\MetaTrader 5`, pass `-MetaEditor "<path to MetaEditor64.exe>"`
to `compile-ea.ps1`. Both scripts check the DLL hashes in [mql5/THIRD_PARTY.md](mql5/THIRD_PARTY.md).

`install-ea.ps1` also creates the bridge key (once; `-RotateKey` replaces it) and prints its path. Put that path in
`config\jarvis.toml`:

```toml
[bridge]
key_file = '<data folder>\MQL5\Files\JarvisMT\bridge.key'
```

### 3. Attach JarvisBridge

In MT5, run the self-test first: **Navigator (Ctrl+N) > Scripts > JarvisMT > JarvisSelfTest**. The Toolbox Experts tab (Ctrl+T) should end with
`=== JarvisSelfTest done: <N> passed, 0 failed ===`.

Then attach the EA to a chart: **Navigator > Expert Advisors > JarvisMT > JarvisBridge**. On its **Common** tab, tick **Allow DLL imports** and
**Allow Algo Trading**. Keep the global option under Tools > Options > Expert Advisors off. The **Algo Trading** button on the toolbar must be on,
because writes are refused while it is off.

The Experts tab should show a line that starts with `JarvisBridge 0.4.0 listening on 127.0.0.1:5555`.

Check the bridge from the repository root:

```powershell
cargo run -p jarvis-bridge-client --example bridge_cli -- --key-file <path to bridge.key> ping
```

`<path to bridge.key>` is the file `scripts/install-ea.ps1` created (`MQL5\Files\JarvisMT\bridge.key` in the MT5 data folder; the script prints it). The EA answers only a holder of that key.

The reply should contain `"trade_mode": "demo"`.

### 4. Create the first login user

Login is on by default, so create a user before you try to sign in. You will be asked for a password (at least 12 characters) and
the QR code for your authenticator app:

```powershell
cargo run -p jarvis-api -- user add <name>
```

### 5. Build the web UI and start the API

```powershell
pnpm -C web install
pnpm -C web build
```

In `config\jarvis.toml`, uncomment `static_dir = "web/build"` under `[server]`. Then start the API from the repository root:

```powershell
cargo run -p jarvis-api
```

Open **http://127.0.0.1:8787**, sign in with your password and the six-digit code.

For UI development with hot reload, run `pnpm -C web dev` in a second terminal and open **http://localhost:5173**. Vite proxies `/api` to port 8787.

The API starts even when MT5 is down, and reports the bridge as disconnected.

### 6. Your first trade (demo)

Ask a few read questions first: "what's gold at?", "what's my equity?", "show my positions", "what did I trade yesterday?".

Then try a trade with a small volume:

- "buy 0.01 gold" creates a preview. Nothing is sent yet.
- Click **Confirm**, or say "confirm", within 15 seconds. The order goes to the demo account, and the result shows the MT5 return code.
- "close all nasdaq longs" is a batch. It asks you to confirm twice, and the second prompt repeats the count.

The phrases and the Hinglish equivalents are in [docs/user-guide.md](docs/user-guide.md). Check the numbers against MT5.

### Voice (optional)

Voice needs local models and a build with the `whisper` feature.

```powershell
pwsh scripts/fetch-models.ps1
```

This downloads Piper, the two Piper voices (`en_US-lessac-medium` for Friday and `en_GB-alan-medium` for Jarvis, both from `rhasspy/piper-voices` at one pinned revision) and the Whisper `base.en` model into `models\`. It checks their SHA-256 hashes and skips files that already match.
The `[voice]` section of the example config already points at the Piper and Whisper paths and has `enabled = true`. Each persona's voice is set in its pack under `personas\`. Restart the API after fetching the models.

Text-to-speech works with the default build. Speech-to-text needs the `whisper` feature, which compiles whisper.cpp. Install these first:

- **Visual Studio Build Tools** with the "Desktop development with C++" workload.
- **CMake** (on `PATH`).
- **LLVM** (provides `libclang.dll`, normally in `C:\Program Files\LLVM\bin`).

Build inside the MSVC environment. Run `vcvars64.bat` from the Build Tools in the same shell window (or use the x64 Native Tools prompt), then:

```powershell
$env:CMAKE_C_FLAGS_RELEASE = "/O2 /Ob2 /DNDEBUG"
$env:CMAKE_CXX_FLAGS_RELEASE = "/O2 /Ob2 /DNDEBUG"
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
cargo build --release -p jarvis-api --features whisper
```

If the build fails with `stdio.h not found` or similar C header errors, the shell did not load the MSVC environment. See the runbook troubleshooting table.

### Desktop app (optional)

The Tauri shell opens the web UI in its own window. It loads the backend from the address you give it, so the backend must be running:

```powershell
pnpm -C apps/desktop install
pnpm -C apps/desktop dev
```

The default backend address is `https://127.0.0.1:8787`. For plain HTTP on loopback, enter `http://127.0.0.1:8787` in the launcher. For HTTPS, see [docs/desktop.md](docs/desktop.md).

## Develop without MT5

- **Full stack on canned prices.** Start the mock EA on the bridge port, then the API and the web UI as above. Stop it before you attach the real EA, because both use port 5555:

  ```powershell
  cargo run -p jarvis-bridge-client --features mock --example mock_server
  ```

  It serves prices for XAUUSD, US100, US30, EURUSD, GBPUSD and BTCUSD and reports a demo account. Chat still needs a language model.

- **UI only.** `pnpm -C web dev:mock` runs the web app with no backend. Every API call is answered from `web/src/lib/mock.ts`.

## Tests

| Command | Needs | What it checks |
|---|---|---|
| `cargo test --workspace --features jarvis-bridge-client/mock` | Rust | Unit and contract tests: protocol fixtures, the mock bridge, the chat loop with a fake model, the gate, auth and the HTTP API |
| `cargo clippy --workspace --all-targets --features jarvis-bridge-client/mock -- -D warnings` | Rust | Lints, with warnings as errors |
| `cargo fmt --all --check` | Rust | Formatting |
| `pnpm -C web check` and `pnpm -C web test` | `pnpm -C web install` | Svelte and TypeScript checks, unit tests |
| `pnpm -C web test:e2e` | Browsers for Playwright | End-to-end tests of the web UI |
| `pwsh scripts/check.ps1` | Everything below | All of the above, the whisper feature, the desktop shell, web build, security scans and a Windows PowerShell 5.1 syntax check of `scripts\*.ps1` |
| `cargo test -p jarvis-bridge-client --features mock --test live -- --ignored --test-threads=1` | Demo terminal with the EA attached | Read-only live tests against the EA |
| `cargo test -p jarvis-bridge-client --features mock --test live_trading -- --ignored --test-threads=1` | Demo terminal with the EA attached | **Places and closes 0.01-lot demo orders and positions.** Run only on a demo account. |

`pwsh scripts/check.ps1` takes `-SkipWeb`, `-SkipSecurity`, `-SkipWhisper` and `-SkipDesktop`. Without the skip flags it needs the
security tools (cargo-deny, cargo-audit, pnpm, gitleaks, semgrep) and counts a missing tool as a failure. A quick Rust-only run:

```powershell
pwsh scripts/check.ps1 -SkipWeb -SkipSecurity -SkipWhisper -SkipDesktop
```

## Project layout

| Path | What it is |
|---|---|
| `crates/api` | `jarvis-api`: HTTP and WebSocket API, login, trading service, voice routes, audit log, `user` and `service` commands |
| `crates/gate` | `jarvis-gate`: limits, intent parser, previews, confirmation tokens, kill switch |
| `crates/llm` | `jarvis-llm`: provider trait with tool calling; Ollama, Anthropic and OpenAI clients |
| `crates/persona` | `jarvis-persona`: persona packs, their validation, wake-phrase matching and the system prompt frame |
| `personas/` | Persona packs, one TOML file each: `jarvis.toml` and `friday.toml` |
| `crates/voice` | `jarvis-voice`: Whisper speech-to-text (feature `whisper`) and Piper text-to-speech |
| `crates/bridge-client` | `jarvis-bridge-client`: wire types, ZeroMQ client, mock bridge, `bridge_cli` and `mock_server` examples |
| `mql5/` | `JarvisBridge` EA, protocol headers, vendored mql-zmq and a libzmq 4.3.5 DLL built from source by `scripts/build-libzmq.ps1` ([THIRD_PARTY.md](mql5/THIRD_PARTY.md)) |
| `web/` | SvelteKit web app: the HUD, login, audit page, PWA |
| `apps/desktop/` | Tauri 2 desktop shell |
| `config/` | `jarvis.example.toml`, the settings template |
| `scripts/` | `setup.ps1`, `run.ps1`, `stop.ps1`, `status.ps1`, `install-autostart.ps1`, `firewall.ps1`, `remote-start.ps1`, `remote-stop.ps1` (with `lib/common.ps1`), `install-ea.ps1`, `compile-ea.ps1`, `build-libzmq.ps1`, `fetch-models.ps1`, `check.ps1` |
| `docs/` | Architecture, runbook, configuration, API, protocol and user docs (see below) |
| `tasks/`, `SPEC.md` | Product spec and sprint plans |
| `.github/` | CI, security, Scorecard, Whisper and release workflows |

## Documentation

| Document | For |
|---|---|
| [SPEC.md](SPEC.md) | The product rules and the boundaries of the project |
| [docs/user-guide.md](docs/user-guide.md) | Traders: what you can ask, how confirmations work, DEMO and LIVE, limits |
| [docs/architecture.md](docs/architecture.md) | Developers and reviewers: components, flows, where each rule is enforced, failure modes |
| [docs/runbook.md](docs/runbook.md) | Operators: start and stop, health, logs, audit queries, users, kill switch, recovery, backups |
| [docs/configuration.md](docs/configuration.md) | Every setting in `jarvis.toml` and every environment variable |
| [docs/symbol-mapping.md](docs/symbol-mapping.md) | Broker symbol names: canonical names, per-broker maps, detection on account switch, overrides and adding a broker |
| [docs/windows-service.md](docs/windows-service.md) | Installing and running the Windows service |
| [docs/vps.md](docs/vps.md) | Running on a Windows VPS: sizing, hardening, TLS, monitoring, LIVE |
| [docs/api.md](docs/api.md) | HTTP and WebSocket contract |
| [docs/protocol.md](docs/protocol.md) | Bridge protocol between the API and the EA |
| [docs/desktop.md](docs/desktop.md) | The desktop app and its TLS setup |
| [docs/glossary.md](docs/glossary.md) | Terms used across the docs |
| [docs/adr/README.md](docs/adr/README.md) | Architecture decision records |
| [tasks/plan-sprints-2-5.md](tasks/plan-sprints-2-5.md) | The plan for Sprints 2 to 5 |
| [CHANGELOG.md](CHANGELOG.md) | Release history |

## Contributing, security and license

- [CONTRIBUTING.md](CONTRIBUTING.md) explains the setup, the ground rules and the pull request checklist. The safety path needs tests and an ADR.
- [SECURITY.md](SECURITY.md) explains how to report a vulnerability privately. Do not open a public issue for one.
- [SUPPORT.md](SUPPORT.md) covers where to ask questions. [GOVERNANCE.md](GOVERNANCE.md) describes decisions and maintainers.
- [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) applies in all project spaces.

Licensed under the [Apache License 2.0](LICENSE). Vendored code and its licences are listed in [mql5/THIRD_PARTY.md](mql5/THIRD_PARTY.md).
Maintained by Ninad K ([@ninad-k](https://github.com/ninad-k)).
