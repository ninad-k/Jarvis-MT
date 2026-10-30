# -----------------------------------------------------------------------------
# common.ps1 -- helpers shared by the Arc MT operations scripts (setup, run,
# stop, status, install-autostart, firewall, remote-start, remote-stop).
#
# Dot-source it from a script in scripts\:
#
#     . (Join-Path $PSScriptRoot 'lib\common.ps1')
#
# What lives here, and why it is shared:
#   * the colored step output every script uses (Write-Step/Ok/Warn/Fail/Hint)
#     and the banner, so all scripts look and read the same;
#   * a small TOML reader for config\jarvis.toml and a .env reader. The scripts
#     only ever CHECK these files (ports, auth, the bridge key, Ollama); they
#     never hand a value from them to another process, and no secret is ever
#     printed. The API loads .env and the config itself;
#   * process discovery by COMMAND LINE / PATH, never by PID file: a PID file
#     only knows about the process that wrote it, while command-line matching
#     also finds the orphan left by a crashed console. Everything is scoped to
#     THIS repository (paths under the repo root), so other projects' processes,
#     MT5 and Ollama are never matched by accident;
#   * the facts several scripts must agree on (what the API listens on, how
#     to ask it for its status, where the Ollama server is, where MT5 keeps its
#     data folders).
#
# Windows PowerShell 5.1 compatible (no ?., no ternary, no &&). ASCII only: the
# 5.1 parser reads a BOM-less file as ANSI, so a stray typographic character
# would silently change meaning.
# -----------------------------------------------------------------------------

$AppName = 'Arc MT'
$ScriptsDir = Split-Path -Parent $PSScriptRoot
$Root = Split-Path -Parent $ScriptsDir
$LogDir = Join-Path $Root 'logs'
$LocalDir = Join-Path $Root '.local'
$ServiceName = 'JarvisMT'
$DefaultApiPort = 8787
$DefaultVitePort = 5173
$DefaultBridgePort = 5555
$DefaultOllamaUrl = 'http://127.0.0.1:11434'
$DefaultOllamaModel = 'qwen3.5:9b'
$PowerShellExe = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'

# Cmdlet modules load on first use. Under -WhatIf that implicit import would itself be "simulated"
# (it prints "Set Alias ..." lines), so the modules the scripts rely on are imported up front with
# the WhatIf preference switched off.
$savedWhatIf = $WhatIfPreference
$WhatIfPreference = $false
try {
    Import-Module CimCmdlets, NetTCPIP -ErrorAction SilentlyContinue
} finally {
    $WhatIfPreference = $savedWhatIf
}

# Imports further modules (ScheduledTasks, NetSecurity) the same quiet way.
function Import-ModuleQuiet {
    param([string[]]$Name)
    $saved = $WhatIfPreference
    $WhatIfPreference = $false
    try { Import-Module $Name -ErrorAction SilentlyContinue } finally { $WhatIfPreference = $saved }
}

# --- output ------------------------------------------------------------------

function Write-Step { param([string]$Message) Write-Host "==> $Message" -ForegroundColor Cyan }
function Write-Ok   { param([string]$Message) Write-Host "    $Message" -ForegroundColor Green }
function Write-Warn { param([string]$Message) Write-Host "    $Message" -ForegroundColor Yellow }
function Write-Fail { param([string]$Message) Write-Host "    $Message" -ForegroundColor Red }
function Write-Hint { param([string]$Message) Write-Host "    $Message" -ForegroundColor DarkGray }

function Write-Banner {
    param([Parameter(Mandatory = $true)][string]$Title)
    Write-Host ''
    Write-Host "  $Title" -ForegroundColor White
    Write-Host "  $('-' * $Title.Length)" -ForegroundColor DarkGray
    Write-Host "  $Root" -ForegroundColor DarkGray
    Write-Host ''
}

# The window title is cosmetic: a hidden or redirected host may refuse it.
function Set-WindowTitle {
    param([string]$State)
    try { $Host.UI.RawUI.WindowTitle = "$AppName - $State" } catch { }
}

function Format-Age {
    param($Since)
    if (-not $Since) { return 'unknown' }
    $span = (Get-Date) - [datetime]$Since
    if ($span.TotalDays -ge 1) { return ('{0}d {1}h' -f [int]$span.TotalDays, $span.Hours) }
    if ($span.TotalHours -ge 1) { return ('{0}h {1}m' -f [int]$span.TotalHours, $span.Minutes) }
    if ($span.TotalMinutes -ge 1) { return ('{0}m {1}s' -f [int]$span.TotalMinutes, $span.Seconds) }
    return ('{0}s' -f [int]$span.TotalSeconds)
}

# --- system ------------------------------------------------------------------

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    return ([Security.Principal.WindowsPrincipal]$identity).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
}

# True when $Path is $Dir or lies below it (case-insensitive, no trailing-slash tricks).
function Test-PathUnder {
    param([string]$Path, [string]$Dir)
    if (-not $Path -or -not $Dir) { return $false }
    $d = $Dir.TrimEnd('\') + '\'
    return $Path.StartsWith($d, [StringComparison]::OrdinalIgnoreCase)
}

# Kill children first so nothing is orphaned.
function Stop-ProcessTree {
    param([Parameter(Mandatory = $true)][int]$TargetId)
    try {
        Get-CimInstance Win32_Process -Filter "ParentProcessId=$TargetId" -ErrorAction Stop |
            ForEach-Object { Stop-ProcessTree -TargetId $_.ProcessId }
    } catch {
        # No children, or CIM unavailable -- fall through and stop the parent anyway.
    }
    try { Stop-Process -Id $TargetId -Force -ErrorAction Stop } catch { }
}

# Can we open a TCP connection to the loopback port right now? The most direct "is something
# listening" test; it needs no rights and no CIM.
function Test-TcpConnect {
    param([int]$Port, [string]$Address = '127.0.0.1', [int]$TimeoutMs = 400)
    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $async = $client.BeginConnect($Address, $Port, $null, $null)
        if (-not $async.AsyncWaitHandle.WaitOne($TimeoutMs)) { return $false }
        $client.EndConnect($async)
        return $client.Connected
    } catch {
        return $false
    } finally {
        $client.Close()
    }
}

