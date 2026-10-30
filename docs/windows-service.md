# Windows service

`jarvis-api` can run as a Windows service named **JarvisMT** (display name "Arc MT"). The service starts
the server at boot without a logged-in user, restarts it after a crash, and writes its log to a file. The
service is for Windows only. On other systems, use the console or your own service manager.

The service runs the same server as the console. Only one copy can listen on `server.bind`, so stop the
service before you start `jarvis-api` by hand on the same port.

MetaTrader 5 is not part of the service. The MT5 terminal, with the JarvisBridge EA attached, still needs to run in a
logged-in user session. See the [VPS guide](vps.md) for how to start it after a reboot.

## Service or logon task?

There are two ways to have Arc MT start by itself. Use one of them, not both, because each starts a server on the same port.

| | Windows service (this page) | Logon task (`scripts\install-autostart.ps1`) |
|---|---|---|
| Runs | At boot, with nobody logged in | After you log on, hidden, in your session |
| Account | The virtual account `NT SERVICE\JarvisMT`, which has no rights until you grant them with `icacls` (see below) | You, with your own file rights, so there is nothing to grant |
| Ollama | Cannot start Ollama's per-user tray app | `-StartOllama` is part of the task |
| Stops | At shutdown, or `service stop` | At log off, or `scripts\stop.ps1` |
| Best for | A VPS or a machine that restarts unattended | A PC you work at, next to MT5 |

MT5 and the EA need a logged-in session either way. `install-autostart.ps1` refuses to register the task while this service is installed (`-Force` overrides),
and `scripts\run.ps1` refuses to start while the service is running. `scripts\status.ps1` shows the service state, and `scripts\stop.ps1 -IncludeService` stops it (elevated).

## Before you install

1. Build the release binary from the repository root:

   ```powershell
   cargo build --release -p jarvis-api
   ```

   The binary is `target\release\jarvis-api.exe`.
2. Make sure the folder you install from holds the runtime files, because the service uses that folder as its
   working directory:
   - `config\jarvis.toml` (or pass `--config` with the file you use)
   - `.env` with `JARVIS_JWT_SECRET` set
   - `data\` (created on first start, or already present)
   - `models\` (voice models, if voice is on)
   - `web\build\` (if `static_dir` points there)
3. Create at least one login user. The service does not ask for passwords, so create them beforehand with the same
   working directory and `.env` (see the [runbook](runbook.md)):

   ```powershell
   cargo run -p jarvis-api -- user add <name>
   ```

## Commands

Run these from an **elevated** (Administrator) PowerShell in the folder that holds `config\`, `data\`,
`models\` and `.env`. `install`, `uninstall`, `start` and `stop` need an elevated prompt. Without one, the
command fails with "cannot reach the Service Control Manager".

```powershell
# Register the service (does not start it). It runs as the virtual account NT SERVICE\JarvisMT,
# and the command prints the icacls lines that give that account its files: run them, then start.
.\target\release\jarvis-api.exe service install

# Start and stop
.\target\release\jarvis-api.exe service start
.\target\release\jarvis-api.exe service stop

