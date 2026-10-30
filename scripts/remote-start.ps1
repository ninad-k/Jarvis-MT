# -----------------------------------------------------------------------------
# remote-start.ps1 -- reach Arc MT from your phone (or any browser) through a
# Cloudflare quick tunnel to the loopback API. Stays attached; Ctrl+C closes the tunnel.
#
# THIS EXPOSES A UI THAT CAN PLACE TRADES TO THE INTERNET. The quick tunnel gives the
# API a random https://<words>.trycloudflare.com address that changes on every start.
# Anyone who learns the address can load the sign-in page; what stands between them and
# your account is the password plus the authenticator code, and the kill switch and the
# gate's limits behind them. So this script REFUSES to start unless ALL of these hold:
#
#   1. [auth] enabled is true in the config AND the running API reports login on. (With
#      login off the API refuses non-loopback Host names; with a tunnel it would also be
#      an open trading console.)
#   2. [auth] cookie_secure = true. The tunnel terminates TLS and talks plain HTTP to the
#      loopback API, so the API cannot see that the browser used HTTPS; without this the
#      session cookie would not be marked Secure. -SetCookieSecure adds it to the config
#      for you (a backup copy goes to .local\), but the API must then be restarted, so the
#      script stops after writing it.
#   3. at least one login user exists (`jarvis-api user list`). Checked only when the
#      other checks passed, because listing users opens the database.
#   4. JARVIS_JWT_SECRET is present and strong, and the config was not changed after the
#      running API started (a changed file is not applied until a restart).
#   5. the API is running (start it with scripts\run.ps1; this script does not start it).
#   6. cloudflared is installed, and no tunnel of this repo is already running.
#
# Then it starts `cloudflared tunnel --url <the API address>`, prints the public address,
# saves it to .local\remote-url.txt (gitignored; removed on stop) and keeps the tunnel up.
# The tunnel is recognised by its --logfile argument (.local\remote-tunnel.log), which is
# how scripts\remote-stop.ps1, stop.ps1 and status.ps1 find it without a PID file.
#
# Things to know before you use it:
#   * Everything arriving through the tunnel looks like 127.0.0.1 to the API. Its login
#     rate limits and lockouts are counted per client address, so all remote users share
#     them: ten wrong attempts by anyone lock the account for 15 minutes for everyone
#     (restarting the API clears it). The audit log records 127.0.0.1 as the client.
#   * If the address leaks: engage the KILL SWITCH (it blocks every write), then close the
#     tunnel (Ctrl+C here, or scripts\remote-stop.ps1) -- the address dies with it.
#   * A quick tunnel has no uptime promise; for a permanent address use a reverse proxy
#     with your own certificate (docs\vps.md).
#
#   scripts\remote-start.ps1                    start the tunnel
#   scripts\remote-start.ps1 -SetCookieSecure   add  cookie_secure = true  to the config, then stop
#   scripts\remote-start.ps1 -Config <path>     use another config file
#   scripts\remote-stop.ps1                     close it from another window
# -----------------------------------------------------------------------------
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [switch]$SetCookieSecure,
    [string]$Config
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lib\common.ps1')

Set-WindowTitle 'checking...'
Write-Banner "$AppName - remote access"

$configPath = Resolve-ConfigPath $Config
$cfg = Get-JarvisConfig -Path $configPath
$dotenv = Read-DotEnv (Join-Path $Root '.env')
$problems = New-Object System.Collections.Generic.List[string]
$configChanged = $false

Write-Step 'Checking that exposing this API is safe'

