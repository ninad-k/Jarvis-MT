# -----------------------------------------------------------------------------
# firewall.ps1 -- Windows Firewall rules for a remotely reachable Arc MT (a VPS,
# or a PC you reach from your phone over a VPN / a fixed address). Elevated.
#
# It manages exactly two inbound rules, both named "Arc MT ...":
#
#   1. an ALLOW rule for the API's HTTPS port (default: the port in [server] bind,
#      8787), scoped to -RemoteAddress. -RemoteAddress is REQUIRED and must list
#      real IP addresses / CIDRs / ranges; "Any", "*", 0.0.0.0/0 and ::/0 are
#      refused, because the whole point of the rule is to NOT be open to the world.
#      It only makes sense together with TLS and login: the API itself refuses a
#      non-loopback bind without both, and this script warns when the config has no
#      tls_cert, has login disabled, or still binds to loopback (then no rule matters).
#   2. an explicit BLOCK rule for the bridge port 5555 on all profiles. Defense in
#      depth: the EA binds loopback anyway, and Windows Firewall does not filter
#      loopback traffic, so the API <-> EA link keeps working. In Windows Firewall a
#      block rule beats an allow rule, so a careless "allow 5555" elsewhere cannot
#      open the order channel.
#
# What it does NOT do:
#   * it never changes, disables or deletes any OTHER firewall rule. Other enabled
#     inbound allow rules that already open these ports -- above all ones that allow
#     them from ANY address, which make rule 1 moot -- are LISTED, and you decide;
#   * it does not touch the config, .env, the EA or any process.
#
# A -WhatIf run only reads, so it works from an unelevated prompt (the list of other
# rules may then be incomplete -- Windows hides some filters from non-administrators);
# anything that changes the firewall needs an elevated prompt and says so up front.
#
#   scripts\firewall.ps1 -RemoteAddress 203.0.113.7 -WhatIf        preview
#   scripts\firewall.ps1 -RemoteAddress 203.0.113.7,198.51.100.0/24    create / update both rules
#   scripts\firewall.ps1 -RemoteAddress 203.0.113.7 -Ports 443         the port a reverse proxy listens on
#   scripts\firewall.ps1 -Remove                                       delete both rules
#
# Afterwards, verify from OUTSIDE the box: a request from an address not on the list
# must time out, a request from a listed one must reach the sign-in page, and port 5555
# must never answer.
# -----------------------------------------------------------------------------
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    # Source IPs / CIDRs / ranges allowed to reach the API port. Required unless -Remove.
    [string[]]$RemoteAddress = @(),
    # The port(s) to open. Default: the port of [server] bind in the config.
    [int[]]$Ports,
    # The port to block. Default: the port of [bridge] endpoint (5555).
    [int]$BridgePort = 0,
    [string]$Config,
    [string]$AllowRuleName = 'Arc MT API (HTTPS, restricted)',
    [string]$BlockRuleName = 'Arc MT Bridge (block inbound)',
    [switch]$Remove
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lib\common.ps1')
Import-ModuleQuiet NetSecurity

Write-Banner "$AppName - firewall"

# A -WhatIf run only reads, so it works unelevated; anything that would change the firewall
# needs admin and says so up front instead of failing halfway.
if (-not $WhatIfPreference -and -not (Test-IsAdministrator)) {
    Write-Fail 'firewall.ps1: run this from an elevated (Administrator) PowerShell  (a -WhatIf preview works without one)'
    exit 1
}

$cfg = Get-JarvisConfig -Path (Resolve-ConfigPath $Config)
if (-not $Ports -or $Ports.Count -eq 0) { $Ports = @($cfg.Port) }
if ($BridgePort -le 0) { $BridgePort = $cfg.BridgePort }

$existingAllow = Get-NetFirewallRule -DisplayName $AllowRuleName -ErrorAction SilentlyContinue
$existingBlock = Get-NetFirewallRule -DisplayName $BlockRuleName -ErrorAction SilentlyContinue

# --- remove -----------------------------------------------------------------------------------
if ($Remove) {
    $any = $false
    foreach ($name in @($AllowRuleName, $BlockRuleName)) {
        $rule = Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue
        if (-not $rule) { Write-Hint "no rule named '$name' -- nothing to remove"; continue }
        $any = $true
        if ($PSCmdlet.ShouldProcess($name, 'Remove inbound firewall rule')) {
            Remove-NetFirewallRule -DisplayName $name
            Write-Ok "removed rule '$name'"
        }
    }
    if (-not $any) { Write-Host 'nothing to do' }
    exit 0
}

