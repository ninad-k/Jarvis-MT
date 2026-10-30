# Running on a Windows VPS

Running Arc MT on a Windows VPS keeps MetaTrader 5 online while your PC is off, and lets you reach the web UI
from a phone or another computer. The whole stack goes on the same VPS: MT5 with the JarvisBridge EA, `jarvis-api`
and, if you use it, Ollama. The EA listens on loopback only, so MT5 and the API must share one machine.

Only the web UI is exposed to the network, over HTTPS, with a login and a TOTP code. Read the [configuration](configuration.md)
notes on non-loopback binds before you open any port.

## Sizing

| Part | What to plan for |
|---|---|
| MetaTrader 5 | A normal desktop session. It needs a logged-in Windows user session, so the VPS must stay logged in (see "Starting after a reboot"). |
| Speech-to-text (Whisper `base.en`) | Runs on the CPU. The maintainer measured about 1 to 2 seconds per short utterance on CPU with `ggml-base.en`. That is a single observation, not a benchmark in this repository, so measure on your VPS. |
| Text-to-speech (Piper) | Runs on the CPU and is fast for short replies. Measure on your VPS. |
| LLM, local (Ollama, `qwen3.5:9b`) | The example config targets a GPU with about 8 GB of memory, where replies take about 2 seconds once the model is loaded. On a CPU-only VPS, replies are too slow for voice. Use a cloud provider instead, or a VPS with a suitable GPU. |
| LLM, cloud (Anthropic or OpenAI) | No local model. Your messages and MT5 data (prices, positions, deals) leave the VPS and go to that provider. |
| Disk | The repository, build output, the voice models in `models\` and `data\jarvis.db`. Keep at least a few gigabytes free, and check the audit database grows as expected. |

Pick the LLM provider before you pick the VPS. Voice needs fast local speech; chat needs a model that answers within the timeouts in the config.

## Folder layout

Use one folder that the service runs from, for example `D:\Jarvis`. It holds:

```text
D:\Jarvis\
  jarvis-api.exe          (or target\release\jarvis-api.exe)
  config\jarvis.toml      (not in Git)
  .env                    (not in Git; JARVIS_JWT_SECRET and provider keys)
  data\jarvis.db          (created on first start)
  logs\                   (service log, one file per day)
  models\                 (voice models, optional)
  web\build\              (the built web UI, if static_dir points there)
```

Limit who can read `.env`, `config\`, `data\` and `certs\`. Only administrators and the account that runs the
service need access. See the [Windows service](windows-service.md) notes on file rights.

## Hardening checklist

**Windows and MT5**

- Install Windows updates and set a maintenance window with a reboot. Plan for MT5 and Jarvis to come back after the reboot (see below).
- Keep MT5 and the broker's terminal updated.
- Enable Windows time sync (the Windows Time service). Authenticator codes use 30-second steps, and a clock that drifts
  by more than about one step makes codes fail.

**Network**

- Allow inbound traffic only to the HTTPS port you serve: `8787` when Jarvis terminates TLS itself, or `443` when a reverse proxy
  does. Block everything else, in particular `5555` (the bridge) and `11434` (Ollama). Both are meant to be loopback-only.
- Restrict Remote Desktop (3389): allow it only from your own IP addresses or over a VPN, or disable it from the internet.
  Use Network Level Authentication, and do not use the built-in Administrator account for daily work.
- Use the provider's firewall as well as Windows Firewall, if it has one. `scripts\firewall.ps1` creates the scoped allow rule and the bridge block rule (see "Firewall rules with a script").

**Accounts**

- Run the service under a dedicated standard account (see [windows-service.md](windows-service.md)).
- Keep a separate Windows account for RDP access, with a strong password.
- Create Jarvis login users with long passwords (12 characters minimum, more is better), each with their own authenticator app.
- Keep `JARVIS_JWT_SECRET` long and random, and do not share the `.env` file.

**Jarvis**

- Keep `[auth] enabled = true`. The server refuses to bind off loopback without it.
- Keep `mode = "demo"` until you have run the full confirmation flow on a demo account for some time (see LIVE below).
- Keep `[limits]` tight: `max_lot`, `max_daily_loss`, `allowed_symbols` and `trading_hours` (in broker server time).
- Keep `[trading] kill_switch_engaged = false` for normal running, and know how to engage the kill switch (see below).

## TLS

The web UI, the login cookie and the live-update connection all need HTTPS when the server is reachable from the network.
Choose one of three ways.

**1. Certificate and key files, served by Jarvis itself.** Use a certificate issued for a name you control. Put the PEM files
under `certs\` (gitignored) and set both keys in `[server]`:

```toml
[server]
bind = "0.0.0.0:8787"
static_dir = "web/build"
tls_cert = "certs/jarvis.crt"     # full chain, PEM
tls_key  = "certs/jarvis.key"     # PEM, not encrypted
```

Jarvis reads the certificate once at start. **Restart the server after every renewal.** Set `[auth] enabled = true`, which the
server requires for any non-loopback bind.

**2. A private certificate with mkcert (private use only).** mkcert creates a local root certificate authority that your devices must trust.
Every device that connects needs the root installed, so this suits one PC or the desktop app, not an open network. The root key
(`mkcert -CAROOT`) lets anyone issue certificates your devices will trust, so keep it private. The commands are in [desktop.md](desktop.md).

**3. A reverse proxy in front of loopback.** Let the proxy terminate TLS, and have Jarvis listen on loopback only:

```toml
[server]
bind = "127.0.0.1:8787"

