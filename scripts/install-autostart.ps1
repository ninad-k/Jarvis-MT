# -----------------------------------------------------------------------------
# install-autostart.ps1 -- start Arc MT when you log on to Windows, by registering
# a Task Scheduler task named "Arc MT" that runs scripts\run.ps1 (hidden, with
# -NoBrowser -StartOllama).
#
# Logon task or Windows service? Pick ONE of the two.
#
#   Logon task (this script)               Windows service (`jarvis-api service install`)
#   ------------------------------------   ----------------------------------------------
#   runs in YOUR desktop session, next     runs at boot with no one logged in, as the
#   to MT5 (a GUI program) and Ollama's    virtual account NT SERVICE\JarvisMT, which has
#   tray app; sees your files with your    no rights until you grant them (icacls on the
#   own rights, so there are no ACLs to    config, .env, data\, logs\, models\, web\build
#   set up                                 and the bridge key -- see docs\windows-service.md)
#   starts after you sign in; the API      needs no sign-in for the API itself, but MT5
#   dies at sign-out                       (and so the EA) still needs a signed-in user
#   starts Ollama for you (-StartOllama)   cannot start Ollama's per-user tray app
#   best for a PC you use                  best for a VPS or a box that reboots unattended
#
# Because both would start the same server on the same port, this script REFUSES to
# register the task while the Windows service is installed, unless -Force.
#
# The task:
#   * Trigger: at logon of THIS user, delayed 30 s so the desktop, the network and
#     MT5's own autostart are up first.
#   * Principal: the current user, interactive logon, limited (non-elevated) token.
#     No password is stored and no administrator rights are needed.
#   * Window: hidden. run.ps1 is written for an attached console, so under the task
#     its console output is not shown. The logs are in logs\ and scripts\status.ps1
#     reports the live state.
#   * Time limit: removed. Task Scheduler's default stops a task after 72 hours, which
#     would silently stop the stack.
#   * One instance: a second logon or a manual start cannot run a second copy
#     (run.ps1 also refuses to start while a jarvis-api is already running).
#   * Scope: this script registers or removes only its own task and never touches
#     config\ or .env.
#
# MT5 is not started by this task. Put a shortcut to terminal64.exe in your Startup
# folder (or its own logon task) so the EA is attached after a sign-in.
#
#   scripts\install-autostart.ps1 -WhatIf     show the task it would register
#   scripts\install-autostart.ps1             register the task (an existing one is reported, not duplicated)
#   scripts\install-autostart.ps1 -Remove     unregister the task
#   scripts\install-autostart.ps1 -NoOllama   leave -StartOllama out of the command
#   scripts\install-autostart.ps1 -Config <path>   pass a config file to run.ps1
# -----------------------------------------------------------------------------
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$TaskName = 'Arc MT',
    [switch]$Remove,
    [switch]$Force,
    [switch]$NoOllama,
    [string]$Config
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lib\common.ps1')
Import-ModuleQuiet ScheduledTasks

$RunScript = Join-Path $PSScriptRoot 'run.ps1'
$UserId = "$env:USERDOMAIN\$env:USERNAME"

# Finds this task by its name, and also by any task whose action already starts this repo's
# run.ps1 under another name -- a second registration of the same stack would race the first
# for the same port.
function Find-ExistingTasks {
    $found = @()
    foreach ($task in @(Get-ScheduledTask -ErrorAction SilentlyContinue)) {
        $actionText = ($task.Actions | ForEach-Object { [string]$_.Arguments }) -join ' '
        if ($task.TaskName -eq $TaskName -or $actionText.Contains($RunScript)) { $found += $task }
    }
    return $found
}

if ($Remove) {
    $existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if (-not $existing) {
        Write-Host "no task named '$TaskName' -- nothing to remove"
        exit 0
    }
    if ($PSCmdlet.ShouldProcess($TaskName, 'Unregister scheduled task')) {
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Write-Host "removed task '$TaskName'" -ForegroundColor Green
        Write-Host 'a running stack is not stopped by this: use scripts\stop.ps1'
    }
    exit 0
}

