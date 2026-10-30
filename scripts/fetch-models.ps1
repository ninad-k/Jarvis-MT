<#
.SYNOPSIS
    Downloads the local voice models into models/ (gitignored).

.DESCRIPTION
    Fetches, verifies (SHA-256) and installs:
      - Piper (Windows build)            -> models/piper/piper.exe (+ DLLs, espeak-ng-data)
      - Piper voice en_US-lessac-medium  -> models/piper/en_US-lessac-medium.onnx(.json)
      - Whisper ggml-base.en             -> models/whisper/ggml-base.en.bin

    Sources are the official upstream projects at pinned versions: rhasspy/piper
    (GitHub releases), rhasspy/piper-voices (Hugging Face) and ggerganov/whisper.cpp
    (Hugging Face). The script is idempotent: files whose hash already matches are
    skipped. A hash mismatch deletes the download and stops with an error.

    Works with Windows PowerShell 5.1 and PowerShell 7.

.PARAMETER Force
    Download and install again even if a verified copy exists.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\fetch-models.ps1
#>
[CmdletBinding()]
param(
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Older PowerShell defaults to TLS 1.0/1.1; the hosts below need TLS 1.2.
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch {
    # Not fatal: newer runtimes negotiate TLS on their own.
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$modelsDir = Join-Path $repoRoot 'models'
$piperDir = Join-Path $modelsDir 'piper'
$whisperDir = Join-Path $modelsDir 'whisper'
$cacheDir = Join-Path $modelsDir '.cache'

# --- Pinned sources and SHA-256 hashes -------------------------------------------

$PiperVersion = '2023.11.14-2'
$PiperZip = @{
    Name   = 'piper_windows_amd64.zip'
    Url    = "https://github.com/rhasspy/piper/releases/download/$PiperVersion/piper_windows_amd64.zip"
    Sha256 = 'f3c58906402b24f3a96d92145f58acba6d86c9b5db896d207f78dc80811efcea'
}

$VoicesRevision = '5041a7a49ae5b3a0f30e6837769eb03fdbb231bd'
$VoiceBase = "https://huggingface.co/rhasspy/piper-voices/resolve/$VoicesRevision/en/en_US/lessac/medium"
$VoiceFiles = @(
    @{
        Name   = 'en_US-lessac-medium.onnx'
        Url    = "$VoiceBase/en_US-lessac-medium.onnx"
        Sha256 = '5efe09e69902187827af646e1a6e9d269dee769f9877d17b16b1b46eeaaf019f'
    },
    @{
        Name   = 'en_US-lessac-medium.onnx.json'
        Url    = "$VoiceBase/en_US-lessac-medium.onnx.json"
        Sha256 = 'efe19c417bed055f2d69908248c6ba650fa135bc868b0e6abb3da181dab690a0'
    }
)

$WhisperRevision = '5359861c739e955e79d9a303bcbc70fb988958b1'
$WhisperModel = @{
    Name   = 'ggml-base.en.bin'
    Url    = "https://huggingface.co/ggerganov/whisper.cpp/resolve/$WhisperRevision/ggml-base.en.bin"
    Sha256 = 'a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002'
}

# --- Helpers ---------------------------------------------------------------------

function Get-Sha256([string]$Path) {
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Test-Verified([string]$Path, [string]$Sha256) {
    return (Test-Path -LiteralPath $Path -PathType Leaf) -and ((Get-Sha256 $Path) -eq $Sha256)
}

# Downloads to <dest>.part, checks the hash, then renames. Returns nothing.
function Save-Verified([hashtable]$Item, [string]$Dest) {
    if (-not $Force -and (Test-Verified $Dest $Item.Sha256)) {
        Write-Host ("[skip] {0} (sha256 {1} ok)" -f $Item.Name, $Item.Sha256)
        return
    }
    $dir = Split-Path -Parent $Dest
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $part = "$Dest.part"
    if (Test-Path -LiteralPath $part) { Remove-Item -LiteralPath $part -Force }

    Write-Host ("[get ] {0}" -f $Item.Url)
    $client = New-Object System.Net.WebClient
    try {
        $client.DownloadFile($Item.Url, $part)
    } finally {
        $client.Dispose()
    }

    $actual = Get-Sha256 $part
    Write-Host ("       sha256 {0}" -f $actual)
    if ($actual -ne $Item.Sha256) {
        Remove-Item -LiteralPath $part -Force
        throw ("SHA-256 mismatch for {0}: expected {1}, got {2}" -f $Item.Name, $Item.Sha256, $actual)
    }
    Move-Item -LiteralPath $part -Destination $Dest -Force
    Write-Host ("[ ok ] {0}" -f $Dest)
}

# --- Piper executable ------------------------------------------------------------

$zipPath = Join-Path $cacheDir $PiperZip.Name
$piperExe = Join-Path $piperDir 'piper.exe'
$marker = Join-Path $piperDir '.piper-release'
$markerText = "$PiperVersion $($PiperZip.Sha256)"

$installed = (Test-Path -LiteralPath $piperExe) -and (Test-Path -LiteralPath $marker) -and
    ((Get-Content -LiteralPath $marker -Raw).Trim() -eq $markerText)

if ($installed -and -not $Force) {
    Write-Host ("[skip] piper {0} already installed in {1}" -f $PiperVersion, $piperDir)
} else {
    Save-Verified $PiperZip $zipPath

    $extractDir = Join-Path $cacheDir 'piper-extract'
    if (Test-Path -LiteralPath $extractDir) { Remove-Item -LiteralPath $extractDir -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $extractDir | Out-Null
    Expand-Archive -LiteralPath $zipPath -DestinationPath $extractDir -Force

    $inner = Join-Path $extractDir 'piper'
    if (-not (Test-Path -LiteralPath (Join-Path $inner 'piper.exe'))) {
        throw "piper.exe not found in the extracted archive ($inner)"
    }
    New-Item -ItemType Directory -Force -Path $piperDir | Out-Null
    Copy-Item -Path (Join-Path $inner '*') -Destination $piperDir -Recurse -Force
    Set-Content -LiteralPath $marker -Value $markerText -Encoding ASCII
    Remove-Item -LiteralPath $extractDir -Recurse -Force
    Write-Host ("[ ok ] piper {0} installed in {1}" -f $PiperVersion, $piperDir)
}

# --- Piper voice -----------------------------------------------------------------

foreach ($file in $VoiceFiles) {
    Save-Verified $file (Join-Path $piperDir $file.Name)
}

# --- Whisper model ---------------------------------------------------------------

Save-Verified $WhisperModel (Join-Path $whisperDir $WhisperModel.Name)

Write-Host ''
Write-Host 'Voice models are ready:'
Write-Host ("  whisper_model = ""models/whisper/{0}""" -f $WhisperModel.Name)
Write-Host '  piper_exe     = "models/piper/piper.exe"'
Write-Host '  piper_voice   = "models/piper/en_US-lessac-medium.onnx"'
