# Runbook

Day-to-day operation of Jarvis MT on a Windows PC or VPS: starting and stopping, health checks, logs, the
audit log, users, the kill switch, recovery and backups. For first-time setup see the [README](../README.md).
For the Windows service see [windows-service.md](windows-service.md). For settings see
[configuration.md](configuration.md).

Run commands from the repository root (for example `D:\Projects\Jarvis-MT`). The config path, `.env`, `data\`,
`logs\` and `models\` are all relative to the working directory, so starting from another folder points the
server at the wrong files.

## Ports

| Port | Component | Bound to | Notes |
|---|---|---|---|
| 8787 | `jarvis-api` (HTTP, WebSocket, web UI) | `server.bind`, `127.0.0.1:8787` by default | Non-loopback needs TLS and login. Expose it only as the server's TLS port. |
| 5555 | `JarvisBridge` EA (ZeroMQ REP) | `127.0.0.1` only | Must match `bridge.endpoint` and the EA input `InpEndpoint`. Never open it on the network. |
| 5173 | Vite dev server | Local | `pnpm -C web dev`. Proxies `/api` to port 8787. |
| 11434 | Ollama | `127.0.0.1` | Used by the `local` provider. Do not expose it. |
| 443 | Reverse proxy (optional) | Public | Terminates TLS in front of a loopback `jarvis-api`. See the [VPS guide](vps.md). |

Only one process can bind a port. If `bind(...) failed` appears in the EA log, or `binding 127.0.0.1:8787 ... is
another jarvis-api running?` appears in the server log, something else holds the port. Stop the other process first.

## Starting

Start in this order: MT5 (logged in, with the EA attached), then Ollama if you use it, then the API, then
the web UI if you use the Vite dev server.

**Development (console, Vite dev server)**

```powershell
cargo run -p jarvis-api
pnpm -C web dev                      # second terminal, http://localhost:5173
```

**Release binary (console)**

```powershell
cargo build --release -p jarvis-api
.\target\release\jarvis-api.exe
```

Build the web UI first if you want the API to serve it (`pnpm -C web build`), and set `static_dir = "web/build"`.
Then open `http://127.0.0.1:8787` (or the `https://` address when TLS is on).

**Windows service**

```powershell
Start-Service JarvisMT               # or: .\target\release\jarvis-api.exe service start (elevated)
```

See [windows-service.md](windows-service.md).

The log of a healthy start contains, in order: `jarvis-api <version> starting`, `bridge reachable` (or a
warning that it is not up yet, which is allowed), and `listening on http://...` or `listening on https://...`.
The server starts even when MT5 is down, and the bridge is checked again on every request.

## Stopping

- **Console:** Ctrl+C. The log shows `Ctrl+C received`, then `shut down cleanly`. Open WebSockets are closed and
  other connections get 10 seconds to finish.
- **Service:** `Stop-Service JarvisMT`.
- **EA:** remove it from the chart (right-click the chart, Expert Advisors, Remove). The Experts log shows
  `JarvisBridge stopped (reason N, M requests served).`
- **Mock EA:** Ctrl+C in its terminal.

Chat history and confirmation tokens are held in memory. A restart clears chat sessions and any pending preview. The
kill switch is saved in the database: if it was engaged when the server stopped, it is engaged again after the restart.

## Health checks

`GET /api/health` needs no login and answers exactly `{"ok":true}` when the server is up. Use it for monitors:

```powershell
Invoke-RestMethod http://127.0.0.1:8787/api/health
```

With TLS on, use the `https://` address. The certificate must be trusted by Windows, for example one made by mkcert.

`GET /api/status` needs a session when login is on, so it is not a monitor target. Use it from the web UI, or after
logging in. It reports the MT5 bridge (`connected`, `latency_ms`, `trade_mode`, `terminal_connected`), the
mode, `mode_mismatch`, the kill switch, voice (`stt`, `tts`), limits and the LLM providers. It never fails because the
bridge is down. It reports that instead.

