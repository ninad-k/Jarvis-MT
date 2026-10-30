# Jarvis MT

Jarvis MT is a voice and chat trading assistant for MetaTrader 5 (MT5). You type a question such as "what's gold at?" or "show my positions", and an LLM answers from live data in your MT5 terminal, read through a Rust backend and a small MQL5 Expert Advisor (EA). It is built for one trader running MT5 on a Windows PC or VPS.

Sprint 1 is read-only. Voice and trading arrive in later sprints.

## Safety model

- The LLM can only call read tools. From Sprint 2 it can also call `propose_*` tools, which create a preview and never send an order.
- A trade reaches MT5 only after the user confirms a preview with a short-lived token (15 seconds, Sprint 2).
- Demo is the default. The API refuses to start in demo mode when the bridge reports a real account. If the account switches to real while the API runs, chat returns `409 mode_mismatch`.
- The bridge listens on `127.0.0.1` only. The API binds to loopback only until authentication arrives in Sprint 4.
- Account login, name and server never reach the LLM or the browser.

The full rules are in [SPEC.md](SPEC.md).

## Status: Sprint 1

Implemented:

- Read-only chat over live MT5 data: price, account, open positions, pending orders and deal history.
- A web HUD with an account strip, watchlist, positions, chat transcript and bridge status.
- An append-only SQLite audit log of every chat step.

Not in Sprint 1: trading, voice, authentication. The plan and demo script are in [tasks/plan.md](tasks/plan.md). The task list is in [tasks/todo.md](tasks/todo.md).

## Repo layout

| Path | What it is |
|---|---|
| `crates/api` | `jarvis-api`: the HTTP API, read tools, agent loop, config and audit log |
| `crates/bridge-client` | `jarvis-bridge-client`: protocol types, ZeroMQ client, mock EA, `bridge_cli` and `mock_server` examples |
| `crates/llm` | `jarvis-llm`: LLM provider trait and Anthropic, OpenAI and Ollama clients |
| `mql5/` | `JarvisBridge` EA, JSON and protocol headers, vendored mql-zmq (see [THIRD_PARTY.md](mql5/THIRD_PARTY.md)) |
| `web/` | SvelteKit web app (the HUD) |
| `config/` | `jarvis.example.toml`: non-secret settings, symbol aliases, LLM providers |
| `scripts/` | `install-ea.ps1` and `compile-ea.ps1` |
| `docs/` | [architecture](docs/architecture.md), [runbook](docs/runbook.md), [HTTP API](docs/api.md), [bridge protocol](docs/protocol.md) |
| `tasks/`, `SPEC.md` | Sprint plan, task list and product spec |
| `.env.example` | Template for secrets (LLM API keys) |

## Windows setup

### Prerequisites