# Process ids listening on a TCP port (empty when none). Get-NetTCPConnection's own -State and
# -LocalPort filters are not used: on some Windows builds that CIM query returns an incomplete
# list, and a listening row can even come back with an empty State. So the full list is read and a
# row counts as a listener when its State is Listen or it has no remote end; `netstat -ano` is a
# second source. The union of both is returned.
function Get-PortOwners {
    param([int]$Port)
    $ids = New-Object System.Collections.Generic.List[int]
    try {
        foreach ($c in @(Get-NetTCPConnection -ErrorAction Stop)) {
            if ($c.LocalPort -ne $Port) { continue }
            $listening = ($c.State -eq 'Listen') -or ($c.RemotePort -eq 0 -and "$($c.RemoteAddress)" -in @('0.0.0.0', '::'))
            if ($listening -and -not $ids.Contains([int]$c.OwningProcess)) { $ids.Add([int]$c.OwningProcess) }
        }
    } catch { }
    try {
        foreach ($line in @(netstat.exe -ano -p TCP 2>$null)) {
            if ($line -match "^\s*TCP\s+\S+:$Port\s+\S+\s+LISTENING\s+(\d+)\s*$") {
                $id = [int]$Matches[1]
                if (-not $ids.Contains($id)) { $ids.Add($id) }
            }
        }
    } catch { }
    return @($ids)
}

# In use = someone owns a listener on it, or a connection to it succeeds.
function Test-PortInUse {
    param([int]$Port)
    if (@(Get-PortOwners $Port).Count -gt 0) { return $true }
    return (Test-TcpConnect -Port $Port)
}

function Get-ProcessSummary {
    param([int]$ProcessId)
    $p = Get-CimInstance Win32_Process -Filter "ProcessId=$ProcessId" -ErrorAction SilentlyContinue
    if (-not $p) { return $null }
    return $p
}

# --- TOML (just enough for config\jarvis.toml) -------------------------------

function ConvertFrom-TomlScalar {
    param([string]$Raw)
    $v = $Raw.Trim()
    if ($v.Length -eq 0) { return $null }
    $first = $v[0]
    if ($first -eq '"') {
        $sb = New-Object System.Text.StringBuilder
        $i = 1
        while ($i -lt $v.Length) {
            $c = $v[$i]
            if ($c -eq '\' -and ($i + 1) -lt $v.Length) {
                $n = $v[$i + 1]
                switch -CaseSensitive ($n) {
                    'n' { [void]$sb.Append("`n") }
                    't' { [void]$sb.Append("`t") }
                    default { [void]$sb.Append($n) }
                }
                $i += 2
                continue
            }
            if ($c -eq '"') { break }
            [void]$sb.Append($c)
            $i++
        }
        return $sb.ToString()
    }
    if ($first -eq "'") {
        $end = $v.IndexOf("'", 1)
        if ($end -lt 0) { return $v.Substring(1) }
        return $v.Substring(1, $end - 1)
    }
    $hash = $v.IndexOf('#')
    if ($hash -ge 0) { $v = $v.Substring(0, $hash).Trim() }
    if ($v -eq 'true') { return $true }
    if ($v -eq 'false') { return $false }
    if ($v -match '^[+-]?\d[\d_]*$') { return [int64]($v -replace '_', '') }
    if ($v -match '^[+-]?\d[\d_]*\.\d+$') { return [double]($v -replace '_', '') }
    return $v    # arrays and inline tables stay as raw text; the scripts do not need them
}

# Reads a TOML file into @{ Sections = @{ 'name' = @{ key = value } }; Arrays = @{ 'a.b' = @(@{...}, ...) } }.
# The root table is Sections['']. It understands tables, arrays of tables, quoted keys,
# comments and the scalar types. It is a checker's reader, not a full TOML parser.
function Read-TomlLite {
    param([Parameter(Mandatory = $true)][string]$Path)
    $sections = @{ '' = @{} }
    $arrays = @{}
    $current = $sections['']
    foreach ($raw in (Get-Content -LiteralPath $Path -Encoding UTF8)) {
        if ($raw -match '^\s*\[\[\s*([^\]]+?)\s*\]\]\s*(#.*)?$') {
            $name = $Matches[1]
            if (-not $arrays.ContainsKey($name)) { $arrays[$name] = @() }
            $table = @{}
            $arrays[$name] += , $table
            $current = $table
            continue
        }
        if ($raw -match '^\s*\[\s*([^\[\]]+?)\s*\]\s*(#.*)?$') {
            $name = $Matches[1]
            if (-not $sections.ContainsKey($name)) { $sections[$name] = @{} }
            $current = $sections[$name]
            continue
        }
        if ($raw -match '^\s*(?:"([^"]+)"|''([^'']+)''|([A-Za-z0-9_\-\.]+))\s*=\s*(.*)$') {
            $key = $Matches[1]
            if (-not $key) { $key = $Matches[2] }
            if (-not $key) { $key = $Matches[3] }
            $current[$key] = ConvertFrom-TomlScalar $Matches[4]
        }
    }
    return @{ Sections = $sections; Arrays = $arrays }
}