# 1. config + login
if (-not $cfg.Exists) {
    Write-Fail "config file not found: $configPath"
    $problems.Add('config file missing')
} elseif ($cfg.Error) {
    Write-Fail "config file cannot be read: $($cfg.Error)"
    $problems.Add('config unreadable')
} else {
    if ($cfg.AuthEnabled) {
        Write-Ok '[auth] enabled = true'
    } else {
        Write-Fail '[auth] enabled = false in the config -- remote access would be an open trading console'
        Write-Hint 'Set enabled = true (or remove the line), create a user, restart the API.'
        $problems.Add('login disabled in the config')
    }

    # 2. cookie_secure
    if ($cfg.CookieSecure -eq $true) {
        Write-Ok '[auth] cookie_secure = true'
    } elseif ($SetCookieSecure) {
        $text = [System.IO.File]::ReadAllText($configPath)
        $newText = Set-TomlKeyText -Text $text -Section 'auth' -Key 'cookie_secure' -Literal 'true'
        if ($PSCmdlet.ShouldProcess($configPath, 'Set [auth] cookie_secure = true (a backup copy goes to .local\)')) {
            New-Item -ItemType Directory -Force -Path $LocalDir | Out-Null
            $backup = Join-Path $LocalDir ('config-backup-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.toml')
            Copy-Item -LiteralPath $configPath -Destination $backup -ErrorAction Stop
            Write-TextNoBom -Path $configPath -Text $newText
            Write-Ok "[auth] cookie_secure = true written to $configPath (backup: $backup)"
            $configChanged = $true
        } else {
            Write-Hint 'would set [auth] cookie_secure = true'
        }
    } else {
        Write-Fail '[auth] cookie_secure is not true. The tunnel ends TLS before the API, so the session cookie would not be marked Secure.'
        Write-Hint 'Run again with -SetCookieSecure to add  cookie_secure = true  to the config (then restart the API), or add it by hand.'
        $problems.Add('cookie_secure is not true')
    }

    # 4a. secret
    $why = Test-JwtSecret (Get-EnvSetting 'JARVIS_JWT_SECRET' $dotenv)
    if ($why) {
        Write-Fail "JARVIS_JWT_SECRET $why"
        $problems.Add('JWT secret unusable')
    } else {
        Write-Ok 'JARVIS_JWT_SECRET is set'
    }
}

# 5. the API is running, and login is on in the RUNNING instance
$api = Get-ApiSummary -BaseUrl $cfg.ApiBase -TimeoutSec 3
if (-not $api.Healthy) {
    Write-Fail "the API is not running at $($cfg.ApiBase) ($($api.HealthError))"
    Write-Hint 'Start it first: scripts\run.ps1  (this script does not start it)'
    $problems.Add('API not running')
} elseif ($api.LoginRequired) {
    Write-Ok 'the running API requires login'
} elseif ($api.StatusAvailable -and $api.AuthEnabled -eq $false) {
    Write-Fail 'the RUNNING API has login DISABLED (/api/status reports auth.enabled = false)'
    Write-Hint 'Enable login in the config, create a user, restart the API, then run this script again.'
    $problems.Add('login disabled in the running API')
} elseif ($api.StatusAvailable) {
    Write-Ok 'the running API reports login on'
}

# 4b. a config edited after the API started has not been applied
$apiExe = $null
if ($api.Healthy) {
    $owners = @(Get-PortOwners $cfg.Port)
    $info = $null
    if ($owners.Count -gt 0) { $info = Get-ProcessSummary $owners[0] }
    if ($info) {
        if ($info.ExecutablePath -and (Test-Path -LiteralPath $info.ExecutablePath)) { $apiExe = $info.ExecutablePath }
        if ($cfg.Exists -and $info.CreationDate -and (Get-Item -LiteralPath $configPath).LastWriteTime -gt $info.CreationDate) {
            Write-Fail "the config file was changed after the API started (pid $($info.ProcessId), $(Format-Age $info.CreationDate) ago) -- the running API does not have the change"
            Write-Hint 'Restart the API (Ctrl+C in the run.ps1 window, then scripts\run.ps1) and run this script again.'
            $problems.Add('API predates the config change')
        }
    }
}

# 6. cloudflared, and no tunnel yet
$cloudflared = Find-Cloudflared
if (-not $cloudflared) {
    Write-Fail 'cloudflared is not installed (winget install --id Cloudflare.cloudflared)'
    $problems.Add('cloudflared missing')
} else {
    Write-Ok "cloudflared found ($cloudflared)"
}
$already = @(Get-RemoteTunnelProcesses)
if ($already.Count -gt 0) {
    $known = if (Test-Path -LiteralPath $RemoteUrlFile) { Get-Content -LiteralPath $RemoteUrlFile -TotalCount 1 } else { 'address unknown' }
    Write-Fail "a remote tunnel of this repo is already running (pid $($already[0].ProcessId), $known)"
    Write-Hint 'Close it with scripts\remote-stop.ps1 first.'
    $problems.Add('tunnel already running')
}