Expected values in a normal demo session: `"mode": "demo"`, `"mode_mismatch": false`, `"connected": true`,
`"trade_mode": "demo"`, `"kill_switch": {"engaged": false, ...}`.

## Logs

| Where | What | How to read it |
|---|---|---|
| Console | Server log when started with `jarvis-api` | Look at the window. The level comes from `RUST_LOG` in `.env`. |
| `logs\jarvis-api.log.<date>` | Server log when running as the service (one file per day) | Open the newest file. Old files are not deleted, so delete them yourself. |
| Toolbox > Experts (MT5) | EA log | Set the EA input `InpVerbose` to `true` for one line per request. |
| `%APPDATA%\MetaQuotes\Terminal\<id>\MQL5\Logs` | Daily MT5 logs | Useful when the Experts tab has been cleared. |
| `data\jarvis.db`, table `audit` | Structured record of every step | See below. |

Browser developer tools show the web UI console. A Content Security Policy error in that console after a web
rebuild is covered in the troubleshooting table.

## The audit log

Every chat turn, preview, confirmation, cancellation, trade result, kill switch change, login and failed login, and
voice transcription or speech is recorded in the `audit` table of `data\jarvis.db`. The table is append-only: triggers
refuse `UPDATE` and `DELETE`. Audio is never stored, only the text.

Ways to read it:

- **Web UI:** the audit page (`/audit`), which lists the newest entries first.
- **API:** `GET /api/audit?limit=100` (needs a session). Add `&before_id=<id>` to page back.
- **SQL:** see below. The file is in WAL mode, so `jarvis.db-wal` and `jarvis.db-shm` exist while the server runs.
  Do not copy `jarvis.db` alone while the server runs.

Row kinds: `utterance` (what you typed or said), `intent` (a confirm, cancel, order, close, modify or cancel-orders
request; questions and small talk get none), `tool_call`, `bridge_request`, `bridge_reply`, `preview`,
`gate_rejected`, `confirm`, `confirm_second`, `cancel`, `trade_result`, `kill_switch`, `login`, `login_failed`,
`voice_stt`, `voice_tts`, `error`, and the final reply row written at the end of each chat turn.

Query with the `sqlite3` command-line tool if it is installed:

```powershell
sqlite3 data\jarvis.db "SELECT id, ts, session_id, kind, payload FROM audit ORDER BY id DESC LIMIT 20;"
```

Or with Python's standard library, read-only, which needs no install:

```powershell
python -c "import sqlite3; c = sqlite3.connect('file:data/jarvis.db?mode=ro', uri=True); [print(r) for r in c.execute('SELECT id, ts, kind, payload FROM audit ORDER BY id DESC LIMIT 20')]"
```

Useful queries:

```sql
-- Everything that touched trading, newest first
SELECT id, ts, kind, payload FROM audit
 WHERE kind IN ('preview', 'confirm', 'confirm_second', 'cancel', 'trade_result', 'kill_switch')
 ORDER BY id DESC LIMIT 50;

-- One chat session, in order (session_id is in the chat response)
SELECT ts, kind, payload FROM audit WHERE session_id = '<uuid>' ORDER BY id;

-- Trade writes whose outcome is unknown (see "Recovering from outcome_unknown")
SELECT ts, payload FROM audit WHERE kind = 'trade_result' AND payload LIKE '%"outcome":"unknown"%';

-- Failed logins and why (reason is one of unknown_user, bad_password, bad_code, code_reused, locked, rate_limited, malformed)
SELECT ts, payload FROM audit WHERE kind = 'login_failed' ORDER BY id DESC LIMIT 50;

-- Rows per kind
SELECT kind, COUNT(*) FROM audit GROUP BY kind;
```

Passwords and TOTP codes are never written to the log.

## Users

Login accounts are managed only from the machine that runs Jarvis. There is no network endpoint for it. Every
`user` command reads `.env` in the working directory, so run it from the repository root, or from the folder the
service runs from. It also needs `JARVIS_JWT_SECRET` to be set.

