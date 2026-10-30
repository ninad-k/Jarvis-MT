# Runbook (Sprint 1)

How to run, check and fix the Sprint 1 stack on a Windows PC. Setup steps are in the [README](../README.md). Sprint 1 is read-only: nothing here places or changes a trade.

Run all commands from the repo root (`D:\Projects\Jarvis-MT` in this checkout). The config path and the audit database path are relative to the working directory.

## Ports

| Port | Component | Started by | Notes |
|---|---|---|---|
| 5555 | `JarvisBridge` EA (ZeroMQ REP) | Attaching the EA in MT5, or `mock_server` | `tcp://127.0.0.1:5555`. Change `bridge.endpoint` in the config and the EA input `InpEndpoint` together. The EA refuses anything but loopback. |
| 8787 | `jarvis-api` (HTTP) | `cargo run -p jarvis-api` | `server.bind` in the config. Loopback only. |
| 5173 | Vite dev server (web) | `pnpm -C web dev` | Vite's default port. The API's CORS allow-list covers `localhost:5173` and `127.0.0.1:5173`. |
| 11434 | Ollama | The Ollama app | `llm.providers` entry `local`, `base_url`. |

Only one process can bind port 5555. Stop `mock_server` before you attach the real EA. If `bind(...) failed` appears in the Experts log, another process has the port.

## Starting

Start in this order:

1. **MT5.** Open the demo terminal and check that it is logged into a demo account.
2. **JarvisBridge.** Attach it if it is not already on a chart, then check the Experts log for the listening line (see below).
3. **Ollama**, if you use the `local` provider. Check the model is present with `ollama list`. Pull it once with `ollama pull qwen3.5:9b`.
4. **API.**

   ```powershell
   cargo run -p jarvis-api
   ```

   The log should show `jarvis-api 0.1.0 starting`, then `bridge reachable` (or a warning if the EA is not up), then `listening on http://127.0.0.1:8787`. The API starts even if MT5 is down.

5. **Web UI.**

   ```powershell
   pnpm -C web dev
   ```

Check the stack:

```powershell
Invoke-RestMethod http://127.0.0.1:8787/api/status | ConvertTo-Json -Depth 5
```

Expect `"connected": true` and `"trade_mode": "demo"` under `bridge`, and `"mode_mismatch": false` at the top level. Under `llm`, the `local` provider should show `"available": true`. The API does not probe providers over the network, so `available` only says whether the provider could be built from its config (for cloud providers, whether the API key variable is set).

## Stopping

- **Web UI:** press Ctrl+C in its terminal.
- **API:** press Ctrl+C. The log shows `Ctrl+C received, shutting down`, then `shut down cleanly`. Chat sessions are kept in memory, so they are lost on restart.
- **EA:** right-click the chart and remove the Expert Advisor (Expert Advisors > Remove). The Experts log shows `JarvisBridge stopped (reason N, M requests served).`
- **mock_server:** press Ctrl+C.
- **Ollama:** quit it from the tray icon if you want to free memory.

## The JarvisBridge EA

### Expected Experts log lines

Open the Toolbox (Ctrl+T) and the Experts tab.

| Line | Meaning |
|---|---|
| `JarvisBridge 0.1.0 listening on 127.0.0.1:5555 (libzmq 4.2.1, read-only)` | Started. The libzmq version is the one in the loaded DLL. |
| `JarvisBridge: 'Allow DLL imports' is required for this EA (libzmq.dll).` | DLL imports are off for this EA. Init fails. See below. |
| `JarvisBridge: refusing InpEndpoint '...': only tcp://127.0.0.1:<port> is allowed (localhost only).` | `InpEndpoint` is not a loopback address. Init fails. |
| `JarvisBridge: bind(...) failed: ...` | The port is in use, usually by `mock_server` or another copy of the EA. |
| `JarvisBridge: socket rebuilt, listening on ...` | The socket was recreated after a failed reply. This is normal. |

For one line per request, set the EA input `InpVerbose` to `true`. It is off by default. Each line shows `cmd`, `id`, `ok`, `retcode` and the time taken.

### Enabling DLL imports for this EA only

`JarvisBridge` needs `libzmq.dll`, so DLL imports must be allowed for it.

1. In the Navigator (Ctrl+N), open Expert Advisors > JarvisMT > JarvisBridge and drag it onto a chart. If the EA is already attached, remove it first.
2. In the properties window, open the **Common** tab and tick **Allow DLL imports**.
3. Click OK.

Do not turn on the global option under Tools > Options > Expert Advisors. That would allow DLL imports for every EA. The Algo Trading button does not affect this EA.

### JarvisSelfTest