# 3. users (opens the database, so only when everything else is in order)
if ($problems.Count -eq 0 -and -not $configChanged) {
    $exe = if ($apiExe) { $apiExe } else { Find-ApiExe }
    if (-not $exe) {
        Write-Fail 'no jarvis-api binary found to list users with (cargo build --release -p jarvis-api)'
        $problems.Add('cannot list users')
    } else {
        $outFile = Join-Path ([System.IO.Path]::GetTempPath()) ('arcmt-users-' + [guid]::NewGuid().ToString('N') + '.out')
        $errFile = "$outFile.err"
        try {
            # (listing users is a read-only check, so a -WhatIf run still does it)
            $proc = Start-Process -FilePath $exe -ArgumentList @('--config', "`"$configPath`"", 'user', 'list') `
                -WorkingDirectory $Root -NoNewWindow -PassThru -RedirectStandardOutput $outFile -RedirectStandardError $errFile
            [void]$proc.Handle    # keeps the exit code readable after the process ends
            if (-not $proc.WaitForExit(30000)) { Stop-ProcessTree -TargetId $proc.Id; throw 'timed out after 30 s' }
            $listing = @(Get-Content -LiteralPath $outFile -ErrorAction SilentlyContinue | Where-Object { $_.Trim() -ne '' })
            if ($proc.ExitCode -ne 0) {
                $first = @(Get-Content -LiteralPath $errFile -ErrorAction SilentlyContinue | Select-Object -First 1)
                throw "exit code $($proc.ExitCode) $first"
            }
            $users = @($listing | Where-Object { $_ -match '\bcreated\b' })
            if ($users.Count -eq 0) {
                Write-Fail 'no login user exists yet'
                Write-Hint "Create one first:  $exe user add <name>"
                $problems.Add('no users')
            } else {
                Write-Ok "$($users.Count) login user(s) exist"
            }
        } catch {
            Write-Fail "could not list the users: $($_.Exception.Message)"
            $problems.Add('cannot list users')
        } finally {
            Remove-Item -LiteralPath $outFile, $errFile -Force -ErrorAction SilentlyContinue -WhatIf:$false
        }
    }
} elseif ($problems.Count -gt 0) {
    Write-Hint 'users not checked yet (fix the items above first)'
}

if ($configChanged) {
    Write-Host ''
    Write-Warn 'The config changed. Restart the API so it takes effect, then run scripts\remote-start.ps1 again.'
    Write-Host ''
    exit 1
}
if ($problems.Count -gt 0) {
    Write-Host ''
    Write-Fail "REFUSING to open a public tunnel: $($problems.Count) problem(s) above ($($problems -join '; '))."
    Write-Hint 'Nothing was started.'
    Write-Host ''
    exit 1
}

if ($WhatIfPreference) {
    Write-Host ''
    Write-Host '  -WhatIf run: every check passed; the tunnel would be opened now. Nothing was started.' -ForegroundColor Yellow
    Write-Host ''
    exit 0
}

# --- start the tunnel ---------------------------------------------------------------------------
New-Item -ItemType Directory -Force -Path $LocalDir | Out-Null
foreach ($f in @($RemoteTunnelLog, "$RemoteTunnelLog.out", "$RemoteTunnelLog.err", $RemoteUrlFile)) {
    # A leftover from an earlier run must never be read as this run's address.
    if (Test-Path -LiteralPath $f) { try { Remove-Item -LiteralPath $f -Force -ErrorAction Stop } catch { } }
}
$tunnel = $null
try {
    Write-Step 'Opening the Cloudflare quick tunnel'
    $tunnelArgs = @('tunnel', '--no-autoupdate', '--url', $cfg.ApiBase, '--logfile', "`"$RemoteTunnelLog`"")
    if ($cfg.Scheme -eq 'https') { $tunnelArgs += '--no-tls-verify' }    # the origin is this PC's own loopback listener
    $tunnel = Start-Process -FilePath $cloudflared -ArgumentList $tunnelArgs -WorkingDirectory $Root `
        -NoNewWindow -PassThru -RedirectStandardOutput "$RemoteTunnelLog.out" -RedirectStandardError "$RemoteTunnelLog.err"

    $url = $null
    $deadline = (Get-Date).AddSeconds(45)
    while ((Get-Date) -lt $deadline -and -not $tunnel.HasExited) {
        $text = (Read-SharedFile $RemoteTunnelLog) + (Read-SharedFile "$RemoteTunnelLog.err") + (Read-SharedFile "$RemoteTunnelLog.out")
        foreach ($m in [regex]::Matches($text, 'https://[a-z0-9-]+\.trycloudflare\.com')) {
            if ($m.Value -notmatch '^https://api\.') { $url = $m.Value; break }    # api.trycloudflare.com is Cloudflare's own API
        }
        if ($url) { break }
        Start-Sleep -Milliseconds 500
    }
    if (-not $url) {
        if ($tunnel.HasExited) { Write-Fail "cloudflared exited (code $($tunnel.ExitCode)) before announcing an address" }
        else { Write-Fail 'cloudflared announced no address within 45 s' }
        foreach ($l in (Read-LogTail "$RemoteTunnelLog.err" 6)) { Write-Hint "  $l" }
        Write-Hint "Log: $RemoteTunnelLog"
        exit 1
    }
    Write-TextNoBom -Path $RemoteUrlFile -Text ($url + "`n")
    Write-Ok "tunnel (pid $($tunnel.Id)) -> $url"

    # Through Cloudflare end to end (the new name can take a few seconds to resolve).
    $reached = $false
    $until = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $until -and -not $tunnel.HasExited) {
        if ((Invoke-HttpGet "$url/api/health" 4).Ok) { $reached = $true; break }
        Start-Sleep -Seconds 2
    }
    if ($reached) { Write-Ok 'reachable through Cloudflare (/api/health answers)' }
    else { Write-Warn 'not reachable yet through Cloudflare -- DNS may still be propagating; try the address in a minute' }

    Write-Host ''
    Write-Host "  public address    $url" -ForegroundColor Green
    Write-Host "  forwards to       $($cfg.ApiBase)  (saved to $RemoteUrlFile)" -ForegroundColor White
    Write-Host ''
    Write-Warn 'This address is PUBLIC. Anyone who has it can reach the sign-in page; only your'
    Write-Warn 'password + authenticator code (and the limits and kill switch behind them) protect the account.'
    Write-Host '    - It changes every time this script starts. Do not post it anywhere.' -ForegroundColor DarkGray
    Write-Host '    - If it leaks: engage the KILL SWITCH in the web UI, then close the tunnel (Ctrl+C here,' -ForegroundColor DarkGray
    Write-Host '      or scripts\remote-stop.ps1). The address dies with the tunnel.' -ForegroundColor DarkGray
    Write-Host '    - All remote users share the login rate limit and lockout (they all look like 127.0.0.1).' -ForegroundColor DarkGray
    Write-Host ''
    Write-Host '  Press Ctrl+C to close the tunnel.' -ForegroundColor DarkGray
    Write-Host ''
    Set-WindowTitle 'tunnel open'

    $apiWarned = $false
    $lastCheck = Get-Date
    while ($true) {
        if ($tunnel.HasExited) {
            Write-Fail "cloudflared stopped (exit code $($tunnel.ExitCode)) -- the public address no longer works"
            foreach ($l in (Read-LogTail "$RemoteTunnelLog.err" 6)) { Write-Hint "  $l" }
            break
        }
        if (((Get-Date) - $lastCheck).TotalSeconds -ge 15) {
            $lastCheck = Get-Date
            $alive = Test-TcpConnect -Port $cfg.Port
            if (-not $alive -and -not $apiWarned) {
                $apiWarned = $true
                Write-Warn 'the API stopped answering -- the tunnel stays open but serves errors until it is back (Ctrl+C to close)'
            } elseif ($alive) { $apiWarned = $false }
        }
        Start-Sleep -Milliseconds 500
    }
} finally {
    Write-Host ''
    Write-Step 'Closing the tunnel'
    Set-WindowTitle 'closing...'
    if ($tunnel -and -not $tunnel.HasExited) { Stop-ProcessTree -TargetId $tunnel.Id }
    try { if (Test-Path -LiteralPath $RemoteUrlFile) { Remove-Item -LiteralPath $RemoteUrlFile -Force -ErrorAction Stop } } catch { }
    Write-Ok 'Closed (the API keeps running)'
    Set-WindowTitle 'closed'
    Write-Host ''
}