```powershell
cargo run -p jarvis-api -- user add <name>          # asks for the password twice, shows a QR code and the key
cargo run -p jarvis-api -- user list                 # name, created time, last login, LOCKED flag
cargo run -p jarvis-api -- user reset-totp <name>    # new authenticator secret (lost phone); ends their sessions, clears a lockout
cargo run -p jarvis-api -- user remove <name>        # asks you to type the name; -y skips the question
```

With the release binary, replace `cargo run -p jarvis-api --` with `.\target\release\jarvis-api.exe`.

Notes:

- Usernames are 1 to 64 characters from `a-z`, `A-Z`, `0-9` and `. _ - @`. They are compared case-insensitively.
- Passwords are at least 12 characters and at most 256 bytes.
- The QR code is printed once. Scan it with an authenticator app, or enter the key by hand. Run `reset-totp` to get a new one.
- `user add` with `--password-stdin` reads the password from the first line of standard input, for scripts. Avoid it
  in shared shell history.
- `remove` ends the user's sessions at once.

## Rotating the JWT secret

Change `JARVIS_JWT_SECRET` only when you have to, for example when `.env` may have leaked.

1. Stop the server.
2. Replace the value in `.env` with a new random value of at least 32 bytes (see [configuration.md](configuration.md)).
3. Start the server. Everyone is logged out, because session tokens are signed with the old key.
4. Run `user reset-totp <name>` for **every** user. Their stored TOTP secrets were encrypted with the old key and cannot be read
   with the new one, so logins fail until each user has a new secret. Give each user the new QR code.

A backup taken before the rotation needs the old `.env` to restore its TOTP secrets. Keep the old `.env` with that backup, and keep it private.

## The kill switch

The kill switch blocks every write at once: new orders, closes, changes and cancellations. It is the first thing to
reach for when something looks wrong.

- **Engage:** use the kill switch control in the web UI, or `POST /api/killswitch` with `{"engaged": true, "reason": "manual"}`.
  Engaging needs only a session. It cancels every pending preview at once.
- **Release:** use the same control. When login is on, the release needs a fresh authenticator code (the request body is
  `{"engaged": false, "totp": "<6 digits>"}`). Each code works once, so wait for the next 30-second code after a failure.
- **It does not close positions.** Close positions in MT5 or with the close controls.
- **It is saved to disk.** A restart, or the service restarting itself after a crash, does not turn trading back on: the
  switch comes back engaged and is only released with a session and a fresh authenticator code. If the saved state cannot
  be read, trading stays off. `[trading] kill_switch_engaged = true` additionally starts the server with it engaged.
- **It is checked again before every single write** in a batch, so a batch stops part way. The response reports `"aborted": "kill_switch"`,
  and the writes that were not sent are listed as not sent.
- The EA-side refusal of writes is separate: turning off Algo Trading in MT5 also stops writes. Use both if you
  are leaving the PC and do not want any trade to happen.

Engaging does not change MT5 state. Check your positions and pending orders in MT5 after an incident.

## Recovering from outcome_unknown

`504 outcome_unknown` means a write was sent to the EA and no reply came back in time. MT5 may or may not have executed it.
The API does not retry it, and the server tells the web UI to refresh positions.

1. **Do not send the same trade again.** Do not retry the request from the chat, the confirm button or a voice command.
2. Check MT5: the Trade tab (open positions and pending orders) and the Account History tab (deals) for the symbol and the time.
   You can also run the read-only commands of the bridge CLI, which do not change anything:
   ```powershell
   cargo run -p jarvis-bridge-client --example bridge_cli -- positions
   cargo run -p jarvis-bridge-client --example bridge_cli -- orders
   cargo run -p jarvis-bridge-client --example bridge_cli -- history --days 1
   ```
3. Read the message. It says which write was unknown, how many earlier writes completed, and that the remaining writes were not sent.
   The audit log has the `trade_result` row with `"outcome":"unknown"` (see the queries above).
4. If the position is not there and you still want it, make a new preview and confirm it. If it is there, manage it as normal.
5. If the EA has stopped answering, follow "EA not answering" below before doing anything else.