# --- validate -RemoteAddress -----------------------------------------------------------------
# Only real addresses: one IP, an IP/prefix, or an IPv4 range a.b.c.d-e.f.g.h. Anything that
# means "everyone" is refused.
function Test-RemoteAddressToken {
    param([string]$Token)
    $t = $Token.Trim()
    if (-not $t) { return 'is empty' }
    if ($t -match '^(any|\*|internet|localsubnet|defaultgateway|dns|dhcp|wins|intranet|ply)$') {
        return 'is a keyword, not an address (give real IPs or CIDRs)'
    }
    $ip = $null
    if ($t.Contains('/')) {
        $parts = $t.Split('/')
        if ($parts.Count -ne 2 -or -not [System.Net.IPAddress]::TryParse($parts[0], [ref]$ip)) { return 'is not a valid CIDR' }
        $prefix = 0
        if (-not [int]::TryParse($parts[1], [ref]$prefix)) { return 'has a non-numeric prefix' }
        $max = if ($ip.AddressFamily -eq 'InterNetworkV6') { 128 } else { 32 }
        if ($prefix -lt 1 -or $prefix -gt $max) { return "has an invalid prefix (1 to $max; /0 means everyone)" }
        if ($ip.AddressFamily -eq 'InterNetwork' -and $prefix -lt 8) { return "is wider than a /8 -- that is not 'restricted'" }
        return $null
    }
    if ($t.Contains('-')) {
        $parts = $t.Split('-')
        $a = $null; $b = $null
        if ($parts.Count -ne 2 -or -not [System.Net.IPAddress]::TryParse($parts[0], [ref]$a) -or -not [System.Net.IPAddress]::TryParse($parts[1], [ref]$b)) {
            return 'is not a valid address range'
        }
        return $null
    }
    if (-not [System.Net.IPAddress]::TryParse($t, [ref]$ip)) { return 'is not a valid IP address' }
    if ($ip.Equals([System.Net.IPAddress]::Any) -or $ip.Equals([System.Net.IPAddress]::IPv6Any)) { return 'is the any-address (0.0.0.0 / ::)' }
    return $null
}

if ($RemoteAddress.Count -eq 0) {
    Write-Fail '-RemoteAddress is required: the IPs / CIDRs that may reach the API (mobile-carrier addresses change: prefer a VPN or a fixed address).'
    Write-Hint 'Example: scripts\firewall.ps1 -RemoteAddress 203.0.113.7,198.51.100.0/24 -WhatIf'
    exit 1
}
$badTokens = 0
foreach ($t in $RemoteAddress) {
    $why = Test-RemoteAddressToken $t
    if ($why) { Write-Fail "-RemoteAddress '$t' $why"; $badTokens++ }
}
if ($badTokens -gt 0) { exit 1 }
$remote = @($RemoteAddress | ForEach-Object { $_.Trim() })

# --- is a remote rule meaningful for this config? ---------------------------------------------
Write-Step 'Config check'
if (-not $cfg.Exists) {
    Write-Warn "no config file at $($cfg.Path) -- cannot check TLS / login settings"
} else {
    if ($cfg.Loopback) {
        Write-Warn "[server] bind is $($cfg.Bind) (loopback): nothing outside this PC can reach the API, so the allow rule has no effect until bind is changed (or a reverse proxy on the opened port forwards to it)"
    }
    if (-not $cfg.TlsCert) {
        Write-Warn 'no [server] tls_cert: the API itself would serve plain HTTP. Open the port only if a TLS reverse proxy owns it, or set tls_cert + tls_key (docs\vps.md)'
    } else {
        Write-Ok 'TLS configured (tls_cert)'
    }
    if (-not $cfg.AuthEnabled) {
        Write-Warn '[auth] enabled = false: login is OFF. Never expose the API like this -- anyone allowed through the firewall could trade.'
    } else {
        Write-Ok 'login enabled'
    }
}

# --- the plan ---------------------------------------------------------------------------------------
Write-Step 'Rules'
Write-Host "    allow  '$AllowRuleName'  inbound TCP $($Ports -join ',')  from $($remote -join ', ')  (all profiles)" -ForegroundColor White
Write-Host "    block  '$BlockRuleName'  inbound TCP $BridgePort  from anywhere  (all profiles; loopback traffic is not filtered by Windows Firewall)" -ForegroundColor White