1. **Rust** (stable) via [rustup](https://rustup.rs). The repo's `rust-toolchain.toml` selects stable with rustfmt and clippy. The crates use edition 2024, so you need Rust 1.85 or newer.
2. **Node.js 22 or newer** and **pnpm**.
3. **MetaTrader 5** with a **demo** account, logged in. Open the terminal at least once so its data folder has an `MQL5` subfolder.
4. **PowerShell 7** (`pwsh`) for the two scripts. Windows PowerShell 5.1 also works if you replace `pwsh` with `powershell`.
5. **An LLM**, either:
   - **Ollama** with the default model: `ollama pull qwen3.5:9b`. This is the default `local` provider.
   - or an **Anthropic** or **OpenAI** API key. Put it in `.env` and pick that provider in `config/jarvis.toml` (`llm.default`) or in the web UI.

### Steps

1. **Create your local config.** From the repo root:

   ```powershell
   Copy-Item config\jarvis.example.toml config\jarvis.toml
   Copy-Item .env.example .env
   ```

   Skip the first line if `config\jarvis.toml` already exists. Set `[symbols] suffix` and the aliases to your broker's symbol names. Put API keys in `.env` only. Both files are gitignored, so keep them out of commits.

2. **Install the EA into your MT5 data folder.**

   ```powershell
   pwsh scripts/install-ea.ps1
   ```

   The script's default `-DataFolder` points at one specific terminal. For any other terminal, pass its data folder ID: the 32-character folder name under `%APPDATA%\MetaQuotes\Terminal`, which MT5 shows under File > Open Data Folder.

   ```powershell
   pwsh scripts/install-ea.ps1 -DataFolder <32-character-id>
   ```

   The script checks the DLL SHA-256 hashes in `mql5/THIRD_PARTY.md` before and after copying. It only adds or overwrites its own files.

3. **Compile the EA.**

   ```powershell
   pwsh scripts/compile-ea.ps1
   ```

   The script runs MetaEditor on the installed copy and fails on any compile error or on a warning in our code. If MT5 is not in `C:\Program Files\MetaTrader 5`, pass `-MetaEditor "<path to MetaEditor64.exe>"`. Use the same `-DataFolder` as in step 2. A successful run ends with `OK: ...\JarvisBridge.ex5`.

4. **Run the self-test in MT5.** In the Navigator (Ctrl+N), open Scripts > JarvisMT > JarvisSelfTest and run it on a chart. In the Experts tab of the Toolbox (Ctrl+T), look for `=== JarvisSelfTest done: <N> passed, 0 failed ===`.

5. **Attach JarvisBridge with DLL imports enabled for this EA only.** In the Navigator, open Expert Advisors > JarvisMT > JarvisBridge (right-click > Refresh if it is missing) and drag it onto a chart. On the Common tab, tick **Allow DLL imports**. Leave the global option under Tools > Options > Expert Advisors off. The Algo Trading button does not matter, because this EA version is read-only.

   The Experts tab should show:

   ```
   JarvisBridge 0.1.0 listening on 127.0.0.1:5555 (libzmq 4.2.1, read-only)
   ```

6. **Check the bridge.**

   ```powershell
   cargo run -p jarvis-bridge-client --example bridge_cli -- ping
   ```

   The reply is JSON. Check that `"trade_mode"` is `"demo"` and `"protocol_version"` is `1`. Then try `cargo run -p jarvis-bridge-client --example bridge_cli -- price XAUUSD`, using your broker's symbol name. `bridge_cli` does not resolve aliases.

7. **Start the API** from the repo root, because the config path is relative to the working directory:

   ```powershell
   cargo run -p jarvis-api
   ```

   It serves `http://127.0.0.1:8787`. To check it, run `Invoke-RestMethod http://127.0.0.1:8787/api/status`. If the EA is not attached yet, the API still starts and reports `"connected": false`.

8. **Start the web UI** in a second terminal:

   ```powershell
   pnpm -C web install
   pnpm -C web dev
   ```

   Open http://localhost:5173. Vite proxies `/api` to port 8787, so keep the API running.

9. **Try it.** Ask "price of gold", "what's my equity", "list my open positions" and "what did I trade yesterday". Then ask "buy gold". The assistant should say it cannot place trades yet. Every step is written to the audit table (see the [runbook](docs/runbook.md#logs-and-the-audit-database)).

## Develop without MT5

- **Full stack on canned data.** Start the mock EA on the bridge port, then the API and the web UI as above:

  ```powershell
  cargo run -p jarvis-bridge-client --features mock --example mock_server
  ```

  It listens on `tcp://127.0.0.1:5555` until you press Ctrl+C. Stop it before you attach the real EA, because both use port 5555. The mock serves canned prices for XAUUSD, US100, US30, EURUSD, GBPUSD and BTCUSD and reports a demo account. Chat still needs an LLM.

- **UI only.** `pnpm -C web dev:mock` runs the web app with no backend. Every API call is answered from `web/src/lib/mock.ts`.

## Tests

| Command | Needs | Checks |
|---|---|---|
| `cargo test --workspace` | Nothing | Unit and contract tests: protocol fixtures, mock bridge, agent loop with a fake LLM, provider request mapping |
| `cargo clippy --workspace --all-targets -- -D warnings` | Nothing | Lints, with warnings treated as errors |
| `cargo fmt --all --check` | Nothing | Formatting |
| `pwsh scripts/compile-ea.ps1` | MT5 installed | The EA compiles with zero errors |
| `cargo test -p jarvis-bridge-client --features mock -- --ignored --test-threads=1` | Demo terminal with the EA attached | Live tests in `crates/bridge-client/tests/live.rs` against the real EA. They only read. |
| `pnpm -C web check` | `pnpm -C web install` | Svelte and TypeScript checks |

For the live tests, set `JARVIS_LIVE_SYMBOL` to a symbol name your broker uses (default `EURUSD`), for example in PowerShell: `$env:JARVIS_LIVE_SYMBOL = "XAUUSD.m"`. `JARVIS_BRIDGE_ENDPOINT` overrides the default `tcp://127.0.0.1:5555`.

## Documentation

- [Architecture](docs/architecture.md): components, the request flow of one chat turn, design decisions and safety rules by sprint.
- [Runbook](docs/runbook.md): starting and stopping, ports, logs, the audit database, troubleshooting and DLL updates.
- [HTTP API](docs/api.md) and [bridge protocol](docs/protocol.md): the wire contracts.
- [SPEC.md](SPEC.md) and [tasks/plan.md](tasks/plan.md): the product spec and the Sprint 1 plan.

## License

Apache-2.0. See [LICENSE](LICENSE).