`Scripts > JarvisMT > JarvisSelfTest` runs canned requests through the EA's request dispatcher. It uses no ZeroMQ and no DLLs, so it does not need DLL imports. It prints `PASS` or `FAIL` for each check in the Experts log, and prints live terminal data (ping, account, positions) so you can compare it with the terminal. The last line is `=== JarvisSelfTest done: <N> passed, <M> failed ===`. Run it after compiling or upgrading the EA.

## Logs and the audit database

**Where the logs are**

- **API:** the console where `cargo run -p jarvis-api` is running. There is no log file. The level comes from `RUST_LOG` in `.env`. The default is `info,jarvis_api=debug,jarvis_bridge_client=debug`.
- **Web UI:** the console of `pnpm -C web dev`, and the browser's developer tools console.
- **EA:** the Experts tab in the MT5 Toolbox. MetaTrader also keeps daily Experts logs in `<data folder>\MQL5\Logs`.

**Where the audit database is**

- The path is `data\jarvis.db`, relative to the working directory. Started from the repo root, that is `D:\Projects\Jarvis-MT\data\jarvis.db`. The `data/` folder is gitignored.
- The database runs in WAL mode. While the API runs, `jarvis.db-wal` and `jarvis.db-shm` sit next to it. Do not copy `jarvis.db` alone while the API is running.
- The table is `audit(id, ts, session_id, kind, payload)`. `ts` is UTC, and `payload` is JSON text. The kinds are `utterance`, `tool_call`, `bridge_request`, `bridge_reply`, `assistant_reply` and `error`.
- The table is append-only. Triggers abort any `UPDATE` or `DELETE`. Do not try to change it.

**How to query it**

The `sqlite3` command-line tool is not installed on this machine. Either install it from sqlite.org, or use the Python one-liner, which opens the file read-only and needs no install:

```powershell
python -c "import sqlite3; c = sqlite3.connect('file:data/jarvis.db?mode=ro', uri=True); [print(r) for r in c.execute('SELECT id, ts, session_id, kind, payload FROM audit ORDER BY id DESC LIMIT 20')]"
```

With `sqlite3`, the same SQL works from the repo root:

```powershell
sqlite3 data\jarvis.db "SELECT id, ts, session_id, kind, payload FROM audit ORDER BY id DESC LIMIT 20;"
```

Other useful queries:

```sql
-- Every step of one chat session (use the session_id from the chat response)
SELECT ts, kind, payload FROM audit WHERE session_id = '<uuid>' ORDER BY id;

-- Tool calls that failed
SELECT ts, payload FROM audit WHERE kind = 'tool_call' AND payload LIKE '%"ok":false%';

-- Row count per kind
SELECT kind, COUNT(*) FROM audit GROUP BY kind;
```

## Configuration

