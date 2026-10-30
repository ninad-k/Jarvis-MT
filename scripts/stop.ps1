# -----------------------------------------------------------------------------
# stop.ps1 -- find and stop every Arc MT process of THIS repository, by command
# line / path rather than a PID file.
#
# run.ps1 normally stops everything itself on Ctrl+C. This script is the fallback
# for when that did not happen: a closed console, a hidden run.ps1 from the logon
# task, or a stray jarvis-api / Vite left over from a crash. Command-line and
# path matching finds those however and whenever they started, which a PID file
# never could.
#
# What it stops (in this order, public exposure first):
#   1. the Cloudflare quick tunnel started by remote-start.ps1 (recognised by its
#      --logfile path inside this repo's .local\ folder; no other cloudflared is touched)
#   2. Vite / pnpm / node / esbuild dev-server processes whose command line points
#      into this repo's web\ folder
#   3. jarvis-api.exe processes whose executable lives under this repo (target\,
#      target-voice\, ...), including their cargo parent
#
# What it never stops:
#   * Ollama, unless -IncludeOllama (it is shared with other tools);
#   * the Windows service JarvisMT, unless -IncludeService (then it runs
#     `jarvis-api service stop`, which needs an elevated prompt). If the service
#     is installed and running and you did not ask, the script says so and moves on;
#   * MetaTrader 5, and any process of another project.
#
#   scripts\stop.ps1 -WhatIf                list what it would stop, change nothing
#   scripts\stop.ps1                        stop the tunnel, dev server and API of this repo
#   scripts\stop.ps1 -IncludeOllama         also stop Ollama
#   scripts\stop.ps1 -IncludeService        also stop the Windows service (elevated)
# -----------------------------------------------------------------------------
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [switch]$IncludeOllama,
    [switch]$IncludeService
)

$ErrorActionPreference = 'Continue'
. (Join-Path $PSScriptRoot 'lib\common.ps1')

Write-Banner "$AppName - stop"

$stopped = 0
$listed = 0

function Stop-Listed {
    param([string]$Label, $Process)
    $script:listed++
    $pidText = "pid $($Process.ProcessId)"
    Write-Host "    $Label ($pidText)" -ForegroundColor White
    if ($Process.CommandLine) { Write-Hint "  $($Process.CommandLine)" }
    if ($PSCmdlet.ShouldProcess("$Label ($pidText)", 'Stop process tree')) {
        Stop-ProcessTree -TargetId $Process.ProcessId
        if (-not (Wait-ProcessGone $Process.ProcessId)) {
            Write-Fail "  $pidText could not be stopped (access denied? try an elevated prompt)"
        } else {
            $script:stopped++
            Write-Ok "  stopped"
        }
    }
}

# --- 1. the remote tunnel -----------------------------------------------------------------
Write-Step 'Remote tunnel (cloudflared started by remote-start.ps1)'
$tunnels = @(Get-RemoteTunnelProcesses)
if ($tunnels.Count -eq 0) { Write-Hint 'none running' }
foreach ($t in $tunnels) { Stop-Listed 'cloudflared quick tunnel' $t }
if ($tunnels.Count -gt 0 -and $PSCmdlet.ShouldProcess($RemoteUrlFile, 'Remove the saved tunnel URL')) {
    if (Test-Path -LiteralPath $RemoteUrlFile) { Remove-Item -LiteralPath $RemoteUrlFile -Force -ErrorAction SilentlyContinue }
}

# --- 2. dev server ------------------------------------------------------------------------
Write-Step 'Web dev server (Vite)'
$dev = @(Get-DevServerProcesses)
if ($dev.Count -eq 0) { Write-Hint 'none running' }
# Parents first: stopping a parent's tree takes its children, which then simply no longer exist.
foreach ($p in ($dev | Sort-Object ProcessId)) {
    if (-not (Get-Process -Id $p.ProcessId -ErrorAction SilentlyContinue) -and -not $WhatIfPreference) { continue }
    Stop-Listed "$($p.Name)" $p
}

# --- 3. the API -----------------------------------------------------------------------------
Write-Step 'API (jarvis-api.exe under this repository)'
$api = @(Get-ApiProcesses)
$mine = @($api | Where-Object { $_.InRepo -and -not $_.IsService })
$serviceProc = @($api | Where-Object { $_.IsService })
$foreign = @($api | Where-Object { -not $_.InRepo -and -not $_.IsService })
if ($mine.Count -eq 0) { Write-Hint 'none running' }
foreach ($p in $mine) { Stop-Listed 'jarvis-api.exe' $p }
foreach ($p in $foreign) {
    Write-Hint "left alone: jarvis-api.exe pid $($p.ProcessId) runs from outside this repo ($($p.Path))"
}

# --- the Windows service ----------------------------------------------------------------------
Write-Step "Windows service $ServiceName"
$svc = Get-JarvisService
if (-not $svc) {
    Write-Hint 'not installed'
} elseif ($svc.State -ne 'Running') {
    Write-Hint "installed, state $($svc.State) -- nothing to stop"
} elseif (-not $IncludeService) {
    Write-Warn "installed and RUNNING (pid $($svc.ProcessId)) -- left alone. Use -IncludeService to stop it (elevated prompt)."
} else {
    $listed++
    $exe = $null
    if ($svc.PathName -match '^"([^"]+)"') { $exe = $Matches[1] } elseif ($svc.PathName -match '^(\S+)') { $exe = $Matches[1] }
    if (-not $exe -or -not (Test-Path -LiteralPath $exe)) { $exe = Find-ApiExe }
    Write-Host "    service $ServiceName (pid $($svc.ProcessId))" -ForegroundColor White
    if (-not $exe) {
        Write-Fail 'cannot find jarvis-api.exe to run `service stop`'
    } elseif ($PSCmdlet.ShouldProcess("service $ServiceName", "$exe service stop")) {
        if (-not (Test-IsAdministrator)) {
            Write-Fail 'stopping a service needs an elevated (Administrator) prompt'
        } else {
            & $exe service stop
            if ($LASTEXITCODE -eq 0) { $stopped++; Write-Ok 'stop requested (check: Get-Service JarvisMT)' }
            else { Write-Fail "`service stop` exited with code $LASTEXITCODE" }
        }
    }
}

# --- Ollama --------------------------------------------------------------------------------------
Write-Step 'Ollama'
$ollama = @(Get-CimInstance Win32_Process -Filter "Name='ollama.exe' OR Name='ollama app.exe'" -ErrorAction SilentlyContinue)
if ($ollama.Count -eq 0) {
    Write-Hint 'not running'
} elseif (-not $IncludeOllama) {
    Write-Hint "running (pid $(($ollama | ForEach-Object { $_.ProcessId }) -join ', ')) -- left alone. Use -IncludeOllama to stop it."
} else {
    foreach ($p in ($ollama | Sort-Object ProcessId)) {
        if (-not (Get-Process -Id $p.ProcessId -ErrorAction SilentlyContinue) -and -not $WhatIfPreference) { continue }
        Stop-Listed $p.Name $p
    }
}

Write-Host ''
if ($WhatIfPreference) {
    Write-Host "  -WhatIf run: $listed item(s) listed, nothing was stopped." -ForegroundColor Yellow
} else {
    Write-Ok "done: $stopped stopped"
}
Write-Hint '(MT5, Ollama without -IncludeOllama, the Windows service without -IncludeService and other projects are untouched)'
Write-Host ''
