#Requires -Version 5.1
<#
.SYNOPSIS
  Compiles the installed JarvisBridge EA with the MetaEditor command line.

.DESCRIPTION
  Runs MetaEditor64.exe /compile:<installed .mq5> /log:<logfile>, waits for it, prints the
  (UTF-16) log and exits non-zero unless the result is "0 errors".
  Warnings raised inside our own code also fail the run (use -AllowWarnings to ignore them).
  Warnings inside the vendored mql-zmq / mql4-lib headers are listed but never fail the run.

  Run scripts\install-ea.ps1 first: this compiles the copy inside the terminal data folder.

.PARAMETER DataFolder
  MT5 data folder (or its 32-character id). Default: the only terminal data folder under
  %APPDATA%\MetaQuotes\Terminal. With several terminals installed the script asks for it.

.PARAMETER MetaEditor
  Path to MetaEditor64.exe.

.PARAMETER Source
  Path to the .mq5 to compile. Default: <DataFolder>\MQL5\Experts\JarvisMT\JarvisBridge.mq5

.PARAMETER LogFile
  Where MetaEditor writes its log. Default: a file in %TEMP%.
#>
[CmdletBinding()]
param(
    [Alias('Terminal')]
    [string]$DataFolder,
    [string]$MetaEditor = 'C:\Program Files\MetaTrader 5\MetaEditor64.exe',
    [string]$Source,
    [string]$LogFile = (Join-Path ([IO.Path]::GetTempPath()) 'jarvis-compile.log'),
    [switch]$AllowWarnings
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# No -DataFolder: use the only MT5 terminal data folder on this PC, or ask which one.
if (-not $DataFolder) {
    $terminalsRoot = Join-Path $env:APPDATA 'MetaQuotes\Terminal'
    $found = @(Get-ChildItem -LiteralPath $terminalsRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'MQL5') })
    if ($found.Count -ne 1) {
        $names = ($found | ForEach-Object { $_.Name }) -join ', '
        if (-not $names) { $names = 'none' }
        throw "Pass -DataFolder <data folder or 32-character id> (File > Open Data Folder in MT5). Terminals found: $names"
    }
    $DataFolder = $found[0].FullName
}

if ($DataFolder -match '^[0-9A-Fa-f]{32}$') {
    $DataFolder = Join-Path (Join-Path $env:APPDATA 'MetaQuotes\Terminal') $DataFolder
}
$mqlRoot = Join-Path $DataFolder 'MQL5'
if (-not $Source) { $Source = Join-Path $mqlRoot 'Experts\JarvisMT\JarvisBridge.mq5' }

if (-not (Test-Path -LiteralPath $MetaEditor)) { throw "MetaEditor not found: $MetaEditor" }
if (-not (Test-Path -LiteralPath $Source)) { throw "Source not found: $Source (run scripts\install-ea.ps1 first)" }

if (Test-Path -LiteralPath $LogFile) { Remove-Item -LiteralPath $LogFile -Force }
$startedAt = Get-Date

# MetaEditor wants /key:"value" as one token.
$argList = @("/compile:`"$Source`"", "/inc:`"$mqlRoot`"", "/log:`"$LogFile`"")
Write-Host "MetaEditor64 $($argList -join ' ')"
$proc = Start-Process -FilePath $MetaEditor -ArgumentList $argList -Wait -PassThru -WindowStyle Hidden

# The log can land a moment after the process exits.
$deadline = (Get-Date).AddSeconds(15)
while (-not (Test-Path -LiteralPath $LogFile) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
if (-not (Test-Path -LiteralPath $LogFile)) {
    Write-Error "MetaEditor produced no log file ($LogFile). Exit code $($proc.ExitCode)."
    exit 2
}

# MetaEditor writes UTF-16 LE with BOM; fall back to UTF-8 if there is no BOM.
$bytes = [IO.File]::ReadAllBytes($LogFile)
if ($bytes.Length -ge 2 -and $bytes[0] -eq 0xFF -and $bytes[1] -eq 0xFE) {
    $text = [Text.Encoding]::Unicode.GetString($bytes, 2, $bytes.Length - 2)
} elseif ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
    $text = [Text.Encoding]::UTF8.GetString($bytes, 3, $bytes.Length - 3)
} else {
    $text = [Text.Encoding]::UTF8.GetString($bytes)
}
$lines = $text -split "\r?\n"
$lines | Where-Object { $_.Trim() -ne '' -and $_ -notmatch ': information: (including|compiling|generating code|code generated)' } |
    ForEach-Object { Write-Host $_.Replace($mqlRoot + '\', '') }

if ($text -notmatch '(?m)Result:\s*(\d+)\s+errors?,\s*(\d+)\s+warnings?') {
    Write-Error 'Could not find the "Result: N errors, M warnings" line in the log.'
    exit 2
}
$errors = [int]$Matches[1]
$warnings = [int]$Matches[2]

# Split warnings into vendored (ignored) and ours.
$vendored = '\\Include\\(Zmq|Mql)\\'
$ourWarnings = @()
$vendoredWarnings = @()
foreach ($line in $lines) {
    if ($line -match '\)\s*:\s*warning\b') {
        if ($line -match $vendored) { $vendoredWarnings += $line } else { $ourWarnings += $line }
    }
}

Write-Host ''
Write-Host "Summary: $errors errors, $warnings warnings ($($ourWarnings.Count) in our code, $($vendoredWarnings.Count) in vendored headers)"

if ($errors -ne 0) { Write-Host 'FAILED: compile errors.'; exit 1 }
if ($ourWarnings.Count -gt 0 -and -not $AllowWarnings) {
    Write-Host 'FAILED: warnings in our code (use -AllowWarnings to ignore).'
    exit 1
}
$ex5 = [IO.Path]::ChangeExtension($Source, '.ex5')
if (-not (Test-Path -LiteralPath $ex5) -or (Get-Item -LiteralPath $ex5).LastWriteTime -lt $startedAt.AddSeconds(-2)) {
    Write-Host "FAILED: no fresh .ex5 produced at $ex5"
    exit 1
}
Write-Host "OK: $ex5"
exit 0