- **Which file:** `config/jarvis.toml`, or the file named by the `JARVIS_CONFIG` environment variable. The default path is relative to the working directory, so start from the repo root or set `JARVIS_CONFIG`.
- **When it is read:** at startup only. After you edit it, restart the API.
- **Missing file:** the API stops with `config file not found at ...` and tells you to copy `config/jarvis.example.toml`.
- **Secrets:** `.env` in the repo root. The API loads it at startup. It holds `ANTHROPIC_API_KEY` and `OPENAI_API_KEY`, and `RUST_LOG` for the log level.
- **Symbol names:** `[symbols] suffix` is appended to every resolved name, for example `.m`. `[symbols.aliases]` maps spoken names to broker symbols.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `/api/status` shows `"connected": false`. Chat answers say MT5 is not reachable. | The EA is not attached, or MT5 is closed. A refused connection is reported after about 2 s on Windows. | Attach `JarvisBridge` and check for the listening line. The API keeps retrying on each call. No restart is needed. |
| `503 bridge_unavailable`, "bridge did not reply within 3000 ms" | The EA is attached but not answering. The timeout is `bridge.timeout_ms`. | Check the Experts log for errors. Remove and re-attach the EA. Raise `bridge.timeout_ms` only if replies are slow. |
| `unknown symbol: XAUUS` (the chat tool reports it, and `/api/watchlist` lists it under `errors`) | The broker's symbol name differs, for example `XAUUSD.m`, `GOLD` or `NAS100`. | Find the exact name in Market Watch (Ctrl+M). Set `[symbols] suffix` or add an entry under `[symbols.aliases]` in `config/jarvis.toml`. Restart the API. |
| `409 mode_mismatch` on chat, and `"mode_mismatch": true` in `/api/status` | The config has `mode = "demo"`, but the terminal is logged into a real account. | Log the terminal into the demo account. Do not switch `mode` to `live` to get past this. The flag clears on the next successful bridge call after the account is fixed. |
| API exits at startup: `refusing to start: config mode is 'demo' but the MT5 account reports trade_mode 'Real'` | The same mismatch, detected at startup while the bridge is up. | Same as above. |
| The first reply from the `local` provider takes about 100 s | Ollama is loading `qwen3.5:9b` into memory. Later replies take about 2 s. | Wait for the first reply, and keep Ollama running. The provider's `timeout_secs` is 180, and the web UI also gives up after 180 s. |
| `502 llm_error` on chat | The provider call failed. For `local`, Ollama is not running or the model is not pulled. | Start Ollama, then run `ollama pull qwen3.5:9b`. |
| A cloud provider shows `"available": false` | Its API key variable is not set (`ANTHROPIC_API_KEY` or `OPENAI_API_KEY`). | Set the key in `.env` and restart the API. |
| EA init fails with `'Allow DLL imports' is required` | DLL imports are off for this EA. | Enable them on the EA's Common tab and re-attach (see above). Do not enable them globally. |
| EA does not start, with `bind(...) failed` | Another process has port 5555. | Stop `mock_server` or the other EA instance. |
| `config file not found at config/jarvis.toml` | The config was not copied, or the API was started from another folder. | Copy `config/jarvis.example.toml` to `config/jarvis.toml`, start from the repo root, or set `JARVIS_CONFIG`. |
| `binding 127.0.0.1:8787 (is another jarvis-api running?)` | Port 8787 is in use. | Stop the other process, or change `server.bind` to another loopback port. |
| `install-ea.ps1`: `SHA-256 mismatch` or `has no SHA-256 entry` | A DLL in `mql5/Libraries` changed, or the table in `THIRD_PARTY.md` does not match. | Restore the DLL from git or from the upstream release. Do not edit the hash to match a file you do not recognize. |
| `install-ea.ps1`: `Not an MT5 data folder (no MQL5 subfolder)` | The `-DataFolder` value is wrong, or the terminal has never been run. | Open the terminal once. Check the folder ID under File > Open Data Folder and pass it with `-DataFolder`. |
| `compile-ea.ps1` fails with `FAILED: compile errors` or `warnings in our code` | A compile error, or a warning in our own code (warnings fail the run unless `-AllowWarnings` is passed). | Read the printed lines, which are the MetaEditor errors and warnings, and fix them. Use `-AllowWarnings` only to look at a build, not as a fix. |
| `Source not found ... run scripts\install-ea.ps1 first` | The EA was never installed into this data folder. | Run `install-ea.ps1` with the right `-DataFolder`. |
| Path errors such as "path too long", or tools that cannot start in a long folder | Windows limits most tools to 260-character paths. This can hit the repo, the Cargo target folder or a long MT5 data path. Some shells also refuse to start in a working directory with a path that is too long, which happened with a scratch folder during this check. | Keep the repo near the drive root, as `D:\Projects\Jarvis-MT` is. If a tool still fails, run it from a shorter folder. |
| Requests to `/api/...` fail in the browser | The API is not running on 8787, so the Vite proxy has nothing to forward to. | Start `cargo run -p jarvis-api`. |

## Updating the EA's DLLs

The DLLs in `mql5/Libraries` are the prebuilt x64 libzmq 4.2.1 and libsodium 1.0.11 binaries from mql-zmq. `scripts/install-ea.ps1` refuses to install any DLL whose hash does not match the table in `mql5/THIRD_PARTY.md`, so the table and the files must change together. SPEC.md says to ask first before committing binaries, so get approval before committing new DLLs.

1. Replace the DLL files in `mql5\Libraries\` with the new x64 builds. The libzmq note in `THIRD_PARTY.md` recommends a current libzmq release before any use beyond localhost.
2. Compute the new hashes:

   ```powershell
   Get-FileHash mql5\Libraries\*.dll -Algorithm SHA256
   ```

3. Update the table in `mql5/THIRD_PARTY.md`: the version and the SHA-256 for each DLL. Keep the table format, with the DLL name in the first column and the 64-character hash in the last column. The install script parses those cells.
4. If the mql-zmq headers are re-vendored instead, re-apply the local `char` to `uchar` patch listed in `THIRD_PARTY.md`.
5. Reinstall and recompile, then check the EA:

   ```powershell
   pwsh scripts/install-ea.ps1
   pwsh scripts/compile-ea.ps1
   cargo run -p jarvis-bridge-client --example bridge_cli -- ping
   ```

   Re-attach the EA, then run the live tests (`cargo test -p jarvis-bridge-client --features mock -- --ignored --test-threads=1`).