# One value, or $Default when the table or key is absent.
function Get-TomlValue {
    param($Toml, [string]$Section, [string]$Key, $Default = $null)
    if ($Toml -and $Toml.Sections.ContainsKey($Section) -and $Toml.Sections[$Section].ContainsKey($Key)) {
        return $Toml.Sections[$Section][$Key]
    }
    return $Default
}

# Returns the text of a TOML file with `Key = Literal` set inside [Section]. Replaces an existing
# uncommented line for the key, else inserts after the section's last key line, else appends a new
# section. Every other line is returned byte for byte, each with its own line ending (a file with
# mixed endings stays mixed). $Literal is the value already written as TOML (for example true).
function Set-TomlKeyText {
    param([string]$Text, [string]$Section, [string]$Key, [string]$Literal)
    $nl = if ($Text.Contains("`r`n")) { "`r`n" } else { "`n" }
    $lines = New-Object System.Collections.Generic.List[string]
    $ends = New-Object System.Collections.Generic.List[string]
    foreach ($m in [regex]::Matches($Text, '([^\r\n]*)(\r\n|\n|\r|$)')) {
        $lines.Add($m.Groups[1].Value)
        $ends.Add($m.Groups[2].Value)
    }
    if ($lines.Count -gt 0 -and $lines[$lines.Count - 1] -eq '' -and $ends[$ends.Count - 1] -eq '') {
        $lines.RemoveAt($lines.Count - 1)
        $ends.RemoveAt($ends.Count - 1)
    }
    $headerRx = '^\s*\[\s*' + [regex]::Escape($Section) + '\s*\]\s*(#.*)?$'
    $anyHeaderRx = '^\s*\[\[?[^\]]+\]\]?\s*(#.*)?$'
    $keyRx = '^\s*(?:"' + [regex]::Escape($Key) + '"|''' + [regex]::Escape($Key) + '''|' + [regex]::Escape($Key) + ')\s*='
    $newLine = "$Key = $Literal"
    $join = {
        $sb = New-Object System.Text.StringBuilder
        for ($i = 0; $i -lt $lines.Count; $i++) { [void]$sb.Append($lines[$i]).Append($ends[$i]) }
        $sb.ToString()
    }
    $start = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match $headerRx) { $start = $i; break }
    }
    if ($start -lt 0) {
        if ($lines.Count -gt 0 -and $ends[$ends.Count - 1] -eq '') { $ends[$ends.Count - 1] = $nl }
        foreach ($l in @('', "[$Section]", $newLine)) { $lines.Add($l); $ends.Add($nl) }
        return (& $join)
    }
    $end = $lines.Count
    for ($i = $start + 1; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match $anyHeaderRx) { $end = $i; break }
    }
    $lastKey = -1
    for ($i = $start + 1; $i -lt $end; $i++) {
        if ($lines[$i] -match $keyRx) {
            $lines[$i] = $newLine
            return (& $join)
        }
        if ($lines[$i] -match '^\s*[A-Za-z0-9_"''\-\.]+[^=]*=') { $lastKey = $i }
    }
    $after = $start
    if ($lastKey -ge 0) {
        $opens = ([regex]::Matches($lines[$lastKey], '\[')).Count
        $closes = ([regex]::Matches($lines[$lastKey], '\]')).Count
        if ($opens -le $closes) { $after = $lastKey }    # a multi-line array stays whole
    }
    $ending = $ends[$after]
    if ($ending -eq '') { $ending = $nl; $ends[$after] = $nl }
    $lines.Insert($after + 1, $newLine)
    $ends.Insert($after + 1, $ending)
    return (& $join)
}

# Writes UTF-8 without a BOM (Windows PowerShell's -Encoding utf8 would add one).
function Write-TextNoBom {
    param([string]$Path, [string]$Text)
    [System.IO.File]::WriteAllText($Path, $Text, (New-Object System.Text.UTF8Encoding($false)))
}

# --- .env --------------------------------------------------------------------

# KEY=value pairs of a .env file, read the way the API's loader reads them (comments, quotes,
# ` #` ends an unquoted value). The table is for CHECKS only: never print a value from it.
function Read-DotEnv {
    param([string]$Path)
    $values = @{}
    if (-not $Path -or -not (Test-Path -LiteralPath $Path)) { return $values }
    foreach ($raw in (Get-Content -LiteralPath $Path -Encoding UTF8)) {
        $line = $raw.Trim()
        if (-not $line -or $line.StartsWith('#')) { continue }
        if ($line.StartsWith('export ')) { $line = $line.Substring(7).Trim() }
        $idx = $line.IndexOf('=')
        if ($idx -lt 1) { continue }
        $key = $line.Substring(0, $idx).Trim()
        $val = $line.Substring($idx + 1).Trim()
        $quoted = $val.Length -ge 2 -and (($val[0] -eq '"' -and $val[$val.Length - 1] -eq '"') -or
            ($val[0] -eq "'" -and $val[$val.Length - 1] -eq "'"))
        if ($quoted) {
            $val = $val.Substring(1, $val.Length - 2)
        } else {
            if ($val.StartsWith('#')) { $val = '' }
            $hash = $val.IndexOf(' #')
            if ($hash -ge 0) { $val = $val.Substring(0, $hash).Trim() }
        }
        $values[$key] = $val
    }
    return $values
}

# The value the API would see: the process environment wins (the loader never overrides it), then .env.
function Get-EnvSetting {
    param([string]$Name, $DotEnv)
    $fromProcess = [Environment]::GetEnvironmentVariable($Name)
    if ($fromProcess) { return $fromProcess }
    if ($DotEnv -and $DotEnv.ContainsKey($Name)) { return [string]$DotEnv[$Name] }
    return $null
}

# Mirrors auth::crypto::validate_secret so a bad secret is caught before the API refuses to
# start. Returns $null when the secret is acceptable, else a short reason. Never echoes the secret.
function Test-JwtSecret {
    param([string]$Secret)
    if (-not $Secret) { return 'is empty' }
    $utf8 = [System.Text.Encoding]::UTF8
    $typed = $utf8.GetBytes($Secret)
    if ($typed.Length -lt 32) { return "is shorter than 32 bytes ($($typed.Length))" }
    $material = $typed
    if (($Secret.Length % 4) -eq 0 -and $Secret -match '^[A-Za-z0-9+/]+={0,2}$') {
        try {
            $decoded = [Convert]::FromBase64String($Secret)
            if ($decoded.Length -ge 32) { $material = $decoded }
        } catch { }
    }
    $distinct = New-Object 'System.Collections.Generic.HashSet[byte]'
    foreach ($b in $material) { [void]$distinct.Add($b) }
    if ($distinct.Count -lt 16) { return "holds only $($distinct.Count) different byte values (needs 16)" }
    foreach ($bytes in @($material, $typed)) {
        $run = 1
        for ($i = 1; $i -lt $bytes.Length; $i++) {
            if ($bytes[$i] -eq $bytes[$i - 1]) { $run++ } else { $run = 1 }
            if ($run -ge 8) { return 'contains 8 or more equal bytes in a row' }
        }
        $half = [int][math]::Floor($bytes.Length / 2)
        for ($p = 1; $p -le $half; $p++) {
            $repeats = $true
            for ($i = 0; $i -lt ($bytes.Length - $p); $i++) {
                if ($bytes[$i] -ne $bytes[$i + $p]) { $repeats = $false; break }
            }
            if ($repeats) { return 'is a short block repeated' }
        }
    }
    return $null
}

# 32 random bytes from the operating system's CSPRNG as base64 (what .env.example asks for).
function New-JwtSecret {
    $bytes = New-Object byte[] 48
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }
    $text = [Convert]::ToBase64String($bytes)
    [Array]::Clear($bytes, 0, $bytes.Length)
    return $text
}

# --- config ------------------------------------------------------------------

# -Config wins, then JARVIS_CONFIG, then config\jarvis.toml. A relative path is looked up from the
# current folder first and then from the repository root (the API's working directory).
function Resolve-ConfigPath {
    param([string]$Config)
    $p = $Config
    if (-not $p) { $p = $env:JARVIS_CONFIG }
    if (-not $p) { $p = 'config\jarvis.toml' }
    $p = $p.Trim().Trim('"')
    if (-not [System.IO.Path]::IsPathRooted($p)) {
        $fromCwd = Join-Path (Get-Location).ProviderPath $p
        if ($Config -and (Test-Path -LiteralPath $fromCwd)) { $p = $fromCwd } else { $p = Join-Path $Root $p }
    }
    try { return [System.IO.Path]::GetFullPath($p) } catch { return $p }
}

function Split-Bind {
    param([string]$Bind)
    $bindHost = '127.0.0.1'
    $port = $DefaultApiPort
    if ($Bind -match '^\[?(.+?)\]?:(\d+)$') {
        $bindHost = $Matches[1]
        $port = [int]$Matches[2]
    }
    return @{ Host = $bindHost; Port = $port }
}

# The settings the scripts check, with the API's defaults filled in. Never throws: a missing or
# unreadable file gives Exists = $false / Error set, and the caller reports it.
function Get-JarvisConfig {
    param([Parameter(Mandatory = $true)][string]$Path)
    $cfg = [ordered]@{
        Path = $Path; Exists = (Test-Path -LiteralPath $Path -PathType Leaf); Error = $null
        Mode = 'demo'; Bind = "127.0.0.1:$DefaultApiPort"; BindHost = '127.0.0.1'; Port = $DefaultApiPort
        Loopback = $true; StaticDir = $null; TlsCert = $null; TlsKey = $null; Scheme = 'http'
        ApiHost = '127.0.0.1'; ApiBase = "http://127.0.0.1:$DefaultApiPort"
        BridgeEndpoint = "tcp://127.0.0.1:$DefaultBridgePort"; BridgePort = $DefaultBridgePort; KeyFile = $null
        AuthEnabled = $true; CookieSecure = $null; DatabaseUrl = $null
        OllamaUrl = $DefaultOllamaUrl; OllamaModel = $DefaultOllamaModel; HasOllama = $false
        VoiceEnabled = $null; Toml = $null
    }
    if (-not $cfg.Exists) { return [pscustomobject]$cfg }
    try {
        $t = Read-TomlLite -Path $Path
    } catch {
        $cfg.Error = $_.Exception.Message
        return [pscustomobject]$cfg
    }
    $cfg.Toml = $t
    $cfg.Mode = [string](Get-TomlValue $t '' 'mode' 'demo')
    $cfg.Bind = [string](Get-TomlValue $t 'server' 'bind' $cfg.Bind)
    $b = Split-Bind $cfg.Bind
    $cfg.BindHost = $b.Host
    $cfg.Port = $b.Port
    $cfg.Loopback = ($b.Host -in @('127.0.0.1', 'localhost', '::1'))
    $cfg.StaticDir = Get-TomlValue $t 'server' 'static_dir'
    $cfg.TlsCert = Get-TomlValue $t 'server' 'tls_cert'
    $cfg.TlsKey = Get-TomlValue $t 'server' 'tls_key'
    if ($cfg.TlsCert) { $cfg.Scheme = 'https' }
    $cfg.ApiHost = $b.Host
    if ($b.Host -in @('0.0.0.0', '::', '*')) { $cfg.ApiHost = '127.0.0.1' }
    if ($cfg.ApiHost.Contains(':')) { $cfg.ApiHost = "[$($cfg.ApiHost)]" }
    $cfg.ApiBase = "$($cfg.Scheme)://$($cfg.ApiHost):$($b.Port)"
    $cfg.BridgeEndpoint = [string](Get-TomlValue $t 'bridge' 'endpoint' $cfg.BridgeEndpoint)
    if ($cfg.BridgeEndpoint -match ':(\d+)$') { $cfg.BridgePort = [int]$Matches[1] }
    $cfg.KeyFile = Get-TomlValue $t 'bridge' 'key_file'
    $cfg.AuthEnabled = [bool](Get-TomlValue $t 'auth' 'enabled' $true)
    $cfg.CookieSecure = Get-TomlValue $t 'auth' 'cookie_secure'
    $cfg.DatabaseUrl = Get-TomlValue $t 'database' 'url'
    $cfg.VoiceEnabled = Get-TomlValue $t 'voice' 'enabled'
    if ($t.Arrays.ContainsKey('llm.providers')) {
        foreach ($prov in $t.Arrays['llm.providers']) {
            if ($prov['kind'] -eq 'ollama') {
                $cfg.HasOllama = $true
                if ($prov['base_url']) { $cfg.OllamaUrl = ([string]$prov['base_url']).TrimEnd('/') }
                if ($prov['model']) { $cfg.OllamaModel = [string]$prov['model'] }
                break
            }
        }
    }
    return [pscustomobject]$cfg
}

# Path from the config (relative ones are relative to the repository root, the API's working directory).
function Resolve-RepoPath {
    param([string]$Path)
    if (-not $Path) { return $null }
    if ([System.IO.Path]::IsPathRooted($Path)) { return $Path }
    return (Join-Path $Root $Path)
}

# Is the first non-empty line 64 hex characters, not all zeros? Never prints it.
function Test-BridgeKeyText {
    param([string]$Text)
    $t = "$Text".Trim()
    return ($t -match '^[0-9A-Fa-f]{64}$') -and ($t -notmatch '^0{64}$')
}

# Where the bridge key comes from and whether it looks usable. The key itself is never returned.
function Get-BridgeKeyStatus {
    param($Config, $DotEnv)
    $result = @{ Source = 'none'; Ok = $false; Path = $null; Problem = $null }
    $fromEnv = Get-EnvSetting 'JARVIS_BRIDGE_KEY' $DotEnv
    if ($fromEnv) {
        $result.Source = 'JARVIS_BRIDGE_KEY'
        $result.Ok = Test-BridgeKeyText $fromEnv
        if (-not $result.Ok) { $result.Problem = 'JARVIS_BRIDGE_KEY is not 64 hex characters' }
        return $result
    }
    if (-not $Config.KeyFile) {
        $result.Problem = 'no [bridge] key_file in the config and no JARVIS_BRIDGE_KEY'
        return $result
    }
    $path = Resolve-RepoPath ([string]$Config.KeyFile)
    $result.Source = 'key_file'
    $result.Path = $path
    if ($path -match '<[^>]+>') {
        $result.Problem = 'key_file is still the placeholder from the example config'
        return $result
    }
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        $result.Problem = 'the key file does not exist'
        return $result
    }
    try {
        $line = Get-Content -LiteralPath $path -TotalCount 5 -ErrorAction Stop |
            Where-Object { $_.Trim() -ne '' } | Select-Object -First 1
        $result.Ok = Test-BridgeKeyText $line
        if (-not $result.Ok) { $result.Problem = 'the key file does not hold 64 hex characters' }
    } catch {
        $result.Problem = 'the key file cannot be read by this account'
    }
    return $result
}

# --- tools -------------------------------------------------------------------

function Find-Cloudflared {
    $cmd = Get-Command cloudflared -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    foreach ($candidate in @(
            "${env:ProgramFiles(x86)}\cloudflared\cloudflared.exe",
            "$env:ProgramFiles\cloudflared\cloudflared.exe")) {
        if ($candidate -and (Test-Path -LiteralPath $candidate)) { return $candidate }
    }
    return $null
}

function Find-Ollama {
    $cmd = Get-Command ollama -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    foreach ($candidate in @("$env:LOCALAPPDATA\Programs\Ollama\ollama.exe", "$env:ProgramFiles\Ollama\ollama.exe")) {
        if ($candidate -and (Test-Path -LiteralPath $candidate)) { return $candidate }
    }
    return $null
}

# pnpm.cmd rather than the pnpm.ps1 shim: it still works when script execution is restricted.
function Find-Pnpm {
    $cmd = Get-Command pnpm.cmd -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $cmd = Get-Command pnpm -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

# The newest build of the API binary: release first, then debug.
function Find-ApiExe {
    param([switch]$ReleaseOnly)
    $release = Join-Path $Root 'target\release\jarvis-api.exe'
    if (Test-Path -LiteralPath $release) { return $release }
    if ($ReleaseOnly) { return $null }
    $debug = Join-Path $Root 'target\debug\jarvis-api.exe'
    if (Test-Path -LiteralPath $debug) { return $debug }
    return $null
}

# "1.95.0" out of any version banner ("rustc 1.95.0 (abc 2026-04-14)", "v24.21.0").
function ConvertTo-VersionObject {
    param([string]$Text)
    if ($Text -match '(\d+)\.(\d+)(?:\.(\d+))?') {
        $patch = 0
        if ($Matches[3]) { $patch = [int]$Matches[3] }
        return New-Object System.Version ([int]$Matches[1]), ([int]$Matches[2]), $patch
    }
    return $null
}

# --- processes of THIS repository ----------------------------------------------

# The Windows service this repo installs (JarvisMT), or $null.
function Get-JarvisService {
    return (Get-CimInstance Win32_Service -Filter "Name='$ServiceName'" -ErrorAction SilentlyContinue)
}

# Every jarvis-api.exe on the machine, flagged when it lives under this repo and when it is the
# service's own process (the service is managed with `jarvis-api service stop`, never killed).
function Get-ApiProcesses {
    $service = Get-JarvisService
    $found = @()
    foreach ($p in @(Get-CimInstance Win32_Process -Filter "Name='jarvis-api.exe'" -ErrorAction SilentlyContinue)) {
        $found += [pscustomobject]@{
            ProcessId   = [int]$p.ProcessId
            Path        = $p.ExecutablePath
            CommandLine = $p.CommandLine
            Started     = $p.CreationDate
            InRepo      = (Test-PathUnder $p.ExecutablePath $Root)
            IsService   = [bool]($service -and $service.ProcessId -eq $p.ProcessId)
        }
    }
    return $found
}

# Vite / pnpm / node / esbuild processes whose command line points into this repo's web\ folder.
function Get-DevServerProcesses {
    $webDir = Join-Path $Root 'web'
    $pattern = [regex]::Escape($webDir)
    $found = @()
    $filter = "Name='node.exe' OR Name='esbuild.exe' OR Name='cmd.exe'"
    foreach ($p in @(Get-CimInstance Win32_Process -Filter $filter -ErrorAction SilentlyContinue)) {
        $cl = "$($p.CommandLine)"
        $inWeb = ($cl -match $pattern) -or (Test-PathUnder $p.ExecutablePath $webDir)
        if (-not $inWeb) { continue }
        if ($p.Name -ne 'esbuild.exe' -and $cl -notmatch '\b(vite|dev|preview)\b') { continue }
        $found += [pscustomobject]@{
            ProcessId = [int]$p.ProcessId; Name = $p.Name; CommandLine = $cl; Started = $p.CreationDate
        }
    }
    return $found
}

# The Cloudflare quick tunnel started by remote-start.ps1. It is recognised by its --logfile
# argument, a path inside this repo's .local\ folder, so no other cloudflared on the machine
# (another project's tunnels, for instance) can ever match.
$RemoteTunnelLog = Join-Path $LocalDir 'remote-tunnel.log'
$RemoteUrlFile = Join-Path $LocalDir 'remote-url.txt'

function Get-RemoteTunnelProcesses {
    $pattern = [regex]::Escape($RemoteTunnelLog)
    $found = @()
    foreach ($p in @(Get-CimInstance Win32_Process -Filter "Name='cloudflared.exe'" -ErrorAction SilentlyContinue)) {
        if ("$($p.CommandLine)" -match $pattern) {
            $found += [pscustomobject]@{
                ProcessId = [int]$p.ProcessId; CommandLine = $p.CommandLine; Started = $p.CreationDate
            }
        }
    }
    return $found
}

# --- HTTP --------------------------------------------------------------------

# GET without a proxy and without Invoke-WebRequest's start-up cost. Never throws.
# Returns Ok (a 2xx answer), Status (0 = no answer), Json (parsed body, when it is JSON), Error.
function Invoke-HttpGet {
    param([Parameter(Mandatory = $true)][string]$Url, [int]$TimeoutSec = 3)
    $result = @{ Ok = $false; Status = 0; Json = $null; Error = $null }
    $response = $null
    try {
        $req = [System.Net.HttpWebRequest]::Create($Url)
        $req.Method = 'GET'
        $req.Proxy = $null
        $req.Timeout = $TimeoutSec * 1000
        $req.ReadWriteTimeout = $TimeoutSec * 1000
        $req.AllowAutoRedirect = $false
        $response = $req.GetResponse()
    } catch [System.Net.WebException] {
        $response = $_.Exception.Response
        if (-not $response) { $result.Error = $_.Exception.Message; return $result }
    } catch {
        $result.Error = $_.Exception.Message
        return $result
    }
    try {
        $result.Status = [int]$response.StatusCode
        $result.Ok = ($result.Status -ge 200 -and $result.Status -lt 300)
        $reader = New-Object System.IO.StreamReader($response.GetResponseStream())
        try { $body = $reader.ReadToEnd() } finally { $reader.Dispose() }
        try { $result.Json = $body | ConvertFrom-Json } catch { }
    } catch {
        $result.Error = $_.Exception.Message
    } finally {
        $response.Close()
    }
    return $result
}

# /api/health and /api/status folded into one flat object. /api/status needs a session when login
# is on; then LoginRequired is set and only the health answer is known.
function Get-ApiSummary {
    param([Parameter(Mandatory = $true)][string]$BaseUrl, [int]$TimeoutSec = 3)
    $s = [ordered]@{
        BaseUrl = $BaseUrl; Healthy = $false; HealthError = $null; StatusAvailable = $false; LoginRequired = $false
        Version = $null; Mode = $null; ModeMismatch = $null; KillSwitchEngaged = $null; KillSwitchReason = $null
        Persona = $null; BridgeConnected = $null; EaVersion = $null; TradeMode = $null; TerminalConnected = $null
        BridgeError = $null; Stt = $null; Tts = $null; AuthEnabled = $null
        DefaultProvider = $null; Providers = @()
    }
    $h = Invoke-HttpGet "$BaseUrl/api/health" $TimeoutSec
    $s.Healthy = [bool]($h.Ok -and $h.Json -and $h.Json.ok -eq $true)
    if (-not $s.Healthy) {
        $s.HealthError = if ($h.Error) { $h.Error } else { "HTTP $($h.Status)" }
        return [pscustomobject]$s
    }
    # The status call pings the bridge, which can take bridge.timeout_ms when MT5 is down.
    $r = Invoke-HttpGet "$BaseUrl/api/status" ($TimeoutSec + 6)
    if ($r.Status -eq 401) { $s.LoginRequired = $true; return [pscustomobject]$s }
    if (-not $r.Ok -or -not $r.Json) { return [pscustomobject]$s }
    $j = $r.Json
    $s.StatusAvailable = $true
    $s.Version = $j.version
    $s.Mode = $j.mode
    $s.ModeMismatch = $j.mode_mismatch
    $s.KillSwitchEngaged = $j.kill_switch.engaged
    $s.KillSwitchReason = $j.kill_switch.reason
    if ($j.persona) { $s.Persona = $j.persona.name }
    if ($j.bridge) {
        $s.BridgeConnected = $j.bridge.connected
        $s.EaVersion = $j.bridge.ea_version
        $s.TradeMode = $j.bridge.trade_mode
        $s.TerminalConnected = $j.bridge.terminal_connected
        $s.BridgeError = $j.bridge.error
    }
    if ($j.voice) { $s.Stt = $j.voice.stt; $s.Tts = $j.voice.tts }
    if ($j.auth) { $s.AuthEnabled = $j.auth.enabled }
    if ($j.llm) {
        $s.DefaultProvider = $j.llm.default
        $s.Providers = @($j.llm.providers | ForEach-Object {
                [pscustomobject]@{ name = $_.name; kind = $_.kind; model = $_.model; available = $_.available }
            })
    }
    return [pscustomobject]$s
}

# Prints the bridge / mode / voice lines both run.ps1 and status.ps1 show.
function Write-ApiSummary {
    param($Summary)
    if (-not $Summary.Healthy) { Write-Fail "API not answering ($($Summary.HealthError))"; return }
    if ($Summary.LoginRequired) {
        Write-Ok 'API healthy (login is on: sign in to the web UI for the details)'
        return
    }
    if (-not $Summary.StatusAvailable) { Write-Warn 'API healthy, but /api/status gave no usable answer'; return }
    $modeLine = "mode $($Summary.Mode)"
    if ($Summary.ModeMismatch) { $modeLine += '  ** MODE MISMATCH: the terminal account does not match the configured mode **' }
    if ($Summary.ModeMismatch) { Write-Fail $modeLine } else { Write-Ok $modeLine }
    if ($Summary.KillSwitchEngaged) { Write-Warn "kill switch ENGAGED ($($Summary.KillSwitchReason)) -- writes are blocked" }
    if ($Summary.BridgeConnected) {
        Write-Ok "bridge connected (EA $($Summary.EaVersion), trade mode $($Summary.TradeMode), terminal connected: $($Summary.TerminalConnected))"
    } else {
        Write-Warn 'bridge NOT connected -- start MT5 and attach JarvisBridge (the API keeps running and retries on every request)'
    }
    Write-Ok "persona $($Summary.Persona), voice stt=$($Summary.Stt) tts=$($Summary.Tts), login $(if ($Summary.AuthEnabled) { 'on' } else { 'OFF' })"
}

# Ollama: is the server answering and is the model pulled?
function Get-OllamaState {
    param([string]$BaseUrl, [string]$Model)
    $state = @{ Url = $BaseUrl; Reachable = $false; ModelCount = 0; Model = $Model; HasModel = $false }
    $r = Invoke-HttpGet "$BaseUrl/api/tags" 3
    if (-not $r.Ok -or -not $r.Json) { return $state }
    $state.Reachable = $true
    $names = @($r.Json.models | ForEach-Object { $_.name })
    $state.ModelCount = $names.Count
    if ($Model) {
        $want = @($Model)
        if ($Model -notmatch ':') { $want += "${Model}:latest" }
        foreach ($n in $names) { if ($want -contains $n) { $state.HasModel = $true } }
    }
    return $state
}

# Asks the EA directly (read-only `ping`) through the bridge_cli example, when it has been built and
# the key is in a key_file. Used only when /api/status needs a login and so cannot say. The key is
# handed to bridge_cli as a file path; it is never read into this script.
function Get-BridgePing {
    param($Config)
    $r = @{ Available = $false; Connected = $false; EaVersion = $null; TradeMode = $null; Error = $null }
    $exe = $null
    foreach ($c in @('target\release\examples\bridge_cli.exe', 'target\debug\examples\bridge_cli.exe')) {
        $full = Join-Path $Root $c
        if (Test-Path -LiteralPath $full) { $exe = $full; break }
    }
    if (-not $exe -or -not $Config.KeyFile) { return $r }
    $keyPath = Resolve-RepoPath ([string]$Config.KeyFile)
    if (-not (Test-Path -LiteralPath $keyPath)) { return $r }
    $r.Available = $true
    $out = Join-Path ([System.IO.Path]::GetTempPath()) ('arcmt-ping-' + [guid]::NewGuid().ToString('N') + '.out')
    try {
        $pingArgs = @('--endpoint', $Config.BridgeEndpoint, '--key-file', "`"$keyPath`"", '--timeout-ms', '2500', 'ping')
        $p = Start-Process -FilePath $exe -ArgumentList $pingArgs -NoNewWindow -PassThru -RedirectStandardOutput $out -RedirectStandardError "$out.err"
        if (-not $p.WaitForExit(8000)) { Stop-ProcessTree -TargetId $p.Id; $r.Error = 'timed out'; return $r }
        $json = (Get-Content -LiteralPath $out -Raw -ErrorAction SilentlyContinue) | ConvertFrom-Json
        if ($json -and $json.ea_version) {
            $r.Connected = $true
            $r.EaVersion = $json.ea_version
            $r.TradeMode = $json.trade_mode
        } else {
            $r.Error = 'no answer from the EA'
        }
    } catch {
        $r.Error = 'no answer from the EA'
    } finally {
        Remove-Item -LiteralPath $out, "$out.err" -Force -ErrorAction SilentlyContinue -WhatIf:$false
    }
    return $r
}

# --- MetaTrader 5 -------------------------------------------------------------

# The data folder (%APPDATA%\MetaQuotes\Terminal\<id>) of a terminal.exe path, found through the
# origin.txt each data folder keeps. Portable installs keep the data next to the program.
function Find-Mt5DataFolder {
    param([string]$TerminalExe)
    $dir = Split-Path -Parent $TerminalExe
    if (Test-Path -LiteralPath (Join-Path $dir 'MQL5')) { return @($dir) }
    $hits = @()
    $base = Join-Path $env:APPDATA 'MetaQuotes\Terminal'
    foreach ($d in @(Get-ChildItem -LiteralPath $base -Directory -ErrorAction SilentlyContinue)) {
        $origin = Join-Path $d.FullName 'origin.txt'
        if (-not (Test-Path -LiteralPath $origin)) { continue }
        try {
            $text = [System.IO.File]::ReadAllText($origin).Trim([char]0xFEFF, [char]13, [char]10, [char]32, [char]0)
        } catch { continue }
        if ($text.TrimEnd('\') -ieq $dir.TrimEnd('\')) { $hits += $d.FullName }
    }
    return $hits
}

# Running terminal64.exe processes with the data folder(s) that belong to each.
function Get-RunningTerminals {
    $list = @()
    foreach ($p in @(Get-CimInstance Win32_Process -Filter "Name='terminal64.exe'" -ErrorAction SilentlyContinue)) {
        if (-not $p.ExecutablePath) { continue }
        $list += [pscustomobject]@{
            ProcessId = [int]$p.ProcessId; Path = $p.ExecutablePath
            DataFolders = @(Find-Mt5DataFolder $p.ExecutablePath)
        }
    }
    return $list
}

# A data folder given as a full path or as the 32-character terminal id.
function Resolve-DataFolderArgument {
    param([string]$Value)
    if ($Value -match '^[0-9A-Fa-f]{32}$') {
        return (Join-Path (Join-Path $env:APPDATA 'MetaQuotes\Terminal') $Value)
    }
    return $Value
}

# A log path for today that never overwrites an earlier run: logs\api-2026-10-30.log, then
# logs\api-2026-10-30-143012.log for a second start the same day.
function New-LogPath {
    param([string]$Prefix)
    New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
    $day = Get-Date -Format 'yyyy-MM-dd'
    $path = Join-Path $LogDir "$Prefix-$day.log"
    if ((Test-Path -LiteralPath $path) -and ((Get-Item -LiteralPath $path).Length -gt 0)) {
        $path = Join-Path $LogDir ("$Prefix-$day-" + (Get-Date -Format 'HHmmss') + '.log')
    }
    return $path
}

# Last lines of a log another process may still hold open for writing. Colour escape codes
# (the API's console log has them) are removed.
function Read-LogTail {
    param([string]$Path, [int]$Lines = 8)
    if (-not (Test-Path -LiteralPath $Path)) { return @() }
    try {
        $fs = [System.IO.File]::Open($Path, 'Open', 'Read', 'ReadWrite')
        try {
            $text = (New-Object System.IO.StreamReader($fs)).ReadToEnd()
        } finally { $fs.Dispose() }
        $text = [regex]::Replace($text, '\x1b\[[0-9;]*m', '')
        return @($text -split "\r?\n" | Where-Object { $_.Trim() -ne '' } | Select-Object -Last $Lines)
    } catch {
        return @()
    }
}

# The tail of a child's two log files (stdout log and its .err), for failure messages.
function Read-ChildLogTail {
    param([string]$LogPath, [int]$Lines = 8)
    $all = @(Read-LogTail $LogPath $Lines) + @(Read-LogTail "$LogPath.err" $Lines)
    return @($all | Select-Object -Last $Lines)
}

# Stop-Process returns before the process is really gone; wait (briefly) so a "still running"
# check afterwards is honest.
function Wait-ProcessGone {
    param([int]$ProcessId, [int]$TimeoutSec = 4)
    $until = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $until) {
        if (-not (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) { return $true }
        Start-Sleep -Milliseconds 100
    }
    return (-not (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue))
}

# Whole-file read that tolerates the writer holding the file open (cloudflared appending).
function Read-SharedFile {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return '' }
    try {
        $fs = [System.IO.File]::Open($Path, 'Open', 'Read', 'ReadWrite')
        try { return (New-Object System.IO.StreamReader($fs)).ReadToEnd() } finally { $fs.Dispose() }
    } catch {
        return ''
    }
}