if (-not (Test-Path -LiteralPath $RunScript)) {
    Write-Host "run.ps1 not found at $RunScript -- is this the Arc MT checkout?" -ForegroundColor Red
    exit 1
}

# One mechanism or the other, never both.
$svc = Get-JarvisService
if ($svc -and -not $Force) {
    Write-Host "the Windows service $ServiceName is installed (state $($svc.State)) -- refusing to add a logon task as well." -ForegroundColor Red
    Write-Host 'Both would start the same server on the same port. Choose one:'
    Write-Host '    keep the service   -> nothing to do here (see docs\windows-service.md)'
    Write-Host '    use the logon task -> jarvis-api service uninstall (elevated), then run this script again'
    Write-Host 'Use -Force to register the task anyway (run.ps1 will then refuse to start while the service runs).'
    exit 1
}
if ($svc -and $Force) {
    Write-Host "warning: the Windows service $ServiceName is installed; -Force registers the task anyway" -ForegroundColor Yellow
}

$existing = @(Find-ExistingTasks)
if ($existing.Count -gt 0) {
    Write-Host 'already registered -- nothing was changed:' -ForegroundColor Yellow
    foreach ($task in $existing) {
        $action = ($task.Actions | ForEach-Object { "$($_.Execute) $($_.Arguments)" }) -join '; '
        Write-Host "    task      $($task.TaskPath)$($task.TaskName)  (state: $($task.State))"
        Write-Host "    action    $action" -ForegroundColor DarkGray
    }
    Write-Host 'To re-register with the current definition, run scripts\install-autostart.ps1 -Remove first, then run it again.'
    exit 0
}

$cfg = Get-JarvisConfig -Path (Resolve-ConfigPath $Config)
if (-not $cfg.Exists) {
    Write-Host "warning: no config file at $($cfg.Path) yet -- the task will fail at logon until scripts\setup.ps1 has run" -ForegroundColor Yellow
}
if (-not (Find-ApiExe -ReleaseOnly)) {
    Write-Host 'warning: no release build (target\release\jarvis-api.exe) -- at logon run.ps1 would have to build one first' -ForegroundColor Yellow
}

$runArgs = "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$RunScript`" -NoBrowser"
if (-not $NoOllama) { $runArgs += ' -StartOllama' }
if ($Config) { $runArgs += " -Config `"$(Resolve-ConfigPath $Config)`"" }

Write-Host "task name   $TaskName"
Write-Host "trigger     at logon of $UserId, after a 30 s delay"
Write-Host "runs as     $UserId (interactive, limited -- no password stored, no elevation)"
Write-Host "command     $PowerShellExe $runArgs"
Write-Host "directory   $Root"
Write-Host 'time limit  none (Task Scheduler''s 72 h default is removed)'
Write-Host 'instances   one (a second start is ignored)'

$action = New-ScheduledTaskAction -Execute $PowerShellExe -Argument $runArgs -WorkingDirectory $Root
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $UserId
$trigger.Delay = 'PT30S'
$principal = New-ScheduledTaskPrincipal -UserId $UserId -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet `
    -ExecutionTimeLimit ([TimeSpan]::Zero) `
    -MultipleInstances IgnoreNew `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable

if ($PSCmdlet.ShouldProcess($TaskName, 'Register logon task that runs scripts\run.ps1')) {
    try {
        Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger `
            -Principal $principal -Settings $settings `
            -Description 'Arc MT: start scripts\run.ps1 at logon (scripts\install-autostart.ps1)' | Out-Null
    } catch {
        Write-Host "could not register '$TaskName': $($_.Exception.Message)" -ForegroundColor Red
        exit 1
    }
    Write-Host "registered task '$TaskName'" -ForegroundColor Green
    Write-Host "check it with:   Get-ScheduledTask -TaskName '$TaskName'"
    Write-Host "start it now:    Start-ScheduledTask -TaskName '$TaskName'"
    Write-Host 'undo it with:    scripts\install-autostart.ps1 -Remove'
    Write-Host 'MT5 is not started by this task: add terminal64.exe to your Startup folder so the EA is attached after sign-in.'
}
