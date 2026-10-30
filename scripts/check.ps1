#Requires -Version 5.1
<#
.SYNOPSIS
  Runs every local check for Jarvis MT and prints a summary table.

.DESCRIPTION
  Mirrors the CI workflows (.github/workflows): Rust (fmt, clippy, tests, optional whisper
  feature, desktop shell), web (check, unit tests, build, end-to-end tests) and security
  (cargo deny, cargo audit, pnpm audit, gitleaks on the working tree and the git history,
  semgrep), and a syntax check of every script under scripts\ in Windows PowerShell 5.1.
  Exits with a non-zero code when any step fails. A step that cannot run because a
  tool is missing counts as a failure, except for the whisper build, which is skipped (with
  the reason) when CMake or LLVM is not installed.

  The working-tree scan covers tracked and untracked files that are not git-ignored, so a
  local .env file is not scanned.

.PARAMETER SkipWeb
  Skip the web app steps (install, check, tests, build, end-to-end tests).

.PARAMETER SkipSecurity
  Skip cargo deny, cargo audit, pnpm audit, gitleaks and semgrep.

.PARAMETER SkipWhisper
  Skip the build and tests of the optional `whisper` feature of jarvis-voice (needs CMake and LLVM).

.PARAMETER SkipDesktop
  Skip the Tauri desktop shell (only runs when apps/desktop/src-tauri exists).

.EXAMPLE
  pwsh scripts/check.ps1

.EXAMPLE
  pwsh scripts/check.ps1 -SkipWeb -SkipSecurity -SkipWhisper   # quick Rust-only run