[auth]
enabled = true
cookie_secure = true             # the session cookie is Secure, though Jarvis itself serves plain HTTP
```

A proxy such as Caddy then does the TLS work and forwards to `127.0.0.1:8787`:

```text
jarvis.example.com {
    reverse_proxy 127.0.0.1:8787
}
```

Points to get right with a proxy:

- **Host header.** Jarvis accepts a live-update (WebSocket) connection only when its `Origin` matches the `Host` the request was sent to.
  The proxy must pass the public host name through. Caddy does this by default. With nginx, use `proxy_set_header Host $host;`.
- **WebSocket upgrades.** The proxy must forward them. Caddy does this by default. With nginx, add
  `proxy_set_header Upgrade $http_upgrade;` and `proxy_set_header Connection "upgrade";`.
- **Client IP.** Jarvis ignores `X-Forwarded-For`. It counts login attempts per TCP peer address, so behind a proxy every client shares
  one bucket of 5 login attempts per minute. Lockouts (10 failed attempts, 15 minutes) are kept per account and client address, so behind a proxy they hit every user together:
  one person's ten failures lock all of you out for 15 minutes, until the server restarts. The same applies to the per-session
  limits on chat and voice transcription when login is off.
  The audit log records the proxy's address as the client IP.
- **`cookie_secure`.** Set it to `true` when the proxy serves HTTPS and Jarvis serves plain HTTP. Without it, the session cookie is not
  `Secure` on the plain-HTTP side.

Do not expose the plain HTTP port directly on the network. Loopback binding is what keeps it private in this setup.

## Firewall rules with a script

`scripts\firewall.ps1` (elevated prompt; a `-WhatIf` preview works without one) manages two inbound rules:

```powershell
scripts\firewall.ps1 -RemoteAddress 203.0.113.7,198.51.100.0/24 -WhatIf     # preview
scripts\firewall.ps1 -RemoteAddress 203.0.113.7,198.51.100.0/24             # create or update
scripts\firewall.ps1 -RemoteAddress 203.0.113.7 -Ports 443                  # the port a reverse proxy listens on
scripts\firewall.ps1 -Remove
```

- **"Arc MT API (HTTPS, restricted)"** allows the API port (the port of `[server] bind`, 8787 by default; `-Ports` overrides it) only from the addresses in `-RemoteAddress`.
  The list is required. "Any", `*`, `0.0.0.0/0` and `::/0` are refused: the rule exists to keep the port closed to everyone else.
  The script warns when the config has no `tls_cert` (the API would speak plain HTTP), when login is off, or when `bind` is still loopback (no rule can matter then).
- **"Arc MT Bridge (block inbound)"** blocks port 5555 on all profiles. The EA binds loopback anyway, and Windows Firewall does not filter loopback traffic, so the API still reaches it.
  A block rule wins over any allow rule, so a careless "allow 5555" elsewhere cannot open the order channel.

The script changes only these two rules. It lists, and never edits, other enabled inbound allow rules that name the same ports, above all ones that allow them from any address,
because those make the scoped rule pointless. Check from outside the machine afterwards: an address that is not on the list must time out, and port 5555 must never answer.
Mobile-carrier addresses change often, so for a phone prefer a VPN with a fixed address, or the tunnel below.

## Phone access through a Cloudflare quick tunnel

For occasional access from a phone, with no open port, `scripts\remote-start.ps1` runs `cloudflared` as a quick tunnel to the loopback API and prints a random
`https://....trycloudflare.com` address. Install `cloudflared` first (`winget install --id Cloudflare.cloudflared`), start the API (`scripts\run.ps1`), then:

```powershell
scripts\remote-start.ps1 -SetCookieSecure    # once: adds  cookie_secure = true  to [auth]; then restart the API
scripts\remote-start.ps1                     # open the tunnel; Ctrl+C closes it
scripts\remote-stop.ps1                      # close it from another window
```

This puts a UI that can place trades on the internet, so the script refuses to start unless all of these hold: `[auth] enabled = true` in the config and in the running API,
`[auth] cookie_secure = true` (the tunnel ends TLS before the API, so Jarvis cannot see it and would not mark the session cookie `Secure`), at least one login user exists,
`JARVIS_JWT_SECRET` is strong, the API is running with the config you edited, and `cloudflared` is installed.

