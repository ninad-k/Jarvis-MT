# Windows service

`jarvis-api` can run as a Windows service named **JarvisMT** (display name "Jarvis MT"). The service starts
the server at boot without a logged-in user, restarts it after a crash, and writes its log to a file. The
service is for Windows only. On other systems, use the console or your own service manager.

The service runs the same server as the console. Only one copy can listen on `server.bind`, so stop the
service before you start `jarvis-api` by hand on the same port.

MetaTrader 5 is not part of the service. The MT5 terminal, with the JarvisBridge EA attached, still needs to run in a
logged-in user session. See the [VPS guide](vps.md) for how to start it after a reboot.

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
# Register the service (does not start it)
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
| Display name | `Jarvis MT` |
| Description | One line naming the product and MetaTrader 5, as set in the code |
| Executable | The `jarvis-api.exe` you ran `install` with (its absolute path) |
| Arguments | `--config <absolute config path> service run --workdir <absolute folder you ran install from>` |
| Start type | Automatic, delayed start |
| Account | LocalSystem (see "Running under a dedicated account" below) |
| Failure actions | Restart after 5 s, then 15 s, then 60 s. The failure count resets after 24 hours without a failure. Applies to crashes and to non-crash failures (an exit with an error code). |

`install` fails if the service already exists, and checks that the config file exists before it registers anything. The
config path is made absolute at install time, so the service keeps working when the working directory is changed later.

Because the arguments are fixed at install time, changing the config path or moving the folder means you
reinstall the service: `service uninstall`, then `service install` from the new folder.

## Running under a dedicated account

By default the service runs as LocalSystem, which has broad rights on the machine. To limit that, run it as a dedicated
local account that is a standard user (not an administrator):

```powershell
sc.exe config JarvisMT obj= ".\jarvis-svc" password= "<the account password>"
```

Replace `jarvis-svc` with your account. `sc.exe` expects a space after `obj=` and after `password=`. The password
ends up in your shell history, so clear it from the history afterwards, or set the account in the service's Log On
tab in `services.msc` instead.

For a dedicated account:

- The account needs **Modify** rights on the Jarvis folder (`config\` read, and write access to `data\`, `logs\` and
  `models\` read). Everything the server writes goes there.
- Create users (`jarvis-api user ...`) as the same account, or make sure that account can write `data\`. SQLite
  needs write access to the database folder, including the `-wal` and `-shm` files.
- The account needs **Read** access to the bridge key file named in `[bridge] key_file` (by default under the MT5
  data folder of the user who runs MT5, `MQL5\Files\JarvisMT\bridge.key`). Grant read on that one file only, or copy
  the key into `.env` as `JARVIS_BRIDGE_KEY` for the service account. Without it the server refuses to start.
- Give the account the "Log on as a service" right if Windows refuses to start the service with an access error.
- Restart the service after changing the account: `service stop`, then `service start`.

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
| Access denied on `data\jarvis.db` after changing the account | The database was created by another account | Give the service account Modify rights on `data\`, or recreate the database as that account. |
