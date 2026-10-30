# -----------------------------------------------------------------------------
# status.ps1 -- fast health check of the Arc MT stack. Read-only: it starts and
# stops nothing and writes nothing.
#
# Reports, in this order:
#   * the API: /api/health, then /api/status (mode, mode_mismatch, kill switch,
#     persona, bridge connected + EA version + trade mode, voice stt/tts, login on
#     or off, language-model providers). When login is on, /api/status needs a
#     session, so only the health answer is available here -- the web UI shows the rest;
#   * the processes of this repo (jarvis-api, the Vite dev server, the remote
#     tunnel) with PID and start time, found by command line / path;
#   * the listening ports 8787 (API), 5173 (Vite), 5555 (EA bridge), 11434 (Ollama);
#   * Ollama: reachable, and whether the configured model is pulled;
#   * the Windows service JarvisMT, if installed;
#   * the remote tunnel URL, if remote-start.ps1 is running.
#
# Exit code: 0 when the API is healthy, 1 otherwise -- usable from monitors and
# from the logon task. -Json prints one JSON document instead (same exit code);
# nothing secret is in it.
#
#   scripts\status.ps1                 human-readable
#   scripts\status.ps1 -Json           machine-readable
#   scripts\status.ps1 -Config <path>  check against another config file (bind address, Ollama model)
# -----------------------------------------------------------------------------
[CmdletBinding()]
param(
    [switch]$Json,
    [string]$Config
)

$ErrorActionPreference = 'Continue'
. (Join-Path $PSScriptRoot 'lib\common.ps1')

$configPath = Resolve-ConfigPath $Config
$cfg = Get-JarvisConfig -Path $configPath
$api = Get-ApiSummary -BaseUrl $cfg.ApiBase -TimeoutSec 3

# processes
$procs = @()
foreach ($p in (Get-ApiProcesses)) {
    $where = if ($p.IsService) { 'service' } elseif ($p.InRepo) { 'this repo' } else { 'other folder' }
    $procs += [pscustomobject]@{ name = 'jarvis-api'; pid = $p.ProcessId; started = $p.Started; origin = $where; path = $p.Path }
}
foreach ($p in (Get-DevServerProcesses | Where-Object { $_.Name -eq 'node.exe' -and $_.CommandLine -match 'vite' })) {
    $procs += [pscustomobject]@{ name = 'vite'; pid = $p.ProcessId; started = $p.Started; origin = 'this repo'; path = $null }
}
$tunnelProcs = @(Get-RemoteTunnelProcesses)
foreach ($p in $tunnelProcs) {
    $procs += [pscustomobject]@{ name = 'cloudflared'; pid = $p.ProcessId; started = $p.Started; origin = 'this repo'; path = $null }
}

# ports
$portDefs = @(
    @{ port = $cfg.Port; role = 'API' },
    @{ port = $DefaultVitePort; role = 'Vite dev server' },
    @{ port = $cfg.BridgePort; role = 'EA bridge' },
    @{ port = 11434; role = 'Ollama' }
)
$ports = @()
foreach ($d in $portDefs) {
    $owners = @(Get-PortOwners $d.port)
    $listening = (($owners.Count -gt 0) -or (Test-TcpConnect -Port $d.port))
    $names = @($owners | ForEach-Object { $i = Get-ProcessSummary $_; if ($i) { $i.Name } else { "pid $_" } } | Sort-Object -Unique)
    if ($listening -and $names.Count -eq 0) { $names = @('owner not visible') }
    $ports += [pscustomobject]@{
        port = $d.port; role = $d.role; listening = $listening
        pids = $owners; process = ($names -join ', ')
    }
}

# Ollama
$ollama = Get-OllamaState $cfg.OllamaUrl $cfg.OllamaModel

# Windows service
$svc = Get-JarvisService
$service = [pscustomobject]@{
    installed = [bool]$svc; state = $(if ($svc) { $svc.State } else { $null })
    start_mode = $(if ($svc) { $svc.StartMode } else { $null }); account = $(if ($svc) { $svc.StartName } else { $null })
}

# remote tunnel
$remoteUrl = $null
if ($tunnelProcs.Count -gt 0 -and (Test-Path -LiteralPath $RemoteUrlFile)) {
    $remoteUrl = (Get-Content -LiteralPath $RemoteUrlFile -TotalCount 1 -ErrorAction SilentlyContinue)
}
$tunnel = [pscustomobject]@{ running = ($tunnelProcs.Count -gt 0); url = $remoteUrl }

$healthy = [bool]$api.Healthy

# With login on, /api/status needs a session. If the bridge_cli example has been built, ask the EA
# directly (a read-only ping) so the bridge line is still useful.
$ping = $null
if ($api.Healthy -and $api.LoginRequired) {
    $ping = Get-BridgePing $cfg
    if ($ping.Available -and $ping.Connected) {
        $api.BridgeConnected = $true; $api.EaVersion = $ping.EaVersion; $api.TradeMode = $ping.TradeMode
    } elseif ($ping.Available) {
        $api.BridgeConnected = $false; $api.BridgeError = $ping.Error
    }
}

