#Requires -Version 5.1
<#
.SYNOPSIS
  Installs the JarvisBridge EA (and its vendored ZeroMQ binding) into an MT5 data folder.

.DESCRIPTION
  Copies, from <repo>\mql5 into <DataFolder>\MQL5:
    Experts\JarvisMT\          (the EA)
    Scripts\JarvisMT\          (JarvisSelfTest: offline JSON/protocol checks)
    Include\Jarvis\            (our JSON / protocol / handlers)
    Include\Zmq\, Include\Mql\ (vendored mql-zmq + mql4-lib subset)
    Libraries\*.dll            (libzmq.dll, libsodium.dll)

  The DLL SHA-256 hashes are checked against mql5\THIRD_PARTY.md before and after copying.
  Only adds or overwrites our own files. Nothing in the terminal folder is ever deleted.

.PARAMETER DataFolder
  MT5 data folder (File > Open Data Folder in the terminal), or just its 32-character id.
  Default: this machine's "C:\Program Files\MetaTrader 5" terminal data folder (pass -DataFolder for any other terminal).

.EXAMPLE
  pwsh scripts\install-ea.ps1
  pwsh scripts\install-ea.ps1 -DataFolder D0E8209F77C8CF37AD8BF550E51FF075
#>
[CmdletBinding()]
param(
    [Alias('Terminal')]
    [string]$DataFolder = 'C:\Users\Ninad\AppData\Roaming\MetaQuotes\Terminal\D0E8209F77C8CF37AD8BF550E51FF075'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$srcRoot = Join-Path $repoRoot 'mql5'

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

Write-Host @"

Next steps
  1. Compile:  pwsh scripts\compile-ea.ps1
  2. In MT5, open the Navigator (Ctrl+N), Expert Advisors > JarvisMT > JarvisBridge
     (right-click Refresh if it is not listed). Drag it onto any chart.
  3. In the EA's Common tab tick "Allow DLL imports" for THIS EA only.
     Do not enable it globally under Tools > Options > Expert Advisors.
  4. The Algo Trading button state does not matter: this EA version is read-only.
  5. The Experts journal should show: JarvisBridge 0.1.0 listening on 127.0.0.1:5555
"@
