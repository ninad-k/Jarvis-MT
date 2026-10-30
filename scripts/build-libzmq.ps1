#Requires -Version 5.1
<#
.SYNOPSIS
  Builds libzmq 4.3.5 from the official source release and installs it as mql5\Libraries\libzmq.dll.

.DESCRIPTION
  Fixes security finding BR-2 (the old libzmq 4.2.1 has CVE-2019-6250 and CVE-2020-15166).

  1. Downloads the pinned source tarball from the official zeromq/libzmq GitHub release into a
     build folder OUTSIDE the repository, and refuses to continue unless its SHA-256 matches the
     value pinned below.
  2. Extracts it and configures it with CMake for x64 Release with the MSVC toolchain.
     CURVE and libsodium are switched off (the EA does not use them), and the Visual C++ runtime
     is linked statically (/MT), so the DLL needs no Visual C++ redistributable.
  3. Builds it, then checks the result: the DLL is x64, it imports only Windows system DLLs, and
     it exports every zmq_* function that the vendored mql-zmq headers (mql5\Include\Zmq) import.
  4. Copies the DLL to mql5\Libraries\libzmq.dll and the licence to mql5\Libraries\LICENSE.libzmq,
     then prints the SHA-256 and compares it with the table in mql5\THIRD_PARTY.md.

  Safe to run again: it re-checks the tarball, reuses the extracted source and the build folder,
  and rewrites the output files only when their content changed. -Clean starts from nothing.

  Needs: Visual Studio Build Tools (C++ x64 tools), CMake 3.15 or newer, Windows 10 tar.exe and
  an internet connection for the first run. It never touches an MT5 terminal; run
  scripts\install-ea.ps1 afterwards to install the DLL.

.PARAMETER WorkDir
  Build folder. Default: %LOCALAPPDATA%\JarvisMT\build\libzmq (outside the repository). Keep it
  short; MSBuild has trouble with very long paths.

.PARAMETER RunTests
  Also build libzmq a second time with the dynamic C runtime and run libzmq's own test suite
  (slower, about 4 minutes; the tests read errno from their own runtime, so they cannot run
  against the static-runtime DLL). The installed DLL is still the static-runtime one. Some tests
  bind fixed ports such as 127.0.0.1:5555: stop the Jarvis EA (MT5) first.

.PARAMETER Clean
  Delete the extracted source and the build folder first.

.PARAMETER NoInstall
  Build and verify only; do not copy anything into mql5\Libraries.

.PARAMETER Generator
  CMake generator. Default: chosen from the Visual Studio found by vswhere, for example
  "Visual Studio 18 2026".

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\build-libzmq.ps1
#>
[CmdletBinding()]
param(
    [string]$WorkDir = (Join-Path $env:LOCALAPPDATA 'JarvisMT\build\libzmq'),
    [switch]$RunTests,
    [switch]$Clean,
    [switch]$NoInstall,
    [string]$Generator
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# --- pinned inputs -------------------------------------------------------------------------
$Version = '4.3.5'
$TarballUrl = "https://github.com/zeromq/libzmq/releases/download/v$Version/zeromq-$Version.tar.gz"
# SHA-256 of the official zeromq-4.3.5.tar.gz. Computed on first download; the release's own
# SHA1SUMS (bdbf686c8a40ba638e21cf74e34dbb425e108500) and MD5SUMS (ae933b1e98411fd7cb8309f9502d2737)
# files were checked against the same download. GitHub shows no digest for this old release.
$TarballSha256 = '6653ef5910f17954861fe72332e68b03ca6e4d9c7160eb3a8de5a5a913bfab43'

$repoRoot = Split-Path -Parent $PSScriptRoot
$mqlRoot = Join-Path $repoRoot 'mql5'
$libDir = Join-Path $mqlRoot 'Libraries'
$headerDir = Join-Path $mqlRoot 'Include\Zmq'
$thirdParty = Join-Path $mqlRoot 'THIRD_PARTY.md'

function Get-Sha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Invoke-Native([string]$Exe, [string[]]$Arguments) {
    # Windows PowerShell 5.1 turns a native command's stderr output into an error record; CMake
    # writes warnings there. Only the exit code decides.
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $Exe @Arguments } finally { $ErrorActionPreference = $previous }
    if ($LASTEXITCODE -ne 0) { throw "$Exe failed with exit code $LASTEXITCODE" }
}

