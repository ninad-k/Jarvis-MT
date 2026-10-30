# -----------------------------------------------------------------------------
# remote-stop.ps1 -- close the Cloudflare quick tunnel started by remote-start.ps1,
# from another window.
#
# Only that tunnel is stopped. It is found by its command line (the --logfile argument
# points into this repo's .local\ folder), so a crashed or closed remote-start.ps1 window
# is cleaned up too, and no other cloudflared on the machine -- another project's tunnels, say
# -- can match. The API, the web UI, Ollama and MT5 are left running; if you need to cut the
# public address off in a hurry, this is the script, and the kill switch in the web UI is
# the companion step (it blocks every trade write).
#
#   scripts\remote-stop.ps1            close the tunnel
#   scripts\remote-stop.ps1 -WhatIf    show what it would stop
# -----------------------------------------------------------------------------
[CmdletBinding(SupportsShouldProcess = $true)]
param()

$ErrorActionPreference = 'Continue'
. (Join-Path $PSScriptRoot 'lib\common.ps1')

$found = 0
foreach ($t in @(Get-RemoteTunnelProcesses)) {
    $found++
    if ($PSCmdlet.ShouldProcess("cloudflared tunnel (pid $($t.ProcessId))", 'Stop process tree')) {
        Stop-ProcessTree -TargetId $t.ProcessId
        if (-not (Wait-ProcessGone $t.ProcessId)) {
            Write-Host "remote tunnel (pid $($t.ProcessId)) could not be stopped" -ForegroundColor Red
        } else {
            Write-Host "stopped remote tunnel (pid $($t.ProcessId))" -ForegroundColor Green
        }
    }
}

if (Test-Path -LiteralPath $RemoteUrlFile) {
    if ($PSCmdlet.ShouldProcess($RemoteUrlFile, 'Remove the saved tunnel address')) {
        Remove-Item -LiteralPath $RemoteUrlFile -Force -ErrorAction SilentlyContinue
    }
}

if ($found -eq 0) { Write-Host 'the remote tunnel is not running' }
Write-Host 'done. (the API and everything else keep running)'