Caveats:

- The address is public and changes every time. The only things between a stranger and your account are the password, the authenticator code, and behind them the limits and the kill switch.
  Do not post it. If it leaks, engage the kill switch, then close the tunnel; the address dies with it.
- Everything arrives from `127.0.0.1` as far as Jarvis can tell, so all remote users share the login rate limit and the lockout, and the audit log records `127.0.0.1` as the client (the same effect as a reverse proxy, see the notes under TLS).
- A quick tunnel has no uptime guarantee and no access policy. For a permanent address use a reverse proxy with your own certificate.
- The tunnel is found by its command line (a `--logfile` inside this repository's `.local\` folder), so `remote-stop.ps1`, `stop.ps1` and `status.ps1` never touch another `cloudflared` on the machine.

## Starting after a reboot

The Jarvis service starts at boot (delayed automatic start, see [windows-service.md](windows-service.md)). MT5 does not, because MT5 is a desktop program that
needs a logged-in user session, and the JarvisBridge EA lives inside it.

Choose one approach:

- **Auto-logon for a dedicated Windows account.** Set the account to log on automatically after a reboot. Use Sysinternals
  Autologon, which stores the password encrypted, not in plain text in the registry. Lock the account down: it can then reach MT5 and
  the Jarvis folder without anyone typing a password.
- **Manual logon after a reboot.** Keep it simple, and check the VPS after every maintenance window.

With auto-logon you can also start Arc MT itself in that session with `scripts\install-autostart.ps1` (a Task Scheduler logon task that runs `run.ps1` hidden) instead of the Windows service.
Choose one of the two: the script refuses to register the task while the service is installed. The service needs file grants for its virtual account and runs without a session; the task needs the session but no grants.

With either approach, make MT5 start when the user logs on: put a shortcut to the terminal in that user's Startup folder, or create a
scheduled task that runs at logon. Then check:

1. MT5 is logged into the **demo** account (or the account you intend, if you are running LIVE).
2. JarvisBridge is attached to a chart, with **Allow DLL imports** and **Allow Algo Trading** on the EA's Common tab, and the Algo Trading button is on.
3. `GET /api/health` answers, and the newest log file shows `bridge reachable`.

If the EA is not attached after a restart, follow the re-attach steps in the [runbook](runbook.md).

## Monitoring

- **Liveness.** Point an uptime monitor at `https://<your host>/api/health`. It needs no login and returns only `{"ok":true}`, so
  it proves the process and the TLS endpoint are up. It does not prove that MT5 is connected.
- **Bridge and account.** `GET /api/status` (needs login) shows the bridge state, `mode_mismatch` and the kill switch. Check it from the web UI after each
  maintenance window. A monitor cannot read it without a session.
- **Logs.** Read the newest file in `logs\` after a restart, and search for `ERROR` and `WARN` lines. The service keeps every daily file, so
  delete old ones on a schedule you choose.
- **MT5.** Check the Experts tab for the listening line after logon.

## Backups

Back up `data\jarvis.db` (with `-wal` and `-shm` if present), `config\jarvis.toml` and `.env`. Use a consistent copy, as described
in the [runbook](runbook.md). Copy backups off the VPS to storage only you can read. A backup of `.env` and `data\jarvis.db` contains
everything needed to log in, so protect both. Test restoring on another folder before you rely on it.

## Keeping the kill switch within reach

Know two ways to stop trading from your phone or another computer:

1. **Web UI kill switch.** Install the web app on your phone (it is a PWA) and log in. Engaging the switch needs only a session, so keep
   the login and your authenticator app handy. Releasing it needs a fresh code.
2. **MT5 Algo Trading button.** Over RDP, turn Algo Trading off. The EA refuses every write while the terminal's Algo Trading is off.

The kill switch does not close positions. Keep a way to close them in MT5 as well.

## LIVE mode

LIVE means real money. Before you consider it on a VPS:

- Run the full confirmation flow on a demo account, including batch close, cancel, the kill switch and a restart of the service. Do it on
  the same VPS you plan to use.
- Set `mode = "live"` deliberately, log MT5 into the real account, and set the EA input `InpAllowReal` to `true`. Each is a separate decision.
  Without all three the server, the EA or both refuse writes.
- The web UI then shows a LIVE banner. Check it is there after every restart.
- Set `max_lot`, `max_daily_loss`, `allowed_symbols` and `trading_hours` for the real account size, not for a demo.
- Watch the first trades yourself. The audit log records every step, so review it after each session.

This project is provided as is. You are responsible for every trade you confirm.

## Related docs

- [Windows service](windows-service.md): install, run and update the service.
- [Configuration](configuration.md): every setting.
- [Runbook](runbook.md): operations, incidents and recovery.
- [Architecture](architecture.md): how the parts fit together.