# Remove the service (stops it first)
.\target\release\jarvis-api.exe service uninstall
```

Use the path of the executable you want the service to run. The examples use the release build. To keep a stable
copy, copy the binary to a folder of your choice and run `.\bin\jarvis-api.exe service install` from the folder that
holds the runtime files.

`service stop` returns as soon as the stop is requested. Check the state with:

```powershell
Get-Service JarvisMT
sc.exe query JarvisMT
```

`service uninstall` marks the service for deletion. It disappears from the list once it has stopped.

You can also use the standard Windows tools (`services.msc`, `sc.exe start JarvisMT`, `sc.exe stop JarvisMT`).

## What `service install` registers

| Setting | Value |
|---|---|
| Service name | `JarvisMT` |
| Display name | `Arc MT` |
| Description | One line naming the product and MetaTrader 5, as set in the code |
| Executable | The `jarvis-api.exe` you ran `install` with (its absolute path) |
| Arguments | `--config <absolute config path> service run --workdir <absolute folder you ran install from>` |
| Start type | Automatic, delayed start |
| Account | `NT SERVICE\JarvisMT`, the virtual account of the service (see "The service account" below) |
| Failure actions | Restart after 5 s, then 15 s, then 60 s. The failure count resets after 24 hours without a failure. Applies to crashes and to non-crash failures (an exit with an error code). |

`install` fails if the service already exists, and checks that the config file exists before it registers anything. The
config path is made absolute at install time, so the service keeps working when the working directory is changed later.

Because the arguments are fixed at install time, changing the config path or moving the folder means you
reinstall the service: `service uninstall`, then `service install` from the new folder.

## The service account

A flaw in the HTTP, audio or speech code would run with the rights of the service account, so the service does not
run as LocalSystem (finding SVC-1). `service install` registers it as **`NT SERVICE\JarvisMT`**, the virtual account
Windows keeps for a service: it has no password to store or leak, it is not an administrator, and it can open
nothing of yours until you grant it. `install` prints the grants. For a repository in `D:\Jarvis` they look like this
(the paths in your output are the real ones; lines for a missing `.env`, `models\` or web folder are left out):

```powershell
icacls "D:\Jarvis" /grant "NT SERVICE\JarvisMT:(RX)"
icacls "D:\Jarvis\bin\jarvis-api.exe" /grant "NT SERVICE\JarvisMT:(RX)"
icacls "D:\Jarvis\config\jarvis.toml" /grant "NT SERVICE\JarvisMT:(R)"
icacls "D:\Jarvis\.env" /grant "NT SERVICE\JarvisMT:(R)"
icacls "D:\Jarvis\models" /grant "NT SERVICE\JarvisMT:(OI)(CI)(RX)" /T
icacls "D:\Jarvis\web\build" /grant "NT SERVICE\JarvisMT:(OI)(CI)(R)" /T
icacls "C:\Users\you\AppData\Roaming\MetaQuotes\Terminal\<id>\MQL5\Files\JarvisMT\bridge.key" /grant "NT SERVICE\JarvisMT:(R)"
icacls "D:\Jarvis\data" /grant "NT SERVICE\JarvisMT:(OI)(CI)(M)" /T
icacls "D:\Jarvis\logs" /grant "NT SERVICE\JarvisMT:(OI)(CI)(M)" /T
```

What each gives, and why only that:

- the working folder, the program and the config file: enter, read and run. The folder grant is not inherited, so the
  other files in it stay closed;
- `.env`, `models\` (Piper is a program that lives there) and the web UI folder: read;
- the **bridge key** file: read, on that file only. Windows lets a program open a file by its full path without rights
  on the folders above it, so the MT5 profile folder does not have to be opened up. If the key is in `.env` as
  `JARVIS_BRIDGE_KEY` instead, no grant is needed;
- `data\` and `logs\`: modify, which the audit database (SQLite also writes `-wal` and `-shm` files) and the log files
  need. `install` creates the two folders if they are missing, and `/T` also covers files that already exist, such as a
  `jarvis.db` the console run created.

Create the users (`jarvis-api user add`) before you start the service, from your own account; the service account never
needs to. After `install`, run the grants, then `service start`, and look at the newest file in `logs\`.

Options of `service install`:

| Flag | Effect |
|---|---|
| (none) | `NT SERVICE\JarvisMT`, no password. |
| `--account <name>` | Another account. It must need no password (`NT AUTHORITY\LocalService`, a managed service account) unless you add `--password-stdin`, which reads the password from the first line of stdin so it does not appear in a command line or the history. The printed `icacls` commands name that account. |
| `--local-system` | LocalSystem, the most privileged account. `install` prints a warning. Use it only if the account above cannot work on your machine. |

A service that is already installed as LocalSystem is moved with
`sc.exe config JarvisMT obj= "NT SERVICE\JarvisMT"` (no password), followed by the grants above. Restart the service after
changing the account: `service stop`, then `service start`.

If the service does not start and the log is empty, the account probably lacks a grant: check the Windows Event Viewer
(Windows Logs > Application) and compare against the list above. Give the account the "Log on as a service" right if
Windows refuses to start the service with an access error (a virtual account gets it automatically).

## Logs

`service run` writes a daily rolling log file to `logs\` in the working directory:

```text
logs\jarvis-api.log.2026-10-30
```

- A new file starts each day. Old files are not deleted automatically, so delete old ones yourself.
- The file has no colour codes, so it is safe to open in any editor.
- The level comes from `RUST_LOG` in `.env`. Without it, the service uses `info,jarvis_api=debug,jarvis_bridge_client=info`.
- A failure that happens before logging starts (for example, the working directory cannot be entered) leaves no
  log file. Check the service status and the Windows Event Viewer (Windows Logs > Application).

To read the newest log file:

```powershell
Get-ChildItem .\logs | Sort-Object LastWriteTime -Descending | Select-Object -First 1 | Get-Content -Tail 50
```

## Stopping and restarts

- `service stop` (or a Windows shutdown) asks the server to stop. Open WebSocket connections are closed, and other
  connections get up to 10 seconds to finish. The log then shows `shut down cleanly`.
- A crash makes Windows restart the service after 5, 15 and 60 seconds.
- The kill switch is saved to disk. A restart, including this automatic one, keeps an engaged switch engaged until it is
  released with a login session and a fresh authenticator code. See [configuration](configuration.md).

## Updating the binary

Windows locks a running executable, so stop the service before you replace the binary.

1. Stop the service: `.\target\release\jarvis-api.exe service stop`, then wait until `Get-Service JarvisMT` shows `Stopped`.
2. Build the new binary: `cargo build --release -p jarvis-api`. If the service runs from a separate folder, copy the new
   `jarvis-api.exe` over the old one instead.
3. If you changed `web/`, run `pnpm -C web build`. The server reads the built files and their hashes at startup.
4. Start the service: `.\target\release\jarvis-api.exe service start`.
5. Check the newest log file for the `starting` line with the new version, and for `listening on`.

Database migrations run automatically at startup. Back up `data\jarvis.db` first (see the [runbook](runbook.md)).

## Uninstall

```powershell
.\target\release\jarvis-api.exe service uninstall
```

This removes the service registration only. Your config, `.env`, `data\` and `logs\` stay in place. Delete them yourself if
you no longer need them, after taking a backup.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `cannot reach the Service Control Manager` | The command was run from a non-elevated prompt | Open PowerShell as Administrator. |
| `config file ... does not exist` during `install` | Wrong folder, or no `config\jarvis.toml` | Run install from the folder with the config, or pass `--config`. |
| `creating the service (is it already installed?)` | The service is registered already | `service uninstall`, then install again. |
| `starting the service dispatcher (service run is started by the Service Control Manager ...)` | `service run` was started by hand from a console | Use `service start`, or run `jarvis-api` in a console instead. |
| Service stops right after start, and the log shows `refusing to start` | Config or secret problem (see the log message) | Fix the file it names, then `service start`. |
| `binding 127.0.0.1:8787 (is another jarvis-api running?)` | A console copy is using the port | Stop the console copy. |
| The service does not start after reboot | Delayed start takes a while after boot, or the start failed | Check `Get-Service JarvisMT` after a few minutes and the log file. |
| Access denied on `data\jarvis.db`, `.env` or the config after installing | The service account was not given its files | Run the `icacls` commands that `service install` printed (see "The service account"). |
