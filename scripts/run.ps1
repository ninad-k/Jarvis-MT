# -----------------------------------------------------------------------------
# run.ps1 -- start the Arc MT stack (jarvis-api, plus the Vite dev server with
# -Dev, plus Ollama with -StartOllama) attached to this console.
#
# Colored step output, a banner, preflight
# checks, and everything supervised as one group. Ctrl+C -- or any one child
# dying on its own -- tears the whole stack down through Stop-ProcessTree, so
# nothing is left running behind you. There are no PID files; scripts\stop.ps1
# and scripts\status.ps1 find the processes by their command line / path.
#
#   scripts\run.ps1                    API serves web\build (pnpm -C web build); http://127.0.0.1:8787
#   scripts\run.ps1 -Dev               also run the Vite dev server (hot reload) on 5173
#   scripts\run.ps1 -StartOllama       start `ollama serve` as a child if Ollama is not answering
#   scripts\run.ps1 -NoBrowser         do not open the browser
#   scripts\run.ps1 -SkipChecks        skip the preflight (config, secrets, ports, build output)
#   scripts\run.ps1 -Config <path>     use another config file (default: JARVIS_CONFIG, then config\jarvis.toml)
#
# What the preflight checks, and why each one is here:
#   * config\jarvis.toml and .env exist -- the API refuses to start without them;
#   * with login enabled, JARVIS_JWT_SECRET is present and strong enough (the same
#     rules as the API: 32 bytes, 16 distinct byte values, no pattern). It is read
#     to check it and never printed;
#   * the bridge key is configured ([bridge] key_file or JARVIS_BRIDGE_KEY) -- the
#     API refuses to start without it, so say so before the server does;
#   * the API port (and 5173 with -Dev) is free;
#   * web\build exists (not -Dev), and the config has static_dir so the API serves it;
#   * a release binary exists; otherwise `cargo run --release -p jarvis-api` builds
#     and runs it (slow the first time), and a debug build is the last resort;
#   * Ollama answers (a warning only: a cloud provider may be in use).
# One check is NOT skippable: if a jarvis-api.exe is already running (or the Windows
# service owns the port) this script refuses to start a second one, because two
# servers on one SQLite database and one EA is exactly what the single-instance
# habits in this repo exist to prevent.
#
# Children: the API and (with -Dev) Vite are essential -- if either exits, everything
# stops. Ollama, when this script started it, is a convenience: if it exits the
# script warns and keeps the API up (trading does not depend on the language model).
#
# Logs: logs\api-<date>.log (+ .err), logs\web-<date>.log (+ .err) with -Dev and
# logs\ollama-<date>.log (+ .err). The API writes its structured log to stdout, so it is the
# .log file; the console here is a status view, not a replacement. A second start on the
# same day gets its own file (api-<date>-<time>.log).
# -----------------------------------------------------------------------------
[CmdletBinding()]
param(
    [switch]$Dev,
    [switch]$SkipChecks,
    [switch]$NoBrowser,
    [switch]$StartOllama,
    [string]$Config
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lib\common.ps1')

Set-WindowTitle 'starting...'
Write-Banner "$AppName"

$configPath = Resolve-ConfigPath $Config
$cfg = Get-JarvisConfig -Path $configPath
$dotenvPath = Join-Path $Root '.env'
$dotenv = Read-DotEnv $dotenvPath

# --- refuse a second instance (always, even with -SkipChecks) -----------------------------
$running = @(Get-ApiProcesses)
$service = Get-JarvisService
if ($running.Count -gt 0 -or ($service -and $service.State -eq 'Running')) {
    Write-Step 'Arc MT is already running'
    foreach ($p in $running) {
        $origin = if ($p.IsService) { 'the Windows service' } elseif ($p.InRepo) { 'this repo' } else { 'another folder' }
        Write-Fail "jarvis-api.exe pid $($p.ProcessId), started $(Format-Age $p.Started) ago ($origin)"
        Write-Hint "    $($p.CommandLine)"
    }
    if ($service -and $service.State -eq 'Running' -and -not ($running | Where-Object { $_.IsService })) {
        Write-Fail "the Windows service $ServiceName is running"
    }
    Write-Host ''
    Write-Hint 'Starting a second server would fight over the port, the database and the EA.'
    Write-Hint 'Check it with scripts\status.ps1, stop it with scripts\stop.ps1 (add -IncludeService for the service).'
    Write-Host ''
    exit 1
}

# --- preflight --------------------------------------------------------------------------
$apiPort = $cfg.Port
if (-not $SkipChecks) {
    Write-Step 'Checking prerequisites'
    $bad = $false

    if (-not $cfg.Exists) {
        Write-Fail "config file not found: $configPath"
        Write-Hint 'Create it with: Copy-Item config\jarvis.example.toml config\jarvis.toml   (or run scripts\setup.ps1)'
        $bad = $true
    } elseif ($cfg.Error) {
        Write-Fail "config file cannot be read: $($cfg.Error)"
        $bad = $true
    } else {
        Write-Ok "config $configPath (mode $($cfg.Mode), bind $($cfg.Bind), login $(if ($cfg.AuthEnabled) { 'on' } else { 'OFF' }))"
    }

    if (-not (Test-Path -LiteralPath $dotenvPath)) {
        Write-Fail '.env not found'
        Write-Hint 'Create it with: Copy-Item .env.example .env, or run scripts\setup.ps1 (it also fills the JWT secret).'
        $bad = $true
    } else {
        Write-Ok '.env found'
    }

    if ($cfg.Exists -and -not $cfg.Error) {
        if ($cfg.AuthEnabled) {
            $why = Test-JwtSecret (Get-EnvSetting 'JARVIS_JWT_SECRET' $dotenv)
            if ($why) {
                Write-Fail "JARVIS_JWT_SECRET $why"
                Write-Hint 'Run scripts\setup.ps1 (it fills an empty secret), or set a random value of 32+ bytes in .env.'
                $bad = $true
            } else {
                Write-Ok 'JARVIS_JWT_SECRET is set'
            }
        } else {
            Write-Warn 'login is DISABLED ([auth] enabled = false): anyone who can reach the port can trade. Loopback only.'
        }

        $key = Get-BridgeKeyStatus $cfg $dotenv
        if ($key.Ok) {
            Write-Ok "bridge key configured ($($key.Source))"
        } else {
            Write-Fail "bridge key: $($key.Problem)"
            Write-Hint 'scripts\install-ea.ps1 creates the key and prints the path for [bridge] key_file; scripts\setup.ps1 can write it.'
            $bad = $true
        }
    }

    $busy = @()
    if (Test-PortInUse -Port $apiPort) { $busy += $apiPort }
    if ($Dev -and (Test-PortInUse -Port $DefaultVitePort)) { $busy += $DefaultVitePort }
    if ($busy.Count -gt 0) {
        foreach ($port in $busy) {
            $owners = @(Get-PortOwners $port | ForEach-Object {
                    $info = Get-ProcessSummary $_
                    if ($info) { "$($info.Name) (pid $_)" } else { "pid $_" }
                })
            Write-Fail "port $port is already in use by $($owners -join ', ')"
        }
        Write-Hint 'Find the owner with: Get-NetTCPConnection -LocalPort <port> -State Listen | Select-Object OwningProcess'
        $bad = $true
    } else {
        $ports = if ($Dev) { "$apiPort and $DefaultVitePort" } else { "$apiPort" }
        Write-Ok "port(s) $ports free"
    }

    if (-not $Dev) {
        $buildIndex = Join-Path $Root 'web\build\index.html'
        if (-not (Test-Path -LiteralPath $buildIndex)) {
            Write-Fail 'web\build is missing -- the web UI is not built'
            Write-Hint 'Build it with: pnpm -C web build   (or run scripts\setup.ps1, or start with -Dev)'
            $bad = $true
        } elseif (-not $cfg.StaticDir) {
            Write-Warn 'web\build exists but the config has no [server] static_dir, so the API will NOT serve the web UI'
            Write-Hint 'Uncomment  static_dir = "web/build"  in the config, or start with -Dev.'
        } else {
            Write-Ok "web UI built (static_dir $($cfg.StaticDir))"
        }
    } elseif (-not (Test-Path -LiteralPath (Join-Path $Root 'web\node_modules'))) {
        Write-Fail 'web\node_modules is missing -- run: pnpm -C web install   (or scripts\setup.ps1)'
        $bad = $true
    }

    if ($bad) {
        Write-Host ''
        Write-Hint 'Fix the items above, or re-run with -SkipChecks to start anyway.'
        Write-Host ''
        exit 1
    }
}

# --- which binary ---------------------------------------------------------------------
$apiExe = Find-ApiExe -ReleaseOnly
$apiFile = $null
$apiArgs = @()
$cfgArg = "`"$configPath`""
if ($apiExe) {
    $apiFile = $apiExe
    $apiArgs = @('--config', $cfgArg)
} elseif (Get-Command cargo -ErrorAction SilentlyContinue) {
    Write-Warn 'no release binary (target\release\jarvis-api.exe): using `cargo run --release -p jarvis-api` (builds first, which can take minutes; progress goes to the .err log)'
    $apiFile = (Get-Command cargo).Source
    $apiArgs = @('run', '--release', '-p', 'jarvis-api', '--', '--config', $cfgArg)
} else {
    $apiExe = Find-ApiExe
    if (-not $apiExe) {
        Write-Fail 'no jarvis-api binary and no cargo -- build one with: cargo build --release -p jarvis-api'
        exit 1
    }
    Write-Warn "using a DEBUG build ($apiExe): slower, and it may be older than the source"
    $apiFile = $apiExe
    $apiArgs = @('--config', $cfgArg)
}

# --- Ollama (reported always, started only with -StartOllama) ---------------------------------
$ollamaState = Get-OllamaState $cfg.OllamaUrl $cfg.OllamaModel
$ollamaToStart = $null
if ($ollamaState.Reachable) {
    if ($ollamaState.HasModel) {
        Write-Ok "Ollama answers at $($cfg.OllamaUrl), model $($cfg.OllamaModel) present"
    } else {
        Write-Warn "Ollama answers, but model $($cfg.OllamaModel) is not pulled: ollama pull $($cfg.OllamaModel)"
    }
} elseif ($cfg.HasOllama -or $StartOllama) {
    if ($StartOllama) {
        $ollamaToStart = Find-Ollama
        if (-not $ollamaToStart) {
            Write-Warn 'Ollama is not installed (https://ollama.com); the local provider will be unavailable'
        }
    } else {
        Write-Warn "Ollama is not answering at $($cfg.OllamaUrl) -- the local provider will fail (start it, or use -StartOllama)"
    }
}

# --- start ------------------------------------------------------------------------------
$procs = @()    # @{ Name; Process; Log; Essential }
$childDied = $false
try {
    if ($ollamaToStart) {
        Write-Step 'Starting Ollama (ollama serve)'
        $ollamaLog = New-LogPath 'ollama'
        $ollamaProc = Start-Process -FilePath $ollamaToStart -ArgumentList @('serve') -WorkingDirectory $Root `
            -NoNewWindow -PassThru -RedirectStandardOutput $ollamaLog -RedirectStandardError "$ollamaLog.err"
        [void]$ollamaProc.Handle    # keeps the exit code readable after the process ends
        Write-Ok "ollama (pid $($ollamaProc.Id)) -> $ollamaLog"
        $procs += @{ Name = 'ollama'; Process = $ollamaProc; Log = $ollamaLog; Essential = $false }
    }

    Write-Step 'Starting the API'
    $apiLog = New-LogPath 'api'
    $apiProc = Start-Process -FilePath $apiFile -ArgumentList $apiArgs -WorkingDirectory $Root `
        -NoNewWindow -PassThru -RedirectStandardOutput $apiLog -RedirectStandardError "$apiLog.err"
    [void]$apiProc.Handle
    Write-Ok "jarvis-api (pid $($apiProc.Id)) -> $apiLog (+ .err)"
    $procs += @{ Name = 'jarvis-api'; Process = $apiProc; Log = $apiLog; Essential = $true }

    if ($Dev) {
        Write-Step 'Starting the Vite dev server'
        $pnpm = Find-Pnpm
        if (-not $pnpm) { throw 'pnpm not found (npm install --global pnpm)' }
        $webLog = New-LogPath 'web'
        $webDir = Join-Path $Root 'web'
        $webProc = Start-Process -FilePath $pnpm `
            -ArgumentList @('-C', "`"$webDir`"", 'dev', '--host', '127.0.0.1', '--port', "$DefaultVitePort", '--strictPort') `
            -WorkingDirectory $Root -NoNewWindow -PassThru -RedirectStandardOutput $webLog -RedirectStandardError "$webLog.err"
        [void]$webProc.Handle
        Write-Ok "vite (pid $($webProc.Id)) -> $webLog (+ .err)"
        $procs += @{ Name = 'vite'; Process = $webProc; Log = $webLog; Essential = $true }
    }

    # Wait for the API: poll /api/health until it answers, the process exits, or 40 s pass.
    Write-Step 'Waiting for the API'
    $deadline = (Get-Date).AddSeconds(40)
    if ($apiArgs -contains 'run') { $deadline = (Get-Date).AddMinutes(20) }    # cargo may be compiling
    $summary = $null
    while ((Get-Date) -lt $deadline) {
        if ($apiProc.HasExited) { break }
        $summary = Get-ApiSummary -BaseUrl $cfg.ApiBase -TimeoutSec 2
        if ($summary.Healthy) { break }
        Start-Sleep -Milliseconds 500
    }
    if (-not $summary -or -not $summary.Healthy) {
        $childDied = $true
        if ($apiProc.HasExited) {
            Write-Fail "jarvis-api exited with code $($apiProc.ExitCode) before it answered"
        } else {
            Write-Fail "the API did not answer on $($cfg.ApiBase) within the time limit"
        }
        foreach ($l in (Read-ChildLogTail $apiLog 8)) { Write-Hint "  $l" }
        Write-Hint "Full log: $apiLog"
        exit 1
    }
    Write-Ok "API healthy at $($cfg.ApiBase)"
    Write-ApiSummary $summary

    $url = $cfg.ApiBase
    if ($Dev) {
        $viteUrl = "http://127.0.0.1:$DefaultVitePort"
        $dead = [DateTime]::UtcNow.AddSeconds(30)
        while ([DateTime]::UtcNow -lt $dead -and -not $webProc.HasExited) {
            if ((Invoke-HttpGet $viteUrl 2).Status -gt 0) { break }
            Start-Sleep -Milliseconds 500
        }
        $url = $viteUrl
    }

    Write-Host ''
    if ($Dev) {
        Write-Host "  web UI (dev)      $url   (proxies /api to $($cfg.ApiBase))" -ForegroundColor White
        Write-Host "  API               $($cfg.ApiBase)" -ForegroundColor White
    } else {
        Write-Host "  web UI + API      $url" -ForegroundColor White
    }
    # /api/status needs a session when login is on, so the live numbers are only available with
    # login off. Otherwise: the config's values (marked), and the EA asked directly if bridge_cli
    # has been built (cargo build --release -p jarvis-bridge-client --example bridge_cli).
    if ($summary.StatusAvailable) {
        $personaText = "$($summary.Persona)"
        $modeText = "$($summary.Mode)"
        $bridgeText = if ($summary.BridgeConnected) { "connected (EA $($summary.EaVersion), trade mode $($summary.TradeMode))" } else { 'NOT connected -- start MT5 and attach JarvisBridge' }
        $voiceText = "stt=$($summary.Stt) tts=$($summary.Tts)"
    } else {
        $personaDefault = Get-TomlValue $cfg.Toml 'personas' 'default'
        $personaText = if ($personaDefault) { "$personaDefault (config default; each user picks their own)" } else { 'n/a' }
        $modeText = "$($cfg.Mode) (config)"
        $ping = Get-BridgePing $cfg
        if ($ping.Available -and $ping.Connected) { $bridgeText = "connected (EA $($ping.EaVersion), trade mode $($ping.TradeMode))" }
        elseif ($ping.Available) { $bridgeText = 'NOT connected -- start MT5 and attach JarvisBridge' }
        else { $bridgeText = 'unknown (login is on; check scripts\status.ps1 after signing in, or build the bridge_cli example)' }
        $voiceText = if ($cfg.VoiceEnabled -eq $true) { 'enabled in the config (models are checked at API start-up)' } elseif ($cfg.VoiceEnabled -eq $false) { 'off in the config' } else { 'n/a' }
    }
    Write-Host "  mode              $modeText" -ForegroundColor White
    Write-Host "  persona           $personaText" -ForegroundColor White
    Write-Host "  bridge (MT5 EA)   $bridgeText" -ForegroundColor White
    Write-Host "  voice             $voiceText" -ForegroundColor White
    Write-Host "  logs              $LogDir" -ForegroundColor DarkGray

    if (-not $NoBrowser) {
        try { Start-Process $url } catch { Write-Warn "could not open the browser: $($_.Exception.Message)" }
    }

    Write-Host ''
    Write-Host '  Press Ctrl+C to stop everything.' -ForegroundColor DarkGray
    Write-Host ''
    Set-WindowTitle "running ($($procs.Count) processes)"

    $warned = @{}
    while ($true) {
        foreach ($p in $procs) {
            if (-not $p.Process.HasExited) { continue }
            if ($p.Essential) {
                $childDied = $true
                Write-Host ''
                Write-Fail "$($p.Name) stopped (exit code $($p.Process.ExitCode)). Shutting down everything else."
                if ($p.Log) { foreach ($l in (Read-ChildLogTail $p.Log 6)) { Write-Hint "  $l" } }
                break
            }
            if (-not $warned.ContainsKey($p.Name)) {
                $warned[$p.Name] = $true
                Write-Warn "$($p.Name) exited (code $($p.Process.ExitCode)) -- the API keeps running; the local model is unavailable until it is back"
            }
        }
        if ($childDied) { break }
        Start-Sleep -Milliseconds 500
    }
} finally {
    Write-Host ''
    Write-Step 'Stopping everything'
    Set-WindowTitle 'stopping...'
    # After Ctrl+C the console already sent the signal to every child, and the API shuts down
    # gracefully (open sockets closed, in-flight requests finished). Give it a moment before
    # forcing anything. When a child died on its own, nobody signalled the others: stop them now.
    if (-not $childDied) {
        foreach ($p in $procs) {
            try { [void]$p.Process.WaitForExit(8000) } catch { }
        }
    }
    foreach ($p in $procs) {
        if (-not $p.Process.HasExited) { Stop-ProcessTree -TargetId $p.Process.Id }
    }
    Write-Ok 'Stopped'
    Set-WindowTitle 'stopped'
    Write-Host ''
}