# --- create / update --------------------------------------------------------------------------------
if ($existingAllow) {
    if ($PSCmdlet.ShouldProcess($AllowRuleName, "Update rule: TCP $($Ports -join ',') from $($remote -join ', ')")) {
        Set-NetFirewallRule -DisplayName $AllowRuleName -Enabled True -Action Allow -Direction Inbound -Profile Any `
            -Protocol TCP -LocalPort $Ports -RemoteAddress $remote
        Write-Ok "updated rule '$AllowRuleName'"
    }
} elseif ($PSCmdlet.ShouldProcess($AllowRuleName, "Create rule: TCP $($Ports -join ',') from $($remote -join ', ')")) {
    New-NetFirewallRule -DisplayName $AllowRuleName `
        -Description 'Arc MT API: inbound HTTPS only from the listed addresses (scripts/firewall.ps1)' `
        -Direction Inbound -Action Allow -Protocol TCP -LocalPort $Ports -RemoteAddress $remote `
        -Profile Any -Enabled True | Out-Null
    Write-Ok "created rule '$AllowRuleName'"
}

if ($existingBlock) {
    if ($PSCmdlet.ShouldProcess($BlockRuleName, "Update rule: block TCP $BridgePort")) {
        Set-NetFirewallRule -DisplayName $BlockRuleName -Enabled True -Action Block -Direction Inbound -Profile Any `
            -Protocol TCP -LocalPort $BridgePort
        Write-Ok "updated rule '$BlockRuleName'"
    }
} elseif ($PSCmdlet.ShouldProcess($BlockRuleName, "Create rule: block TCP $BridgePort")) {
    New-NetFirewallRule -DisplayName $BlockRuleName `
        -Description 'Arc MT: the EA bridge port must never be reachable from the network (scripts/firewall.ps1)' `
        -Direction Inbound -Action Block -Protocol TCP -LocalPort $BridgePort `
        -Profile Any -Enabled True | Out-Null
    Write-Ok "created rule '$BlockRuleName'"
}

# --- other rules that already open these ports (read-only) ------------------------------------------
# A rule that allows one of these ports from ANY address defeats the scoped rule above. They are
# reported, never changed. The rule and port-filter lists are read once and joined on the rule id
# (asking per rule is many seconds slower on a machine with hundreds of rules); only the few rules
# that really name one of the ports are then asked for their address filter. Rules with
# "any local port" (every program-specific rule) are not listed: they name no port.
Write-Step 'Other enabled inbound allow rules that open these ports (not changed)'
$watch = @($Ports) + @($BridgePort)
$rules = @(Get-NetFirewallRule -Direction Inbound -Enabled True -Action Allow -ErrorAction SilentlyContinue)
$portFilters = @{}
foreach ($f in @(Get-NetFirewallPortFilter -ErrorAction SilentlyContinue)) { $portFilters[$f.InstanceID] = $f }

function Test-PortListCovers {
    param($LocalPort, [int]$Port)
    foreach ($entry in @($LocalPort)) {
        $e = "$entry"
        if ($e -eq "$Port") { return $true }
        if ($e -match '^(\d+)-(\d+)$' -and $Port -ge [int]$Matches[1] -and $Port -le [int]$Matches[2]) { return $true }
    }
    return $false
}

$others = @()
foreach ($rule in $rules) {
    if ($rule.DisplayName -eq $AllowRuleName -or $rule.DisplayName -eq $BlockRuleName) { continue }
    $pf = $portFilters[$rule.Name]
    if (-not $pf) { continue }
    if ("$($pf.Protocol)" -notin @('TCP', 'Any', '6')) { continue }
    $covered = @($watch | Where-Object { Test-PortListCovers $pf.LocalPort $_ })
    if ($covered.Count -eq 0) { continue }
    $af = $rule | Get-NetFirewallAddressFilter -ErrorAction SilentlyContinue
    $from = if ($af) { (@($af.RemoteAddress) -join ',') } else { 'unknown' }
    $others += [pscustomobject]@{ Name = $rule.DisplayName; Ports = ($covered -join ','); From = $from; Any = ($from -match '(^|,)Any(,|$)') }
}
if ($others.Count -eq 0) {
    Write-Ok 'none found'
} else {
    foreach ($o in ($others | Sort-Object Name -Unique)) {
        $line = "{0}  (ports {1}, from {2})" -f $o.Name, $o.Ports, $o.From
        if ($o.Any) { Write-Warn "$line  <- allows ANY address" } else { Write-Hint $line }
    }
    if (@($others | Where-Object { $_.Any -and ($_.Ports -split ',' | Where-Object { $_ -ne "$BridgePort" }) }).Count -gt 0) {
        Write-Warn 'a rule that allows these ports from ANY address makes the scoped allow rule above moot until it is disabled or narrowed (this script changes none of them)'
    }
    if (@($others | Where-Object { $_.Ports -split ',' -contains "$BridgePort" }).Count -gt 0) {
        Write-Hint "a block rule wins over allow rules, so the bridge port $BridgePort stays closed regardless"
    }
}
if (-not (Test-IsAdministrator)) {
    Write-Hint '(not elevated: Windows may hide some rule filters from this prompt, so the list can be incomplete)'
}

Write-Host ''
if ($WhatIfPreference) {
    Write-Host '  -WhatIf run: nothing was changed.' -ForegroundColor Yellow
} else {
    Write-Host "  Allowed sources: $($remote -join ', ')"
    Write-Host "  Verify from outside the box: an address not on the list must time out on port $($Ports -join '/'), and port $BridgePort must never answer."
}
Write-Host ''
