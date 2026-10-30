#Requires -Version 5.1
<#
.SYNOPSIS
  Installs the JarvisBridge EA (and its vendored ZeroMQ binding) into an MT5 data folder.

.DESCRIPTION
  Copies, from <repo>\mql5 into <DataFolder>\MQL5:
    Experts\JarvisMT\          (the EA)
    Scripts\JarvisMT\          (JarvisSelfTest: offline JSON/protocol checks; sends no trades)
    Include\Jarvis\            (our JSON / protocol / handlers)
    Include\Zmq\, Include\Mql\ (vendored mql-zmq + mql4-lib subset)
    Libraries\*.dll            (libzmq.dll; built by scripts\build-libzmq.ps1)

  The DLL SHA-256 hashes are checked against mql5\THIRD_PARTY.md before and after copying.
  Only adds or overwrites our own files. Nothing in the terminal folder is ever deleted.

  Also creates the shared bridge key (finding BR-1, ADR-0016) if it does not exist yet:
  32 random bytes from the system's cryptographic generator, written as 64 hex characters to
  <DataFolder>\MQL5\Files\JarvisMT\bridge.key. That folder is the EA's file sandbox and lives in
  your Windows profile, so other Windows users cannot read it. The key is NEVER printed. An
  existing key is never touched (unless -RotateKey is given). Set [bridge] key_file in
  config\jarvis.toml to the printed path; without the key neither the EA nor the API starts.

.PARAMETER DataFolder
  MT5 data folder (File > Open Data Folder in the terminal), or just its 32-character id.
  Default: the only terminal data folder under %APPDATA%\MetaQuotes\Terminal. With several
  terminals installed the script stops and asks for -DataFolder.

.PARAMETER RotateKey
  Replace an existing bridge key with a new random one. The EA and the API must both be
  restarted afterwards (the old key stops working at once for whoever still holds it).

.EXAMPLE
  pwsh scripts\install-ea.ps1
  pwsh scripts\install-ea.ps1 -DataFolder <32-character terminal id>
  pwsh scripts\install-ea.ps1 -RotateKey
#>
[CmdletBinding()]
param(
    [Alias('Terminal')]
    [string]$DataFolder,
    [switch]$RotateKey
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$srcRoot = Join-Path $repoRoot 'mql5'

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

# A bare terminal id is accepted too.
if ($DataFolder -match '^[0-9A-Fa-f]{32}$') {
    $DataFolder = Join-Path (Join-Path $env:APPDATA 'MetaQuotes\Terminal') $DataFolder
}
$dstRoot = Join-Path $DataFolder 'MQL5'

if (-not (Test-Path -LiteralPath $srcRoot)) { throw "Source folder not found: $srcRoot" }
if (-not (Test-Path -LiteralPath $dstRoot)) {
    throw "Not an MT5 data folder (no MQL5 subfolder): $DataFolder"
}

function Get-Sha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

# --- the shared bridge key (BR-1) -----------------------------------------------------------
$keyDir = Join-Path $dstRoot 'Files\JarvisMT'
$keyPath = Join-Path $keyDir 'bridge.key'

# 32 random bytes from the operating system's CSPRNG, as 64 lowercase hex characters.
function New-BridgeKeyHex {
    $bytes = New-Object byte[] 32
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }
    $hex = ($bytes | ForEach-Object { $_.ToString('x2') }) -join ''
    [Array]::Clear($bytes, 0, $bytes.Length)
    return $hex
}

# Is the first non-empty line of the file 64 hex characters (not all zeros)? Never prints it.
function Test-BridgeKeyFile([string]$Path) {
    $line = Get-Content -LiteralPath $Path -TotalCount 5 -ErrorAction Stop |
        Where-Object { $_.Trim() -ne '' } | Select-Object -First 1
    if (-not $line) { return $false }
    $line = $line.Trim()
    return ($line -match '^[0-9A-Fa-f]{64}$') -and ($line -notmatch '^0{64}$')
}

$keyExists = Test-Path -LiteralPath $keyPath
if ($keyExists -and -not $RotateKey) {
    if (-not (Test-BridgeKeyFile $keyPath)) {
        throw "The existing bridge key file is not a valid key (64 hex characters): $keyPath`nIt was left untouched. Fix it by hand, or run again with -RotateKey to replace it with a new random key."
    }
}

# --- expected DLL hashes from THIRD_PARTY.md -----------------------------------------------
$thirdParty = Join-Path $srcRoot 'THIRD_PARTY.md'
if (-not (Test-Path -LiteralPath $thirdParty)) { throw "Missing $thirdParty" }
$expected = @{}
foreach ($line in Get-Content -LiteralPath $thirdParty) {
    if ($line -match '^\|\s*([A-Za-z0-9_.\-]+\.dll)\s*\|.*\|\s*([0-9a-fA-F]{64})\s*\|\s*$') {
        $expected[$Matches[1].ToLowerInvariant()] = $Matches[2].ToLowerInvariant()
    }
}
if ($expected.Count -eq 0) { throw "No DLL hashes found in $thirdParty" }