# A build folder inside the repository would be committed by accident.
$WorkDir = [IO.Path]::GetFullPath($WorkDir)
if ($WorkDir.StartsWith($repoRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "WorkDir must be outside the repository: $WorkDir"
}

# --- tools ---------------------------------------------------------------------------------
$cmake = (Get-Command cmake.exe -ErrorAction SilentlyContinue)
if ($cmake) { $cmake = $cmake.Source } else { $cmake = Join-Path $env:ProgramFiles 'CMake\bin\cmake.exe' }
if (-not (Test-Path -LiteralPath $cmake)) { throw 'CMake not found. Install it from https://cmake.org/download/ and re-run.' }

$tar = Join-Path $env:SystemRoot 'System32\tar.exe'
if (-not (Test-Path -LiteralPath $tar)) { throw "tar.exe not found at $tar (Windows 10 1803 or newer is required)." }

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere)) {
    throw 'vswhere.exe not found. Install the Visual Studio Build Tools with the "Desktop development with C++" workload.'
}
$vsVersion = (& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationVersion | Select-Object -First 1)
if (-not $vsVersion) { throw 'No Visual Studio with the C++ x64 tools found (workload "Desktop development with C++").' }
$vsMajor = [int]($vsVersion -split '\.')[0]
if (-not $Generator) {
    $names = @{ 16 = 'Visual Studio 16 2019'; 17 = 'Visual Studio 17 2022'; 18 = 'Visual Studio 18 2026' }
    if (-not $names.ContainsKey($vsMajor)) { throw "Unknown Visual Studio major version $vsMajor; pass -Generator." }
    $Generator = $names[$vsMajor]
}
Write-Host "CMake:     $cmake ($((& $cmake --version | Select-Object -First 1)))"
Write-Host "Generator: $Generator (Visual Studio $vsVersion), x64"
Write-Host "WorkDir:   $WorkDir"

if ($Clean -and (Test-Path -LiteralPath $WorkDir)) {
    Write-Host 'Cleaning the work folder.'
    Remove-Item -LiteralPath $WorkDir -Recurse -Force
}
if ($WorkDir -match '\s') { throw "WorkDir must not contain spaces (compiler flags embed it): $WorkDir. Pass -WorkDir, for example C:\jarvis-build\libzmq." }
New-Item -ItemType Directory -Path $WorkDir -Force | Out-Null

# --- download and verify the tarball -------------------------------------------------------
$tarball = Join-Path $WorkDir "zeromq-$Version.tar.gz"
if (-not (Test-Path -LiteralPath $tarball) -or (Get-Sha256 $tarball) -ne $TarballSha256) {
    Write-Host "Downloading $TarballUrl"
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $part = "$tarball.part"
    Invoke-WebRequest -Uri $TarballUrl -OutFile $part -UseBasicParsing
    $got = Get-Sha256 $part
    if ($got -ne $TarballSha256) {
        Remove-Item -LiteralPath $part -Force
        throw "SHA-256 mismatch for the libzmq tarball`n  expected $TarballSha256`n  actual   $got"
    }
    Move-Item -LiteralPath $part -Destination $tarball -Force
}
Write-Host "Tarball SHA-256 verified: $TarballSha256"

# --- extract -------------------------------------------------------------------------------
$srcParent = Join-Path $WorkDir 'src'
$srcDir = Join-Path $srcParent "zeromq-$Version"
$marker = Join-Path $srcParent ".extracted-$TarballSha256"
if (-not (Test-Path -LiteralPath $marker)) {
    if (Test-Path -LiteralPath $srcParent) { Remove-Item -LiteralPath $srcParent -Recurse -Force }
    New-Item -ItemType Directory -Path $srcParent -Force | Out-Null
    Write-Host 'Extracting.'
    Invoke-Native $tar @('-xzf', $tarball, '-C', $srcParent)
    New-Item -ItemType File -Path $marker -Force | Out-Null
}
$zmqH = Get-Content -LiteralPath (Join-Path $srcDir 'include\zmq.h') -Raw
if ($zmqH -notmatch '#define ZMQ_VERSION_MAJOR 4\s+#define ZMQ_VERSION_MINOR 3\s+#define ZMQ_VERSION_PATCH 5') {
    throw 'The extracted source is not libzmq 4.3.5.'
}
if ($zmqH -notmatch '_\[64\]') { throw 'zmq_msg_t is not 64 bytes; mql-zmq ZmqMsg would need changing.' }