## EA re-attach after an MT5 restart

After MT5 restarts, check that JarvisBridge is still attached. The chart shows the smiley face in the top-right corner when
an EA is running. The Experts tab should show the listening line:

```text
JarvisBridge 0.2.0 listening on 127.0.0.1:5555 (libzmq 4.2.1, writes only on request, real accounts refused)
```

With `InpAllowReal` set to `true`, the last part reads `real accounts ALLOWED (InpAllowReal=true)`.

If the line is missing, attach the EA again:

1. Open the chart, then Navigator (Ctrl+N), Expert Advisors > JarvisMT > JarvisBridge, and drag it onto the chart.
2. On the Common tab, tick **Allow DLL imports** and **Allow Algo Trading**. Leave the global option under Tools > Options > Expert Advisors off.
3. Make sure the **Algo Trading** button on the toolbar is on. Writes are refused while it is off.

For a real account, the EA also needs its `InpAllowReal` input set to `true`. Do not set it unless you have decided to trade real money.

If the server is running while the EA is re-attached, the next request reconnects automatically. No restart is needed.

## Updating the models

Voice models are fetched by `scripts/fetch-models.ps1`. The script pins each download to a source revision and a SHA-256 hash,
skips files that already match, and refuses a file whose hash does not match.

```powershell
pwsh scripts/fetch-models.ps1            # install missing or mismatched files
pwsh scripts/fetch-models.ps1 -Force     # download and install again even if they match
```

Models are loaded at server start. After any change, restart the server. Changing a pinned revision or hash is a code change
and needs review.

## Upgrading

1. Stop the server or the service, and take a backup (see below).
2. Update the source: `git pull` on `main`, or the tag you want.
3. Rebuild: `cargo build --release -p jarvis-api`, and `pnpm -C web install` followed by `pnpm -C web build` if `web/` changed.
4. If `mql5/` changed, reinstall and recompile the EA, then re-attach it:
   ```powershell
   pwsh scripts/install-ea.ps1 -DataFolder <32-character-id>
   pwsh scripts/compile-ea.ps1 -DataFolder <32-character-id>
   ```
5. Start the server or the service. Migrations run at startup and are applied once each.
6. Check `/api/health`, then `/api/status`, and place nothing until both look right.
7. Read the [CHANGELOG](../CHANGELOG.md) for the version you moved to.

## Backups