#>
[CmdletBinding()]
param(
    [switch]$SkipWeb,
    [switch]$SkipSecurity,
    [switch]$SkipWhisper,
    [switch]$SkipDesktop
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Continue'

$Root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $Root

# Advisory ignored in CI too (.github/workflows/security.yml): rsa is in Cargo.lock only through
# sqlx's optional MySQL driver and is not compiled into any Jarvis MT binary.
$AuditIgnore = @('--ignore', 'RUSTSEC-2023-0071')

$script:Results = New-Object System.Collections.Generic.List[object]
$script:TempDirs = New-Object System.Collections.Generic.List[string]

# --- helpers ---------------------------------------------------------------------------------

function Find-Tool {
    param([string]$Name, [string[]]$Fallbacks = @())
    $cmd = Get-Command $Name -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($cmd) { return $cmd.Source }
    foreach ($pattern in $Fallbacks) {
        $hit = Get-ChildItem -Path $pattern -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($hit) { return $hit.FullName }
    }
    return $null
}

# Runs a native command and throws when it exits with a non-zero code.
function Invoke-Native {
    param([string]$Exe, [string[]]$ArgList = @())
    & $Exe @ArgList
    if ($LASTEXITCODE -ne 0) {
        throw "'$Exe $($ArgList -join ' ')' exited with code $LASTEXITCODE"
    }
}

# Thrown to mark a step as skipped rather than failed.
function Skip-Step([string]$Reason) {
    throw "SKIP: $Reason"
}

function Invoke-Step {
    param([string]$Name, [scriptblock]$Action)
    Write-Host ""
    Write-Host "=== $Name ===" -ForegroundColor Cyan
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $status = 'PASS'
    $note = ''
    try {
        $global:LASTEXITCODE = 0
        & $Action
    }
    catch {
        $message = $_.Exception.Message
        if ($message -like 'SKIP: *') {
            $status = 'SKIP'
            $note = $message.Substring(6)
        }
        else {
            $status = 'FAIL'
            $note = $message
            Write-Host $message -ForegroundColor Red
        }
    }
    $watch.Stop()
    $colour = switch ($status) { 'PASS' { 'Green' } 'SKIP' { 'Yellow' } default { 'Red' } }
    Write-Host ("--- {0}: {1} ({2:N1}s)" -f $Name, $status, $watch.Elapsed.TotalSeconds) -ForegroundColor $colour
    $script:Results.Add([pscustomobject]@{
            Step    = $Name
            Status  = $status
            Seconds = [math]::Round($watch.Elapsed.TotalSeconds, 1)
            Note    = $note
        })
}

function New-TempDir {
    $path = Join-Path ([System.IO.Path]::GetTempPath()) ("jarvis-check-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $path | Out-Null
    $script:TempDirs.Add($path)
    return $path
}

function Get-NpmScripts([string]$Dir) {
    $pkg = Join-Path $Dir 'package.json'
    if (-not (Test-Path -LiteralPath $pkg)) { return @() }
    $json = Get-Content -LiteralPath $pkg -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($json.PSObject.Properties.Name -contains 'scripts') {
        return @($json.scripts.PSObject.Properties.Name)
    }
    return @()
}

# --- tool discovery ---------------------------------------------------------------------------

$local = $env:LOCALAPPDATA
$roaming = $env:APPDATA
$gitleaks = Find-Tool 'gitleaks' @(
    (Join-Path $local 'Microsoft\WinGet\Links\gitleaks.exe'),
    (Join-Path $local 'Microsoft\WinGet\Packages\Gitleaks.Gitleaks_*\gitleaks.exe')
)
$semgrep = Find-Tool 'semgrep' @(
    (Join-Path $roaming 'Python\Python3*\Scripts\semgrep.exe'),
    (Join-Path $local 'Programs\Python\Python3*\Scripts\semgrep.exe')
)

if ($semgrep) {
    # semgrep.exe starts pysemgrep, which must be on PATH too (it lives in the same folder).
    $env:PATH = (Split-Path -Parent $semgrep) + [System.IO.Path]::PathSeparator + $env:PATH
}

$hasDesktop = Test-Path -LiteralPath (Join-Path $Root 'apps\desktop\src-tauri\Cargo.toml')
$hasWebLock = Test-Path -LiteralPath (Join-Path $Root 'web\pnpm-lock.yaml')

# --- Scripts -----------------------------------------------------------------------------------

# Every script under scripts\ must parse in Windows PowerShell 5.1 (the shell the operations scripts
# target; pwsh would accept things 5.1 rejects) and stay ASCII (5.1 reads a BOM-less file as ANSI, so a
# typographic dash would silently change meaning).
Invoke-Step 'scripts: PowerShell 5.1 syntax' {
    $ps51 = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    if (-not (Test-Path -LiteralPath $ps51)) { Skip-Step 'Windows PowerShell 5.1 not found' }
    $code = @'
$ProgressPreference = 'SilentlyContinue'
$bad = 0
foreach ($f in Get-ChildItem -LiteralPath 'scripts' -Filter '*.ps1' -Recurse) {
    $tokens = $null; $errors = $null
    [void][System.Management.Automation.Language.Parser]::ParseFile($f.FullName, [ref]$tokens, [ref]$errors)
    foreach ($e in $errors) { $bad++; '{0}:{1}: {2}' -f $f.FullName, $e.Extent.StartLineNumber, $e.Message }
    foreach ($b in [System.IO.File]::ReadAllBytes($f.FullName)) {
        if ($b -gt 127) { $bad++; '{0}: contains non-ASCII characters' -f $f.FullName; break }
    }
}
if ($bad -gt 0) { exit 1 }
'@
    $encoded = [Convert]::ToBase64String([System.Text.Encoding]::Unicode.GetBytes($code))
    Invoke-Native $ps51 @('-NoProfile', '-NonInteractive', '-EncodedCommand', $encoded)
}

# --- Rust -------------------------------------------------------------------------------------

Invoke-Step 'rust: fmt' { Invoke-Native 'cargo' @('fmt', '--all', '--check') }

Invoke-Step 'rust: clippy' {
    Invoke-Native 'cargo' @('clippy', '--workspace', '--all-targets', '--features', 'jarvis-bridge-client/mock', '--', '-D', 'warnings')
}

Invoke-Step 'rust: test' {
    Invoke-Native 'cargo' @('test', '--workspace', '--features', 'jarvis-bridge-client/mock')
}

if ($SkipWhisper) {
    $script:Results.Add([pscustomobject]@{ Step = 'rust: whisper feature'; Status = 'SKIP'; Seconds = 0; Note = '-SkipWhisper' })
}
else {
    Invoke-Step 'rust: whisper feature' {
        if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) { Skip-Step 'cmake not found (install CMake, or pass -SkipWhisper)' }
        $libclang = $env:LIBCLANG_PATH
        if (-not $libclang) {
            foreach ($candidate in @('C:\Program Files\LLVM\bin', 'C:\Program Files (x86)\LLVM\bin')) {
                if (Test-Path (Join-Path $candidate 'libclang.dll')) { $libclang = $candidate; break }
            }
        }
        if (-not $libclang) { Skip-Step 'LLVM (libclang.dll) not found (install LLVM, or pass -SkipWhisper)' }
        $env:LIBCLANG_PATH = $libclang
        Invoke-Native 'cargo' @('clippy', '-p', 'jarvis-voice', '--all-targets', '--features', 'whisper', '--', '-D', 'warnings')
        Invoke-Native 'cargo' @('test', '-p', 'jarvis-voice', '--features', 'whisper')
    }
}

if ($SkipDesktop) {
    if ($hasDesktop) {
        $script:Results.Add([pscustomobject]@{ Step = 'desktop: tauri shell'; Status = 'SKIP'; Seconds = 0; Note = '-SkipDesktop' })
    }
}
elseif ($hasDesktop) {
    Invoke-Step 'desktop: tauri shell' {
        Push-Location (Join-Path $Root 'apps\desktop\src-tauri')
        try {
            Invoke-Native 'cargo' @('fmt', '--all', '--check')
            Invoke-Native 'cargo' @('clippy', '--all-targets', '--locked', '--', '-D', 'warnings')
            Invoke-Native 'cargo' @('build', '--locked')
        }
        finally { Pop-Location }
    }
}

# --- Web --------------------------------------------------------------------------------------

if ($SkipWeb) {
    $script:Results.Add([pscustomobject]@{ Step = 'web: all'; Status = 'SKIP'; Seconds = 0; Note = '-SkipWeb' })
}
else {
    $pnpm = Find-Tool 'pnpm'
    $webScripts = Get-NpmScripts (Join-Path $Root 'web')

    Invoke-Step 'web: install' {
        if (-not $pnpm) { throw 'pnpm not found (npm install --global pnpm)' }
        Invoke-Native $pnpm @('--dir', 'web', 'install', '--frozen-lockfile')
    }
    Invoke-Step 'web: check' {
        if (-not $pnpm) { throw 'pnpm not found' }
        Invoke-Native $pnpm @('--dir', 'web', 'run', 'check')
    }
    Invoke-Step 'web: unit tests' {
        if (-not $pnpm) { throw 'pnpm not found' }
        if ($webScripts -notcontains 'test') { Skip-Step 'no "test" script in web/package.json yet' }
        Invoke-Native $pnpm @('--dir', 'web', 'run', 'test')
    }
    Invoke-Step 'web: build' {
        if (-not $pnpm) { throw 'pnpm not found' }
        Invoke-Native $pnpm @('--dir', 'web', 'run', 'build')
    }
    Invoke-Step 'web: e2e (playwright)' {
        if (-not $pnpm) { throw 'pnpm not found' }
        if ($webScripts -notcontains 'test:e2e') { Skip-Step 'no "test:e2e" script in web/package.json yet' }
        Invoke-Native $pnpm @('--dir', 'web', 'run', 'test:e2e')
    }
}

# --- Security ---------------------------------------------------------------------------------

if ($SkipSecurity) {
    $script:Results.Add([pscustomobject]@{ Step = 'security: all'; Status = 'SKIP'; Seconds = 0; Note = '-SkipSecurity' })
}
else {
    Invoke-Step 'security: cargo deny' {
        Invoke-Native 'cargo' @('deny', 'check')
        if ($hasDesktop) {
            Invoke-Native 'cargo' @('deny', '--manifest-path', 'apps/desktop/src-tauri/Cargo.toml', '--all-features', '--config', 'deny.toml', 'check')
        }
    }

    Invoke-Step 'security: cargo audit' {
        Invoke-Native 'cargo' (@('audit') + $AuditIgnore)
        if ($hasDesktop -and (Test-Path 'apps\desktop\src-tauri\Cargo.lock')) {
            Invoke-Native 'cargo' @('audit', '--file', 'apps/desktop/src-tauri/Cargo.lock')
        }
    }

    Invoke-Step 'security: pnpm audit' {
        $pnpmCmd = Find-Tool 'pnpm'
        if (-not $pnpmCmd) { throw 'pnpm not found (npm install --global pnpm)' }
        $audited = 0
        foreach ($dir in @('web', 'apps\desktop')) {
            if (Test-Path -LiteralPath (Join-Path $dir 'pnpm-lock.yaml')) {
                Write-Host "pnpm audit in $dir"
                Invoke-Native $pnpmCmd @('--dir', $dir, 'audit', '--prod', '--audit-level', 'high')
                $audited++
            }
        }
        if ($audited -eq 0) { Skip-Step 'no pnpm-lock.yaml found' }
    }

    Invoke-Step 'security: gitleaks (working tree)' {
        if (-not $gitleaks) { throw 'gitleaks not found (winget install Gitleaks.Gitleaks)' }
        # Copy tracked and untracked, non-ignored files to a temp dir, so git-ignored files
        # (.env, node_modules, target) are left out.
        $tree = New-TempDir
        $files = & git ls-files -co --exclude-standard
        if ($LASTEXITCODE -ne 0) { throw 'git ls-files failed' }
        foreach ($file in $files) {
            if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { continue }
            $dest = Join-Path $tree $file
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest) | Out-Null
            Copy-Item -LiteralPath $file -Destination $dest
        }
        Invoke-Native $gitleaks @('dir', '--redact', '--no-banner', '--exit-code', '1', $tree)
    }

    Invoke-Step 'security: gitleaks (git history)' {
        if (-not $gitleaks) { throw 'gitleaks not found (winget install Gitleaks.Gitleaks)' }
        Invoke-Native $gitleaks @('git', '--redact', '--no-banner', '--exit-code', '1', '.')
    }

    Invoke-Step 'security: semgrep' {
        if (-not $semgrep) { throw 'semgrep not found (pip install semgrep)' }
        $out = Join-Path (New-TempDir) 'semgrep.sarif'
        $env:PYTHONUTF8 = '1'
        Invoke-Native $semgrep @(
            'scan',
            '--config', 'p/default', '--config', 'p/rust', '--config', 'p/typescript', '--config', 'p/secrets',
            '--config', 'p/github-actions',
            '--metrics=off',
            '--exclude', 'node_modules', '--exclude', 'target', '--exclude', 'build', '--exclude', '.svelte-kit',
            '--sarif', '--output', $out
        )
        # Same gate as CI: warnings and errors fail the step; "note" results are listed only.
        $sarif = Get-Content -LiteralPath $out -Raw -Encoding UTF8 | ConvertFrom-Json
        $blocking = @()
        $notes = 0
        foreach ($run in $sarif.runs) {
            $levels = @{}
            foreach ($rule in $run.tool.driver.rules) {
                $level = 'warning'
                if ($rule.PSObject.Properties.Name -contains 'defaultConfiguration' -and $rule.defaultConfiguration.level) {
                    $level = $rule.defaultConfiguration.level
                }
                $levels[$rule.id] = $level
            }
            foreach ($result in $run.results) {
                $level = $levels[$result.ruleId]
                if ($result.PSObject.Properties.Name -contains 'level' -and $result.level) { $level = $result.level }
                $loc = $result.locations[0].physicalLocation
                $where = '{0}:{1}' -f $loc.artifactLocation.uri, $loc.region.startLine
                if ($level -eq 'error' -or $level -eq 'warning') {
                    $blocking += ('{0}: {1} at {2}' -f $level, $result.ruleId, $where)
                }
                else {
                    $notes++
                    Write-Host ("note: {0} at {1}" -f $result.ruleId, $where) -ForegroundColor Yellow
                }
            }
        }
        Write-Host ("semgrep: {0} blocking result(s), {1} note(s)" -f $blocking.Count, $notes)
        if ($blocking.Count -gt 0) {
            $blocking | ForEach-Object { Write-Host $_ -ForegroundColor Red }
            throw "$($blocking.Count) blocking semgrep result(s)"
        }
    }
}

# --- summary ----------------------------------------------------------------------------------

foreach ($dir in $script:TempDirs) {
    Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "=== Summary ===" -ForegroundColor Cyan
$script:Results | Format-Table -AutoSize -Wrap Step, Status, Seconds, Note | Out-String -Width 200 | Write-Host

$failed = @($script:Results | Where-Object { $_.Status -eq 'FAIL' })
$skipped = @($script:Results | Where-Object { $_.Status -eq 'SKIP' })
$passed = @($script:Results | Where-Object { $_.Status -eq 'PASS' })
Write-Host ("{0} passed, {1} failed, {2} skipped" -f $passed.Count, $failed.Count, $skipped.Count)

if ($failed.Count -gt 0) {
    Write-Host 'FAILED' -ForegroundColor Red
    exit 1
}
Write-Host 'OK' -ForegroundColor Green
exit 0
