# Runbook

Day-to-day operation of Arc MT on a Windows PC or VPS: starting and stopping, health checks, logs, the
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

## Scripts

The scripts in `scripts\` cover installing, starting, stopping and checking Arc MT on Windows. They run in Windows PowerShell 5.1
(`powershell`, or `pwsh`), from any folder: each one finds the repository from its own location. They share `scripts\lib\common.ps1` and the same habits:
coloured step output, preflight checks before anything starts, `-WhatIf` wherever they change state, no PID files (processes are found by
command line or path, and only the ones that belong to this repository), and no secret is ever printed.

| Script | What it does |
|---|---|
| `setup.ps1` | First-time setup, safe to repeat. Checks Rust, Node, pnpm; copies `config\jarvis.toml` and `.env` from the examples only when missing; fills an empty `JARVIS_JWT_SECRET`; fetches the voice models; builds the web UI and the release binary; installs and compiles the EA; reports Ollama. |
| `run.ps1` | Starts the API (and the Vite dev server with `-Dev`, Ollama with `-StartOllama`) attached to the console. Ctrl+C, or any essential child dying, stops everything. |
| `stop.ps1` | Stops the Arc MT processes of this repository: the API, the Vite dev server and the remote tunnel. The fallback when `run.ps1` did not clean up. |
| `status.ps1` | Fast health check: API, processes, ports, Ollama, the Windows service, the remote tunnel. Exit code 0 when the API is healthy. `-Json` for monitors. |
| `install-autostart.ps1` | Registers a Task Scheduler task "Arc MT" that runs `run.ps1` hidden at logon. |
| `firewall.ps1` | Elevated. An allow rule for the API port restricted to the addresses you list, and a block rule for the bridge port 5555. |
| `remote-start.ps1`, `remote-stop.ps1` | Phone access through a Cloudflare quick tunnel to the loopback API, with safety checks. |
| `install-ea.ps1`, `compile-ea.ps1`, `fetch-models.ps1`, `build-libzmq.ps1`, `check.ps1` | EA install and compile (and the bridge key), voice models, the libzmq build, and every local check. |

**Common commands**

```powershell
scripts\setup.ps1 -WhatIf              # preview the first-time setup
scripts\setup.ps1                      # run it (add -DataFolder <id> to install the EA, -Whisper for speech-to-text)
scripts\run.ps1                        # start; http://127.0.0.1:8787; Ctrl+C stops everything
scripts\run.ps1 -Dev -StartOllama      # hot-reload UI on http://127.0.0.1:5173, and ollama serve as a child
scripts\status.ps1                     # is it healthy? (exit code 0 = yes)
scripts\status.ps1 -Json               # the same, for a monitor
scripts\stop.ps1 -WhatIf               # what would be stopped?
scripts\stop.ps1                       # stop this repository's API, dev server and tunnel
```

**`run.ps1`**

- It refuses to start while another `jarvis-api.exe` is running or the Windows service is running (two servers would fight over the port, the database and the EA). This check cannot be skipped.
- The preflight (skip with `-SkipChecks`) checks the config and `.env`, the JWT secret (read to check it, never printed), the bridge key, the ports, `web\build` and the `static_dir` setting, the binary, and Ollama. A missing release binary falls back to `cargo run --release -p jarvis-api`, then to a debug build.
- `-Config <path>` uses another config file (otherwise `JARVIS_CONFIG`, then `config\jarvis.toml`). `-NoBrowser` does not open the browser.
- Logs go to `logs\api-<date>.log` (the API's own log, plus an empty `.err` unless it crashes), `logs\web-<date>.log` with `-Dev`, and `logs\ollama-<date>.log` when it started Ollama. `logs\` is gitignored.
- With login on, `/api/status` needs a session, so the start-up summary shows the config's mode and persona and asks the EA directly only if you built the `bridge_cli` example (`cargo build --release -p jarvis-bridge-client --example bridge_cli`). With login off it prints the live numbers.

**`stop.ps1`** never stops Ollama (unless `-IncludeOllama`), the Windows service (unless `-IncludeService`, from an elevated prompt; it runs `jarvis-api service stop`), MetaTrader 5, or anything of another project. If the service is installed and running it says so.

**`install-autostart.ps1`** versus the Windows service: use the logon task on a PC you work at (it runs in your session next to MT5 and Ollama's tray app, with your own file rights), and the service on a VPS or a box that must come up unattended
(it needs file grants for its virtual account). Choose one: the script refuses to register the task while the service is installed (`-Force` overrides). See [windows-service.md](windows-service.md). Options: `-Remove`, `-NoOllama`, `-Config <path>`, `-WhatIf`.

**`firewall.ps1`** needs an elevated prompt to change anything (a `-WhatIf` preview works without one). Examples:

```powershell
scripts\firewall.ps1 -RemoteAddress 203.0.113.7,198.51.100.0/24 -WhatIf
scripts\firewall.ps1 -RemoteAddress 203.0.113.7 -Ports 443       # the port a reverse proxy listens on
scripts\firewall.ps1 -Remove
```

`-RemoteAddress` is required and "Any" is refused. Other enabled inbound allow rules that already open the same ports are listed, never changed.

**`remote-start.ps1`** refuses to open a public tunnel unless login is on (in the config and in the running API), `[auth] cookie_secure = true` is set (`-SetCookieSecure` writes it, with a backup in `.local\`, and then you restart the API), at least one user exists, the API is running and `cloudflared` is installed.
It prints the `https://....trycloudflare.com` address, saves it to `.local\remote-url.txt`, and stays attached; Ctrl+C or `remote-stop.ps1` closes it. The address is public and protected only by the password and the authenticator code. If it leaks, engage the kill switch, then close the tunnel.
See [vps.md](vps.md) for the caveats.