| What | Where | Why |
|---|---|---|
| Database | `data\jarvis.db`, plus `data\jarvis.db-wal` and `data\jarvis.db-shm` if present | The audit log, users (password hashes, encrypted TOTP secrets) and revoked sessions |
| Config | `config\jarvis.toml` | Your limits, symbols and providers |
| Secrets | `.env` | `JARVIS_JWT_SECRET` and provider keys. Needed with the database to read TOTP secrets. |
| Models | `models\` | Optional. Re-download with `fetch-models.ps1`. |
| Logs | `logs\` | Optional. Useful for an incident. |

Take a consistent copy of the database. Either stop the server or the service first, then copy all three database files, or use SQLite's
online backup while the server runs (with the `sqlite3` tool installed):

```powershell
sqlite3 data\jarvis.db ".backup 'D:\backups\jarvis-2026-10-30.db'"
```

Store backups where only the trading user can read them. A copy of `.env` and the database together contains everything needed to log in
to Jarvis, so treat it like the secrets it is. Test a restore on another folder before you rely on it.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `refusing to start: login is enabled but the session key is unusable` | `JARVIS_JWT_SECRET` is missing or shorter than 32 bytes | Set it in `.env` in the working directory (see [configuration.md](configuration.md)). |
| `refusing to start: config mode is 'demo' but the MT5 account reports trade_mode 'Real'` | The terminal is logged into a real account | Log MT5 into the demo account. Do not change `mode` to `live`. |
| `409 mode_mismatch` on chat, `"mode_mismatch": true` in `/api/status` | Same, detected while running | Same fix. The flag clears on the next successful bridge call. |
| `/api/status` shows `"connected": false`, or chat says MT5 is not reachable | The EA is not attached, or MT5 is closed | Start MT5, attach JarvisBridge, and check the listening line. No restart of the server is needed. |
| `503 bridge_unavailable`, "bridge did not reply within 3000 ms" | The EA is attached but not answering | Check the Experts log. Remove and re-attach the EA. Raise `bridge.timeout_ms` only if replies are slow. |
| `unknown symbol: XAUUS` or `unknown symbol` in the watchlist errors | The broker's name differs (`XAUUSD.m`, `XAUUSD_`, `GOLD`, `US100`, `NAS100`) | Find the exact name in Market Watch (Ctrl+M). Set `[symbols] suffix` (for example `".m"`), or an alias under `[symbols.aliases]`. Restart. |
| `writes refused: Algo Trading is disabled in the terminal` or `... trading is not allowed for this EA` | The Algo Trading button is off, or the EA's Allow Algo Trading is not ticked | Turn on the Algo Trading button and tick Allow Algo Trading on the EA's Common tab. |
| `writes refused on a real account (InpAllowReal is false)` | The MT5 account is real and the EA input is off | Expected while on a real account. Set `InpAllowReal` only if you have decided to trade real money. |
| `bind(...) failed` in the EA log | Port 5555 is used by another process | Stop `mock_server` or the other EA. |
| `binding 127.0.0.1:8787 (is another jarvis-api running?)` | Port 8787 is used | Stop the other process, or pick another loopback port. |
| The first chat reply takes about 100 seconds | Ollama is loading `qwen3.5:9b` into memory | Wait. Later replies are faster. Keep Ollama running. |
| `502 llm_error` | The provider call failed. For `local`, Ollama is not running or the model is missing | Start Ollama, then `ollama pull qwen3.5:9b`. For a cloud provider, check the key variable in `.env`. |
| A provider shows `"available": false` | Its key variable (for example `ANTHROPIC_API_KEY`) is not set in the environment | Set it in `.env` and restart. |
| `503 voice_unavailable`, "speech-to-text is not available" | Speech-to-text did not load: the model file is missing, or the build has no `whisper` feature | Run `scripts/fetch-models.ps1`, check `whisper_model`, and check the log line `speech-to-text is off: ...`. Build with `--features whisper` for speech-to-text. |
| `503 voice_unavailable`, "text-to-speech is not available" | Piper did not load: `piper_exe` or `piper_voice` is wrong | Check both paths and the log line `text-to-speech is off: ...`. |
| Voice is off with a log line `voice is off: ...` | The `[voice]` section is incomplete (for example `enabled = true` without model paths) | Fill in the paths in `[voice]`, or set `enabled = false`. |
| `whisper` build fails with `stdio.h not found` or similar C header errors | The build did not run inside the MSVC environment | Open a Developer PowerShell, or run `"<VS Build Tools>\VC\Auxiliary\Build\vcvars64.bat"` first, then build in that same window. |
| Browser console shows `Refused to execute inline script` after `pnpm -C web build` | The server hashes the inline script once at startup and still has the old hash | Restart the API (or the service) after every web rebuild. |
| Login succeeds, but the next request is unauthorized, when the page is opened over `http://` while TLS is on | The session cookie is `Secure` and is not sent over plain HTTP | Open the `https://` address. |
| `401 unauthorized` with "code not accepted" when releasing the kill switch | Wrong, reused or late code | Wait for the next 30-second code and enter it. |
| `401 unauthorized` with "invalid username, password or code" | Wrong password, code, or a locked account | Re-check the code. After 10 failed attempts the account is locked for 15 minutes. `user reset-totp` also clears a lock. |
| `429 rate_limited` on login | More than 5 login attempts in a minute from one client IP | Wait for the `Retry-After` time. Behind a reverse proxy all users share one IP. |
| `429 rate_limited` on confirm | More than 10 confirmations per minute in one session | Wait, then confirm again. Each preview still expires after 15 seconds. |
| `403 origin not allowed` on the live-update connection, behind a reverse proxy | The proxy changed the `Host` header, so the `Origin` does not match | Make the proxy pass `Host $host` (or the public host), and keep the `Upgrade` headers. See the [VPS guide](vps.md). |
| `config file not found at config/jarvis.toml` | Config not copied, or started from another folder | Copy `config\jarvis.example.toml` to `config\jarvis.toml`, start from the repository root, or set `JARVIS_CONFIG`. |
| `install-ea.ps1`: `SHA-256 mismatch` or `has no SHA-256 entry` | A DLL in `mql5\Libraries` changed, or the table in `mql5\THIRD_PARTY.md` does not match | Restore the DLL from Git or the upstream release. Do not edit a hash to match a file you do not recognise. |
| `install-ea.ps1`: `Not an MT5 data folder (no MQL5 subfolder)` | Wrong `-DataFolder`, or the terminal was never run | Open the terminal once, then check the folder under File > Open Data Folder. |
| `compile-ea.ps1` fails with compile errors or with warnings in our code | An error in the EA source | Read the printed MetaEditor lines and fix them. `-AllowWarnings` is for looking at a build, not for shipping. |
| Path errors, or tools that cannot start in a long folder | Windows limits paths to 260 characters without the long-path setting | Keep the repository near the drive root. |