$srcLib = Join-Path $srcRoot 'Libraries'
$dlls = @(Get-ChildItem -LiteralPath $srcLib -Filter '*.dll' -File)
if ($dlls.Count -eq 0) { throw "No DLLs in $srcLib" }
foreach ($dll in $dlls) {
    $want = $expected[$dll.Name.ToLowerInvariant()]
    if (-not $want) { throw "$($dll.Name) has no SHA-256 entry in THIRD_PARTY.md; refusing to install it." }
    $got = Get-Sha256 $dll.FullName
    if ($got -ne $want) {
        throw "SHA-256 mismatch for $($dll.Name)`n  expected $want`n  actual   $got"
    }
}
Write-Host "DLL hashes match THIRD_PARTY.md ($($dlls.Count) files)."

# --- copy our files (add/overwrite only) ---------------------------------------------------
function Copy-Tree([string]$RelDir, [string[]]$Include = @('*')) {
    $from = Join-Path $srcRoot $RelDir
    $to = Join-Path $dstRoot $RelDir
    if (-not (Test-Path -LiteralPath $from)) { throw "Missing source folder $from" }
    $copied = 0; $same = 0
    foreach ($file in Get-ChildItem -LiteralPath $from -Recurse -File -Include $Include) {
        $rel = $file.FullName.Substring($from.Length).TrimStart('\')
        $target = Join-Path $to $rel
        $targetDir = Split-Path -Parent $target
        if (-not (Test-Path -LiteralPath $targetDir)) {
            New-Item -ItemType Directory -Path $targetDir -Force | Out-Null
        }
        if ((Test-Path -LiteralPath $target) -and ((Get-Sha256 $target) -eq (Get-Sha256 $file.FullName))) {
            $same++
            continue
        }
        Copy-Item -LiteralPath $file.FullName -Destination $target -Force
        $copied++
    }
    Write-Host ("  {0,-22} {1} copied, {2} unchanged" -f $RelDir, $copied, $same)
}

Write-Host "Installing into $dstRoot"
Copy-Tree 'Experts\JarvisMT'
Copy-Tree 'Scripts\JarvisMT'
Copy-Tree 'Include\Jarvis'
Copy-Tree 'Include\Zmq'
Copy-Tree 'Include\Mql'
Copy-Tree 'Libraries' -Include @('*.dll')

# --- verify what landed in the terminal ----------------------------------------------------
foreach ($dll in $dlls) {
    $target = Join-Path $dstRoot ('Libraries\' + $dll.Name)
    if ((Get-Sha256 $target) -ne $expected[$dll.Name.ToLowerInvariant()]) {
        throw "Installed $($dll.Name) does not match the expected SHA-256."
    }
}
Write-Host "Installed DLL hashes verified."

# --- create (or rotate) the bridge key ------------------------------------------------------
$keyAction = 'kept'
if (-not $keyExists -or $RotateKey) {
    if (-not (Test-Path -LiteralPath $keyDir)) {
        New-Item -ItemType Directory -Path $keyDir -Force | Out-Null
    }
    $hex = New-BridgeKeyHex
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    if (-not $keyExists) {
        # CreateNew: fails instead of overwriting if another process created the file meanwhile.
        $stream = [System.IO.File]::Open($keyPath, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
        try {
            $payload = $utf8NoBom.GetBytes($hex + "`n")
            $stream.Write($payload, 0, $payload.Length)
        } finally { $stream.Dispose() }
        $keyAction = 'created'
    } else {
        $tmp = $keyPath + '.new'
        [System.IO.File]::WriteAllText($tmp, $hex + "`n", $utf8NoBom)
        Move-Item -LiteralPath $tmp -Destination $keyPath -Force
        $keyAction = 'rotated'
    }
    $hex = $null
}
Write-Host "Bridge key $keyAction`: $keyPath"

$appData = $env:APPDATA
if ($appData -and -not $keyPath.StartsWith($appData, [System.StringComparison]::OrdinalIgnoreCase)) {
    Write-Warning "The MT5 data folder is outside your Windows profile ($appData). Make sure that only your own Windows account can read $keyPath (right-click > Properties > Security)."
}
if ($keyAction -eq 'rotated') {
    Write-Host "The key was replaced. Restart the EA (remove it from the chart and add it again) and the Jarvis API; until both run with the new key, every request is refused."
}

Write-Host @"

Next steps
  1. Compile:  pwsh scripts\compile-ea.ps1
  2. In MT5, open the Navigator (Ctrl+N), Expert Advisors > JarvisMT > JarvisBridge
     (right-click Refresh if it is not listed). Drag it onto any chart.
  3. In the EA's Common tab tick "Allow DLL imports" for THIS EA only.
     Do not enable it globally under Tools > Options > Expert Advisors.
  4. Write commands (order_send, ...) need the Algo Trading button ON and "Allow Algo Trading"
     for this EA. Without them the EA answers writes with retcode -3. Reads work either way.
  5. Real accounts are refused unless you set the EA input InpAllowReal = true (default false).
  6. The Experts journal should show: JarvisBridge 0.3.1 listening on 127.0.0.1:5555 (protocol 2, authenticated, ..., max order 1 lots)
  7. Tell the Jarvis API where the key is. In config\jarvis.toml:

         [bridge]
         key_file = '$keyPath'

     (single quotes: backslashes are literal in a TOML literal string). Or set JARVIS_BRIDGE_KEY
     to the key's hex in .env. The API refuses to start without a valid key. Never copy the key
     into the repository, a chat, an issue or a log.
"@