## Starting

Start in this order: MT5 (logged in, with the EA attached), then Ollama if you use it, then the API, then
the web UI if you use the Vite dev server.

**With the scripts (console)**

```powershell
scripts\run.ps1                       # API + web UI from web\build; add -Dev for the Vite dev server
```

See [Scripts](#scripts). The manual commands below do the same thing.

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
  other connections get 10 seconds to finish. When the stack was started with `scripts\run.ps1`, Ctrl+C in its window stops
  every child it started.
- **Stray processes:** `scripts\stop.ps1 -WhatIf` lists, and `scripts\stop.ps1` stops, the API, Vite dev server and remote tunnel of this
  repository (see [Scripts](#scripts)).
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

`scripts\status.ps1` does the health check, plus processes, ports, Ollama, the service and the tunnel, and exits with code 0 only when the
API is healthy (`-Json` prints a document instead). It reads `/api/status` when it can (login off); with login on only the health answer is available to a script.

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
cargo run -p jarvis-api -- user list                 # name, created time, last login, LOCKED flag (kill-switch release lock only)
cargo run -p jarvis-api -- user reset-totp <name>    # new authenticator secret (lost phone); ends their sessions, clears the kill-switch release lock
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

## The bridge key

The EA answers only a program that holds the shared bridge key (finding BR-1, [ADR-0016](adr/0016-authenticated-bridge-protocol.md)).
Without it, any program on the PC could send `order_send` straight to the EA and skip the gate, the limits, the confirmation and the audit log.

### First setup

1. Run `pwsh scripts\install-ea.ps1` (add `-DataFolder <id>` if you have several terminals). If `MQL5\Files\JarvisMT\bridge.key` does not exist yet, the script creates it:
   32 random bytes from the system's cryptographic generator, as 64 hex characters. It prints the path and never the key.
   An existing key is never changed, and a damaged one stops the script.
2. In `config\jarvis.toml` set `key_file` under `[bridge]` to the printed path (a TOML literal string, single quotes), or put the key in `.env` as `JARVIS_BRIDGE_KEY`.
3. Compile and attach the EA (see the install script's next steps). The Experts log must say `JarvisBridge 0.4.0 listening on ... (protocol 2, authenticated, ...)`.
   An EA without a usable key does not start; the log says `cannot open the key file` or `the key file ... is not usable`.
4. Start the API. `bridge_cli --key-file <path> ping` checks the link on its own.

The key lives in the terminal's file sandbox (`MQL5\Files`), which is inside your Windows profile, so other Windows users cannot read it.
If the MT5 data folder is elsewhere (a portable install, another drive), make sure only your own account can read the file.
Do not copy the key into the repository, an issue, a chat or a log. The API and the EA never log it.

### Rotating the bridge key

Rotate the key when it may have been exposed (for example, the file was copied to a shared place) and when someone else had access to the PC.

1. Turn the kill switch on and stop the API (or the service).
2. Run `pwsh scripts\install-ea.ps1 -RotateKey`. It replaces `bridge.key` with a new random key. If you use `JARVIS_BRIDGE_KEY`, put the new key (read it from the file) in `.env`.
3. Remove the EA from the chart and attach it again. It reads the key only when it starts. Until then the EA still uses the old key and refuses the API.
4. Start the API and check `/api/status`. Release the kill switch.

### When requests are refused

A refused request is answered `retcode -4`, "authentication failed". The API shows it as a bridge error that mentions the key and the clock. The Experts log of the EA says why, in one line per 10 seconds at most:

| Experts log | Meaning |
|---|---|
| `MAC does not match (wrong key or altered message)` | The API and the EA use different keys (one was rotated, or `key_file` points at another terminal's file), or another program is sending garbage. |
| `timestamp more than 30 s away from this PC's clock` | The clock of the PC that runs the API differs from the EA's by more than 30 seconds. Both are normally the same PC; check Windows time sync. |
| `request id already used in the last 60 s` | A message was sent twice. The API never does this; the cause is another program replaying captured traffic. |
| `not a two-frame message` | The sender does not speak protocol 2: an old API build, or another program. |

If the API itself reports `reply authentication failed`, the reply was not signed with its key: the keys differ, or a program other than the EA answers on port 5555. Check which process owns the port (`Get-NetTCPConnection -LocalPort 5555`).

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
JarvisBridge 0.4.0 listening on 127.0.0.1:5555 (protocol 2, authenticated, libzmq 4.3.5, writes only on request, real accounts refused, max order 1 lots)
```

With `InpAllowReal` set to `true`, the real-accounts part reads `real accounts ALLOWED (InpAllowReal=true)`. The last part shows the order cap (`InpMaxLot`).

EA inputs (set on the Inputs tab when attaching; changing one needs the EA to be re-attached):

| Input | Default | Meaning |
|---|---|---|
| `InpEndpoint` | `tcp://127.0.0.1:5555` | ZeroMQ endpoint. Must be `tcp://127.0.0.1:<port>` and match `bridge.endpoint`. |
| `InpVerbose` | `false` | Log one line per request. |
| `InpAllowReal` | `false` | Allow writes on a real account. |
| `InpMaxLot` | `1.0` | Largest volume of one `order_send`, in lots; `0` turns the cap off. A larger order is refused with `volume exceeds the EA's InpMaxLot (...)` (retcode -1). Closing positions is never capped. This is a second line of defence: keep `[limits] max_lot` in the API config at or below it. |

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
| `refusing to start: login is enabled but the session key is unusable` | `JARVIS_JWT_SECRET` is missing, shorter than 32 bytes, or guessable (fewer than 16 different byte values, or a repeated block) | Generate one with the command in the message (the same as in `.env.example`) and set it in `.env` in the working directory (see [configuration.md](configuration.md)). |
| `refusing to start: config mode is 'demo' but the MT5 account reports trade_mode 'Real'` | The terminal is logged into a real account | Log MT5 into the demo account. Do not change `mode` to `live`. |
| `409 mode_mismatch` on chat, `"mode_mismatch": true` in `/api/status` | Same, detected while running | Same fix. The flag clears on the next successful bridge call. |
| `/api/status` shows `"connected": false`, or chat says MT5 is not reachable | The EA is not attached, or MT5 is closed | Start MT5, attach JarvisBridge, and check the listening line. No restart of the server is needed. |
| `503 bridge_unavailable`, "bridge did not reply within 3000 ms" | The EA is attached but not answering | Check the Experts log. Remove and re-attach the EA. Raise `bridge.timeout_ms` only if replies are slow. |
| `unknown symbol: XAUUS` or `unknown symbol` in the watchlist errors | The broker's name differs (`XAUUSD.m`, `XAUUSD_`, `GOLD`, `US100`, `NAS100`) | Find the exact name in Market Watch (Ctrl+M). Set `[symbols] suffix` (for example `".m"`), or an alias under `[symbols.aliases]`. Restart. |
| `writes refused: Algo Trading is disabled in the terminal` or `... trading is not allowed for this EA` | The Algo Trading button is off, or the EA's Allow Algo Trading is not ticked | Turn on the Algo Trading button and tick Allow Algo Trading on the EA's Common tab. |
| `writes refused on a real account (InpAllowReal is false)` | The MT5 account is real and the EA input is off | Expected while on a real account. Set `InpAllowReal` only if you have decided to trade real money. |
| `volume exceeds the EA's InpMaxLot (1)` | The order is larger than the EA's own cap | Expected. Lower the order, or raise `InpMaxLot` on the EA's Inputs tab (and re-attach it) if the larger size is intended. |
| `cannot open the key file` / `the key file ... is not usable` in the EA log; `refusing to start: ... no bridge key` from the API | The bridge key is missing or damaged | Run `pwsh scripts\install-ea.ps1` and set `[bridge] key_file`. See [The bridge key](#the-bridge-key). |
| `authentication failed` (retcode -4) or `reply authentication failed` | The API and the EA use different keys, the clock is off by more than 30 s, or another program uses the port | See [When requests are refused](#when-requests-are-refused). |
| `the EA speaks protocol version 1 but this client speaks 2` | The EA is the old 0.2.0 build | Run `scripts\install-ea.ps1`, `scripts\compile-ea.ps1`, and re-attach the EA. |
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
| `401 unauthorized` with "invalid username, password or code" | Wrong password, code, or this address is locked out | Re-check the code. After 10 failed attempts from one address that address is locked out of the account for 15 minutes; other addresses are not affected. Restarting the server clears it. After 50 failures from many addresses every attempt on the account waits 2 s for 15 minutes. |
| `429 rate_limited` on login | More than 5 login attempts in a minute from one client IP | Wait for the `Retry-After` time. Behind a reverse proxy all users share one IP. |
| `429 rate_limited` on confirm | More than 10 confirmations per minute in one session | Wait, then confirm again. Each preview still expires after 15 seconds. |
| `429 rate_limited` on chat or voice transcription | More than `chat_rate_limit_per_minute` (20) chat requests, or `transcribe_rate_limit_per_minute` (20) transcriptions, in a minute in one session | Wait for the `Retry-After` time. Raise the limit under `[server]` if you really need more. |
| `408 request_timeout` | A request took longer than `request_timeout_secs` (30 s), or `long_request_timeout_secs` (200 s) for chat, voice and confirm | For chat: a slow model, so use a faster one or raise `long_request_timeout_secs` above the provider's `timeout_secs`. After a timeout on confirm, check the positions before trying again: the trade keeps running. |
| `422 gate_rejected`, "The checks were run again at confirmation ..." | Between the preview and your confirmation a limit was reached (for example another preview was confirmed and `max_positions` is full), the day's loss limit was hit, the trading hours ended, or the market moved so that SL or TP no longer fits | Nothing was sent and the confirmation is spent. Look at the message, then ask for the order again if you still want it. |
| `403 origin not allowed` on the live-update connection, behind a reverse proxy | The proxy changed the `Host` header, so the `Origin` does not match | Make the proxy pass `Host $host` (or the public host), and keep the `Upgrade` headers. See the [VPS guide](vps.md). |
| `config file not found at config/jarvis.toml` | Config not copied, or started from another folder | Copy `config\jarvis.example.toml` to `config\jarvis.toml`, start from the repository root, or set `JARVIS_CONFIG`. |
| `install-ea.ps1`: `SHA-256 mismatch` or `has no SHA-256 entry` | A DLL in `mql5\Libraries` changed, or the table in `mql5\THIRD_PARTY.md` does not match | Restore the DLL from Git, or rebuild it with `scripts/build-libzmq.ps1`. Do not edit a hash to match a file you do not recognise. |
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
cargo run -p jarvis-bridge-client --example bridge_cli -- --key-file <path to bridge.key> ping
cargo run -p jarvis-bridge-client --example bridge_cli -- --key-file <path to bridge.key> price XAUUSD
cargo run -p jarvis-bridge-client --example bridge_cli -- --key-file <path to bridge.key> account
```

`bridge_cli` needs the bridge key like the API does (see [The bridge key](#the-bridge-key)): pass `--key-file`, or set `JARVIS_BRIDGE_KEY` in the environment.
The other `bridge_cli` examples in this runbook leave the option out for brevity; add it.

The `symbol` argument takes the broker's name, not an alias. The `ping` reply reports `trade_mode`. Check it says `demo` before any write.

## Updating the EA's DLLs

The only DLL is `mql5\Libraries\libzmq.dll`, libzmq 4.3.5 built from the official source (no CURVE, no libsodium, static C runtime, so it needs no
Visual C++ runtime). `scripts/install-ea.ps1` refuses to install a DLL whose hash does not match the table in `mql5\THIRD_PARTY.md`, so the table and the file
change together. [SPEC.md](../SPEC.md) requires approval before new binaries are committed.

Needs the Visual Studio Build Tools (workload "Desktop development with C++"), CMake and an internet connection for the first run.

1. To upgrade libzmq, change `$Version`, `$TarballUrl` and `$TarballSha256` at the top of `scripts\build-libzmq.ps1`.
2. Build: `powershell -ExecutionPolicy Bypass -File scripts\build-libzmq.ps1`. It downloads the tarball, checks its SHA-256, builds, checks that the DLL imports only
   Windows system DLLs and exports every function the mql-zmq headers use, copies it to `mql5\Libraries\libzmq.dll` and prints its SHA-256. Add `-RunTests` to run
   libzmq's own tests too (stop MT5 first; some tests use port 5555). The same toolchain gives the same hash, so you can rebuild to check the committed file.
3. Update the table and the build notes in `mql5\THIRD_PARTY.md` (version, tarball hash, compiler, DLL SHA-256, keeping the table format; the install script reads it).
4. If the mql-zmq headers are re-vendored, re-apply the local patch listed in `THIRD_PARTY.md`.
5. Close the terminal (or remove the EA from its chart) because MT5 keeps the old DLL locked, then reinstall, recompile and check:
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