### EA not answering

If writes time out repeatedly (`outcome_unknown`, `bridge_unavailable`):

1. Turn the kill switch on, so nothing else is sent.
2. Read the Experts log for `reply could not be sent` or `rebuilding the socket`. These lines mean the EA is recovering on its own.
3. Remove the EA from the chart and attach it again. Check the listening line.
4. Check each position and pending order in MT5 (see "Recovering from outcome_unknown").
5. Release the kill switch only when the bridge answers `ping` with `connected: true`.

## Development tools that can trade

`bridge_cli` sends commands straight to the EA. Its `buy`, `sell`, `modify`, `close` and `cancel` commands need `--yes`, and they
skip the confirmation gate, the limits and the kill switch. Use them only on a demo account, for testing the EA. Never use them
in place of the chat flow.

```powershell
cargo run -p jarvis-bridge-client --example bridge_cli -- ping
cargo run -p jarvis-bridge-client --example bridge_cli -- price XAUUSD
cargo run -p jarvis-bridge-client --example bridge_cli -- account
```

The `symbol` argument takes the broker's name, not an alias. The `ping` reply reports `trade_mode`. Check it says `demo` before any write.

## Updating the EA's DLLs

The DLLs in `mql5\Libraries` are the x64 builds of libzmq and libsodium from mql-zmq. `scripts/install-ea.ps1` refuses to install a DLL whose
hash does not match the table in `mql5\THIRD_PARTY.md`, so the table and the files change together. [SPEC.md](../SPEC.md) requires approval before
new binaries are committed.

1. Replace the files in `mql5\Libraries\` with the new x64 builds.
2. Compute the new hashes: `Get-FileHash mql5\Libraries\*.dll -Algorithm SHA256`.
3. Update the table in `mql5\THIRD_PARTY.md`: version and SHA-256 for each DLL, keeping the format. The install script reads it.
4. If the mql-zmq headers are re-vendored, re-apply the local patch listed in `THIRD_PARTY.md`.
5. Reinstall, recompile and check:
   ```powershell
   pwsh scripts/install-ea.ps1 -DataFolder <32-character-id>
   pwsh scripts/compile-ea.ps1 -DataFolder <32-character-id>
   cargo run -p jarvis-bridge-client --example bridge_cli -- ping
   ```
6. Re-attach the EA, then run the live tests (see the README) on a demo account.

## Related docs

- [Configuration](configuration.md): every setting and environment variable.
- [Windows service](windows-service.md): install, run and update the service.
- [VPS guide](vps.md): running on a server next to MT5.
- [HTTP API](api.md) and [bridge protocol](protocol.md): the wire contracts.
- [Architecture](architecture.md): how the parts fit together.