# --- configure and build -------------------------------------------------------------------
# Static runtime: libzmq's CMakeLists declares cmake_minimum_required 2.8.12, which leaves policy
# CMP0091 off, so CMAKE_MSVC_RUNTIME_LIBRARY is ignored. The old way, /MT in the release flags,
# works. -DCMAKE_POLICY_VERSION_MINIMUM is needed with CMake 4, which dropped support below 3.5.
# /Brepro and /PDBALTPATH make the DLL bit-for-bit repeatable for the same toolchain.
$buildDir = Join-Path $WorkDir 'build'
$linkFlags = '/Brepro /PDBALTPATH:libzmq.pdb'

# libzmq's assertions embed __FILE__. /d1trimfile cuts the build folder off those paths, so the DLL
# does not contain the builder's user name and builds from different folders are identical. The
# flag value must not end in a backslash (PowerShell would pass it as an escaped quote), so
# /DNDEBUG comes last in the flags below.
$trimFlags = "/d1trimfile:$srcDir\ /d1trimfile:$WorkDir\"

function Invoke-LibzmqBuild([string]$Dir, [string]$RuntimeFlag, [string]$Tests) {
    $cfg = @(
        '-G', $Generator, '-A', 'x64', '-S', $srcDir, '-B', $Dir,
        '-DCMAKE_POLICY_VERSION_MINIMUM=3.5',
        "-DCMAKE_C_FLAGS_RELEASE=$RuntimeFlag /O2 /Ob2 $trimFlags /DNDEBUG",
        "-DCMAKE_CXX_FLAGS_RELEASE=$RuntimeFlag /O2 /Ob2 $trimFlags /DNDEBUG",
        "-DCMAKE_SHARED_LINKER_FLAGS=$linkFlags",
        '-DWITH_LIBSODIUM=OFF', '-DENABLE_CURVE=OFF',
        '-DWITH_DOCS=OFF', "-DBUILD_TESTS=$Tests",
        '-DBUILD_SHARED=ON', '-DBUILD_STATIC=OFF',
        '-DENABLE_DRAFTS=OFF', '-DWITH_PERF_TOOL=OFF', '-DENABLE_CPACK=OFF'
    )
    Invoke-Native $cmake $cfg
    Invoke-Native $cmake @('--build', $Dir, '--config', 'Release', '--parallel', '--', '/nologo', '/verbosity:minimal')
}

Write-Host 'Configuring and building (Release, x64, static runtime).'
Invoke-LibzmqBuild $buildDir '/MT' 'OFF'

$built = @(Get-ChildItem -LiteralPath (Join-Path $buildDir 'bin\Release') -Filter 'libzmq*.dll' -File)
if ($built.Count -ne 1) { throw "Expected exactly one libzmq*.dll in $buildDir\bin\Release, found $($built.Count)." }
$dll = $built[0].FullName

if ($RunTests) {
    # libzmq's tests read errno from their own C runtime, so they only pass against a DLL that
    # shares it. Run them on a second build that uses the dynamic runtime (same sources and
    # options otherwise). The DLL that gets installed is the static-runtime one built above.
    $testDir = Join-Path $WorkDir 'build-tests'
    Write-Host 'Building the test configuration (dynamic runtime) and running the libzmq test suite.'
    Invoke-LibzmqBuild $testDir '/MD' 'ON'
    try {
        Invoke-Native (Join-Path (Split-Path -Parent $cmake) 'ctest.exe') @('--test-dir', $testDir, '-C', 'Release', '--output-on-failure', '--timeout', '120')
    } catch {
        Write-Warning 'Some libzmq tests failed (see above). Tests that bind a fixed port (5555 among them) fail while the Jarvis EA or another program holds it: stop MT5 first and re-run.'
    }
}

# --- toolchain details and verification tools ----------------------------------------------
$ccFile = @(Get-ChildItem -LiteralPath (Join-Path $buildDir 'CMakeFiles') -Recurse -Filter 'CMakeCXXCompiler.cmake' -File)[0].FullName
$ccText = Get-Content -LiteralPath $ccFile -Raw
$clPath = [regex]::Match($ccText, 'set\(CMAKE_CXX_COMPILER "([^"]+)"').Groups[1].Value
$clVersion = [regex]::Match($ccText, 'set\(CMAKE_CXX_COMPILER_VERSION "([^"]+)"').Groups[1].Value
$toolset = [regex]::Match($clPath, 'MSVC/([0-9.]+)/').Groups[1].Value
$dumpbin = Join-Path (Split-Path -Parent $clPath) 'dumpbin.exe'
if (-not (Test-Path -LiteralPath $dumpbin)) { throw "dumpbin.exe not found next to $clPath" }
Write-Host "Compiler:  MSVC cl $clVersion (toolset $toolset)"