if ($Json) {
    $doc = [ordered]@{
        healthy = $healthy
        config  = [ordered]@{ path = $configPath; exists = $cfg.Exists; bind = $cfg.Bind; mode = $cfg.Mode }
        api     = [ordered]@{
            url = $api.BaseUrl; healthy = $api.Healthy; health_error = $api.HealthError
            status_available = $api.StatusAvailable; login_required = $api.LoginRequired
            version = $api.Version; mode = $api.Mode; mode_mismatch = $api.ModeMismatch
            kill_switch = [ordered]@{ engaged = $api.KillSwitchEngaged; reason = $api.KillSwitchReason }
            persona = $api.Persona
            bridge = [ordered]@{
                connected = $api.BridgeConnected; ea_version = $api.EaVersion; trade_mode = $api.TradeMode
                terminal_connected = $api.TerminalConnected; error = $api.BridgeError
            }
            voice = [ordered]@{ stt = $api.Stt; tts = $api.Tts }
            auth_enabled = $api.AuthEnabled
            llm = [ordered]@{ default = $api.DefaultProvider; providers = @($api.Providers) }
        }
        processes = @($procs)
        ports     = @($ports)
        ollama    = [ordered]@{ url = $ollama.Url; reachable = $ollama.Reachable; model = $ollama.Model; model_present = $ollama.HasModel; model_count = $ollama.ModelCount }
        service   = $service
        remote_tunnel = $tunnel
    }
    $doc | ConvertTo-Json -Depth 6
    if ($healthy) { exit 0 } else { exit 1 }
}

Write-Banner "$AppName - status"

Write-Step "API  $($api.BaseUrl)"
Write-ApiSummary $api
if ($ping -and $ping.Available) {
    if ($ping.Connected) { Write-Ok "EA answers a direct ping (EA $($ping.EaVersion), trade mode $($ping.TradeMode))" }
    else { Write-Warn 'EA does not answer a direct ping -- MT5 is closed or JarvisBridge is not attached' }
}
if ($api.StatusAvailable) {
    $avail = @($api.Providers | Where-Object { $_.available } | ForEach-Object { "$($_.name) ($($_.model))" })
    $off = @($api.Providers | Where-Object { -not $_.available } | ForEach-Object { $_.name })
    if ($avail.Count -gt 0) { Write-Ok "language models available: $($avail -join ', ') (default $($api.DefaultProvider))" }
    else { Write-Warn 'no language-model provider is available' }
    if ($off.Count -gt 0) { Write-Hint "unavailable: $($off -join ', ')" }
    if ($api.Version) { Write-Hint "jarvis-api $($api.Version)" }
}

Write-Step 'Processes (this repository)'
if ($procs.Count -eq 0) { Write-Hint 'none running' }
foreach ($p in $procs) {
    $started = if ($p.started) { ([datetime]$p.started).ToString('yyyy-MM-dd HH:mm:ss') } else { 'unknown' }
    Write-Host ("    {0,-12} pid {1,-7} started {2} ({3} ago)  [{4}]" -f $p.name, $p.pid, $started, (Format-Age $p.started), $p.origin) -ForegroundColor Green
    if ($p.path) { Write-Hint "  $($p.path)" }
}

Write-Step 'Ports'
foreach ($p in $ports) {
    if ($p.listening) {
        Write-Host ("    {0,-6} {1,-16} listening ({2})" -f $p.port, $p.role, $p.process) -ForegroundColor Green
    } else {
        Write-Host ("    {0,-6} {1,-16} not listening" -f $p.port, $p.role) -ForegroundColor DarkGray
    }
}

Write-Step 'Ollama'
if (-not $ollama.Reachable) {
    Write-Warn "not reachable at $($ollama.Url) (start it, or run.ps1 -StartOllama)"
} elseif ($ollama.HasModel) {
    Write-Ok "reachable, model $($ollama.Model) present ($($ollama.ModelCount) model(s) pulled)"
} else {
    Write-Warn "reachable, but model $($ollama.Model) is NOT pulled: ollama pull $($ollama.Model)"
}

Write-Step "Windows service $ServiceName"
if (-not $service.installed) {
    Write-Hint 'not installed'
} else {
    Write-Host "    $($service.state) (start mode $($service.start_mode), account $($service.account))" -ForegroundColor $(if ($service.state -eq 'Running') { 'Green' } else { 'Yellow' })
}

Write-Step 'Remote tunnel (phone access)'
if (-not $tunnel.running) {
    Write-Hint 'not running (scripts\remote-start.ps1)'
    if (Test-Path -LiteralPath $RemoteUrlFile) { Write-Warn "a stale $RemoteUrlFile exists -- the tunnel it names is gone" }
} elseif ($tunnel.url) {
    Write-Warn "RUNNING -- $($tunnel.url)  (public; protected by login + authenticator code only)"
} else {
    Write-Warn 'RUNNING (URL not known yet)'
}

Write-Host ''
if ($healthy) { Write-Ok 'API healthy' } else { Write-Fail 'API NOT healthy' }
Write-Host ''
if ($healthy) { exit 0 } else { exit 1 }