# 1. x64
$headers = (& $dumpbin /nologo /headers $dll) -join "`n"
if ($headers -notmatch 'machine \(x64\)') { throw 'The DLL is not an x64 image.' }

# 2. imports only Windows system DLLs (no VC++ runtime, no libsodium)
$depText = (& $dumpbin /nologo /dependents $dll) -join "`n"
$depBlock = [regex]::Match($depText, 'Image has the following dependencies:(.*?)Summary', 'Singleline').Groups[1].Value
$deps = @($depBlock -split "`n" | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$allowed = '^(kernel32|advapi32|ws2_32|iphlpapi|ntdll|bcrypt|api-ms-win-[a-z0-9-]+)\.dll$'
$bad = @($deps | Where-Object { $_ -notmatch $allowed })
Write-Host "Imports:   $($deps -join ', ')"
if ($bad.Count -gt 0) { throw "The DLL depends on non-system libraries: $($bad -join ', ')" }

# 3. exports every function the mql-zmq headers import
$required = New-Object 'System.Collections.Generic.SortedSet[string]'
foreach ($file in Get-ChildItem -LiteralPath $headerDir -Filter '*.mqh' -File) {
    $inImport = $false
    foreach ($line in Get-Content -LiteralPath $file.FullName) {
        if ($line -match '^\s*#import\s+"libzmq\.dll"') { $inImport = $true; continue }
        if ($line -match '^\s*#import\b') { $inImport = $false; continue }
        if (-not $inImport) { continue }
        $code = ($line -replace '//.*$', '')
        foreach ($m in [regex]::Matches($code, '\b(zmq_[a-z0-9_]+)\s*\(')) { [void]$required.Add($m.Groups[1].Value) }
    }
}
if ($required.Count -lt 40) { throw "Parsed only $($required.Count) imported functions from $headerDir; the parser is broken." }
$expText = (& $dumpbin /nologo /exports $dll) -join "`n"
$exported = New-Object 'System.Collections.Generic.HashSet[string]'
foreach ($m in [regex]::Matches($expText, '(?m)^\s+\d+\s+[0-9A-F]+\s+[0-9A-F]{8}\s+(\S+)')) { [void]$exported.Add($m.Groups[1].Value) }
$missing = @($required | Where-Object { -not $exported.Contains($_) })
Write-Host "Exports:   $($exported.Count) symbols; all $($required.Count) functions imported by mql-zmq are present"
if ($missing.Count -gt 0) { throw "The DLL does not export: $($missing -join ', ')" }

$dllSha = Get-Sha256 $dll
Write-Host ''
Write-Host "Built DLL SHA-256: $dllSha"

# --- install into the repository -----------------------------------------------------------
if ($NoInstall) {
    Write-Host 'NoInstall: nothing copied.'
    return
}
$target = Join-Path $libDir 'libzmq.dll'
if (-not (Test-Path -LiteralPath $target) -or (Get-Sha256 $target) -ne $dllSha) {
    Copy-Item -LiteralPath $dll -Destination $target -Force
    Write-Host "Installed $target"
} else {
    Write-Host "$target is already up to date."
}
$licenseTarget = Join-Path $libDir 'LICENSE.libzmq'
$licenseSource = Join-Path $srcDir 'LICENSE'
if (-not (Test-Path -LiteralPath $licenseTarget) -or (Get-Sha256 $licenseTarget) -ne (Get-Sha256 $licenseSource)) {
    Copy-Item -LiteralPath $licenseSource -Destination $licenseTarget -Force
    Write-Host "Installed $licenseTarget"
}

# --- compare with THIRD_PARTY.md -----------------------------------------------------------
$recorded = $null
foreach ($line in Get-Content -LiteralPath $thirdParty) {
    if ($line -match '^\|\s*libzmq\.dll\s*\|.*\|\s*([0-9a-fA-F]{64})\s*\|\s*$') { $recorded = $Matches[1].ToLowerInvariant() }
}
if ($recorded -eq $dllSha) {
    Write-Host 'SHA-256 matches mql5\THIRD_PARTY.md (the committed DLL is reproduced).'
} else {
    Write-Host 'SHA-256 DIFFERS from mql5\THIRD_PARTY.md. If you changed the version, toolchain or options on purpose,'
    Write-Host "update the libzmq.dll row there to: $dllSha"
    Write-Host "(recorded: $recorded; toolchain MSVC $clVersion, toolset $toolset)"
}
