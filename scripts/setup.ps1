# -----------------------------------------------------------------------------
# setup.ps1 -- idempotent first-time setup of Arc MT from a fresh clone.
#
# Why this exists: a new machine needs about a dozen separate steps (toolchains,
# config, secrets, voice models, web build, release build, EA install and compile,
# Ollama), and every missed one only shows up later as a confusing start-up
# failure. This script runs them in order, skips what is already done, and reports
# what it cannot fix itself. Running it twice is always safe.
#
#   1. Toolchains: rustup + a stable Rust at least as new as the rust-version in
#      Cargo.toml; Node 22 or newer; pnpm (unless -SkipWeb). With -Whisper also
#      Visual Studio Build Tools (C++), CMake and LLVM. Nothing is installed for you.
#   2. config\jarvis.toml is copied from config\jarvis.example.toml ONLY when missing.
#   3. .env is copied from .env.example ONLY when missing. Then JARVIS_JWT_SECRET is
#      filled with a fresh random value (48 bytes from the system CSPRNG, base64) ONLY
#      when it is empty. No other line is touched, and the secret is never printed.
#   4. Voice models: scripts\fetch-models.ps1 (hash-checked, skips what is present).
#   5. Web UI: pnpm -C web install --frozen-lockfile, then pnpm -C web build.
#   6. API: cargo build --release -p jarvis-api. With -Whisper it builds inside the MSVC
#      environment (vcvars64) with --features whisper and the release CMake flags.
#   7. EA: with -DataFolder (an MT5 data folder or its 32-character id), or when exactly
#      one terminal64.exe is running and its data folder can be found through the
#      origin.txt files under %APPDATA%\MetaQuotes\Terminal, runs install-ea.ps1 and
#      compile-ea.ps1 and prints the bridge key file path. Several terminals running
#      (this box has other projects' terminals) -> it does NOT guess; pass -DataFolder.
#      If [bridge] key_file is missing or still the example placeholder, it prints the
#      line to add; -WriteKeyFile writes it for you (a TOML literal string).
#   8. Ollama: reports whether it answers and whether the configured model is pulled.
#      It pulls the model only with -PullModel.
#   9. Next steps: create the first user, attach the EA, run scripts\run.ps1.
#
# Nothing here starts a server, places an order, registers a task, a service or a
# firewall rule.
#
#   scripts\setup.ps1 -WhatIf            print each step, change nothing
#   scripts\setup.ps1                    run the setup
#   scripts\setup.ps1 -SkipModels -SkipEa -SkipWeb -SkipBuild    only the toolchain check, config, .env and Ollama report
#   scripts\setup.ps1 -Whisper           also build speech-to-text (needs the MSVC build tools)
#   scripts\setup.ps1 -DataFolder <id>   install the EA into this terminal
#   scripts\setup.ps1 -PullModel         ollama pull the configured model if it is missing
# -----------------------------------------------------------------------------
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [switch]$SkipModels,
    [switch]$SkipEa,
    [switch]$SkipWeb,
    [switch]$SkipBuild,
    [switch]$Whisper,
    [string]$DataFolder,
    [switch]$PullModel,
    [switch]$WriteKeyFile
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lib\common.ps1')

$ConfigFile = Join-Path $Root 'config\jarvis.toml'
$ConfigExample = Join-Path $Root 'config\jarvis.example.toml'
$EnvFile = Join-Path $Root '.env'
$EnvExample = Join-Path $Root '.env.example'

$failures = New-Object System.Collections.Generic.List[string]    # things that stop the setup
$attention = New-Object System.Collections.Generic.List[string]   # things to look at afterwards

function Find-VisualStudioBuildTools {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) { return $null }
    $path = & $vswhere -latest -products '*' -requires 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64' -property installationPath
    if ($LASTEXITCODE -ne 0 -or -not $path) { return $null }
    return ($path | Select-Object -First 1)
}

function Find-LlvmBin {
    foreach ($candidate in @($env:LIBCLANG_PATH, 'C:\Program Files\LLVM\bin', 'C:\Program Files (x86)\LLVM\bin')) {
        if ($candidate -and (Test-Path -LiteralPath (Join-Path $candidate 'libclang.dll'))) { return $candidate }
    }
    return $null
}

function Find-CMake {
    $cmd = Get-Command cmake -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $fixed = 'C:\Program Files\CMake\bin\cmake.exe'
    if (Test-Path -LiteralPath $fixed) { return $fixed }
    return $null
}

# Runs a native command; a non-zero exit code is a failure of the whole setup.
function Invoke-Native {
    param([string]$Exe, [string[]]$ArgumentList = @())
    & $Exe @ArgumentList
    if ($LASTEXITCODE -ne 0) { throw "'$([System.IO.Path]::GetFileName($Exe)) $($ArgumentList -join ' ')' exited with code $LASTEXITCODE" }
}

Write-Banner "$AppName setup"
Push-Location $Root
try {
    # --- 1. toolchains ----------------------------------------------------------------------
    Write-Step 'Toolchains'
    $minRust = $null
    $cargoToml = Get-Content -LiteralPath (Join-Path $Root 'Cargo.toml') -Raw
    if ($cargoToml -match '(?m)^\s*rust-version\s*=\s*"([\d.]+)"') { $minRust = ConvertTo-VersionObject $Matches[1] }

    $rustup = Get-Command rustup -ErrorAction SilentlyContinue
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $rustup -or -not $cargo) {
        Write-Fail 'rustup / cargo not found -- install Rust (stable) from https://rustup.rs'
        $failures.Add('Rust is not installed')
    } else {
        # Run rustc from a neutral folder: inside the repo, rust-toolchain.toml would make rustup
        # download the toolchain, and a -WhatIf run must not install anything.
        $installed = @(& rustup toolchain list)
        if (-not ($installed | Where-Object { $_ -match '^stable' })) {
            Write-Fail 'no stable Rust toolchain installed -- run: rustup toolchain install stable'
            $failures.Add('stable Rust toolchain missing')
        } else {
            Push-Location ([System.IO.Path]::GetTempPath())
            try { $rustcLine = (& rustc +stable --version | Select-Object -First 1); $hostLine = (& rustc +stable -vV | Where-Object { $_ -match '^host:' } | Select-Object -First 1) } finally { Pop-Location }
            $have = ConvertTo-VersionObject "$rustcLine"
            if (-not $have) {
                Write-Fail "could not read the Rust version ($rustcLine)"
                $failures.Add('Rust version unreadable')
            } elseif ($minRust -and $have -lt $minRust) {
                Write-Fail "Rust $have is older than the $minRust this project needs -- run: rustup update stable"
                $failures.Add('Rust too old')
            } else {
                Write-Ok "$rustcLine (needs $minRust or newer)"
            }
            if ("$hostLine" -match 'msvc' -and -not (Find-VisualStudioBuildTools)) {
                Write-Warn 'Rust targets MSVC, but the Visual Studio Build Tools (C++ workload) were not found; linking will fail without them'
                $attention.Add('install the Visual Studio Build Tools (Desktop development with C++)')
            }
        }
    }

    $node = Get-Command node -ErrorAction SilentlyContinue
    if ($SkipWeb) {
        Write-Hint 'Node / pnpm not checked (-SkipWeb)'
    } else {
        if (-not $node) {
            Write-Fail 'Node.js not found -- install Node 22 or newer (https://nodejs.org)'
            $failures.Add('Node.js is not installed')
        } else {
            $nodeVersion = ConvertTo-VersionObject ((& node --version) | Select-Object -First 1)
            if (-not $nodeVersion -or $nodeVersion.Major -lt 22) {
                Write-Fail "Node $nodeVersion is older than 22"
                $failures.Add('Node too old')
            } else {
                Write-Ok "Node $nodeVersion"
            }
        }
        $pnpm = Find-Pnpm
        if (-not $pnpm) {
            Write-Fail 'pnpm not found -- run: npm install --global pnpm'
            $failures.Add('pnpm is not installed')
        } else {
            Write-Ok "pnpm $((& $pnpm --version) | Select-Object -First 1)"
        }
    }

    $vsPath = $null
    $llvmBin = $null
    if ($Whisper) {
        $vsPath = Find-VisualStudioBuildTools
        if ($vsPath) { Write-Ok "Visual Studio Build Tools: $vsPath" } else { Write-Fail 'Visual Studio Build Tools (Desktop development with C++) not found'; $failures.Add('Build Tools missing for -Whisper') }
        $cmake = Find-CMake
        if ($cmake) { Write-Ok "CMake: $cmake" } else { Write-Fail 'CMake not found (winget install Kitware.CMake)'; $failures.Add('CMake missing for -Whisper') }
        $llvmBin = Find-LlvmBin
        if ($llvmBin) { Write-Ok "LLVM (libclang.dll): $llvmBin" } else { Write-Fail 'LLVM / libclang.dll not found (winget install LLVM.LLVM)'; $failures.Add('LLVM missing for -Whisper') }
    }

    if ($failures.Count -gt 0) {
        Write-Host ''
        Write-Fail "$($failures.Count) prerequisite(s) missing: $($failures -join '; ')"
        if (-not $WhatIfPreference) { Write-Hint 'Install them and run this script again. Nothing was changed.'; exit 1 }
        Write-Hint '(-WhatIf: continuing to show the remaining steps)'
    }

    # --- 2. config -----------------------------------------------------------------------------
    Write-Step 'config\jarvis.toml'
    if (Test-Path -LiteralPath $ConfigFile) {
        Write-Ok 'already exists -- never overwritten by this script'
    } elseif ($PSCmdlet.ShouldProcess($ConfigFile, 'copy config\jarvis.example.toml')) {
        Copy-Item -LiteralPath $ConfigExample -Destination $ConfigFile -ErrorAction Stop
        Write-Ok 'created from config\jarvis.example.toml'
        $attention.Add('review config\jarvis.toml: [symbols] suffix and aliases for your broker, [limits], and static_dir')
    }

    # --- 3. .env and the JWT secret ----------------------------------------------------------------
    Write-Step '.env'
    $envCreated = $false
    if (Test-Path -LiteralPath $EnvFile) {
        Write-Ok 'already exists -- never overwritten by this script'
    } elseif ($PSCmdlet.ShouldProcess($EnvFile, 'copy .env.example')) {
        Copy-Item -LiteralPath $EnvExample -Destination $EnvFile -ErrorAction Stop
        $envCreated = $true
        Write-Ok 'created from .env.example'
    }
    if (Test-Path -LiteralPath $EnvFile) {
        $dot = Read-DotEnv $EnvFile
        if ($dot['JARVIS_JWT_SECRET']) {
            $why = Test-JwtSecret $dot['JARVIS_JWT_SECRET']
            if ($why) {
                Write-Warn "JARVIS_JWT_SECRET is set but $why -- the API will refuse to start; replace it (the line is left as it is)"
                $attention.Add('replace the weak JARVIS_JWT_SECRET in .env')
            } else {
                Write-Ok 'JARVIS_JWT_SECRET already set -- left as it is'
            }
        } elseif ($PSCmdlet.ShouldProcess($EnvFile, 'fill the empty JARVIS_JWT_SECRET with a random value')) {
            $secret = New-JwtSecret
            for ($i = 0; $i -lt 5 -and (Test-JwtSecret $secret); $i++) { $secret = New-JwtSecret }
            $text = [System.IO.File]::ReadAllText($EnvFile)
            $re = New-Object System.Text.RegularExpressions.Regex('(?m)^([ \t]*JARVIS_JWT_SECRET[ \t]*=)[ \t]*(?:""|'''')?[ \t]*(?:#[^\r\n]*)?(\r?)$')
            $hits = $re.Matches($text)
            if ($hits.Count -gt 0) {
                $m = $hits[$hits.Count - 1]
                $line = $m.Groups[1].Value + $secret + $m.Groups[2].Value
                $newText = $text.Substring(0, $m.Index) + $line + $text.Substring($m.Index + $m.Length)
            } else {
                $nl = if ($text.Contains("`r`n")) { "`r`n" } else { "`n" }
                if ($text.Length -gt 0 -and -not $text.EndsWith("`n")) { $text += $nl }
                $newText = $text + 'JARVIS_JWT_SECRET=' + $secret + $nl
            }
            Write-TextNoBom -Path $EnvFile -Text $newText
            $secret = $null; $newText = $null; $text = $null
            if ((Read-DotEnv $EnvFile)['JARVIS_JWT_SECRET']) {
                Write-Ok 'JARVIS_JWT_SECRET was empty: filled with a random value (not shown; it never leaves .env)'
            } else {
                Write-Fail 'could not write JARVIS_JWT_SECRET into .env -- set it by hand (see .env.example)'
                $attention.Add('set JARVIS_JWT_SECRET in .env by hand')
            }
        }
    } else {
        Write-Hint '(-WhatIf: .env would be created, then an empty JARVIS_JWT_SECRET filled with a random value)'
    }

    # --- 4. voice models ---------------------------------------------------------------------------------
    Write-Step 'Voice models'
    if ($SkipModels) {
        Write-Hint 'skipped (-SkipModels)'
    } elseif ($PSCmdlet.ShouldProcess('models\', 'scripts\fetch-models.ps1 (downloads Piper and Whisper models, hash-checked)')) {
        Invoke-Native $PowerShellExe @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $ScriptsDir 'fetch-models.ps1'))
        Write-Ok 'models ready'
    }

    # --- 5. web UI ---------------------------------------------------------------------------------------
    Write-Step 'Web UI'
    if ($SkipWeb) {
        Write-Hint 'skipped (-SkipWeb)'
    } elseif ($PSCmdlet.ShouldProcess('web\', 'pnpm install --frozen-lockfile, then pnpm build')) {
        Invoke-Native $pnpm @('-C', 'web', 'install', '--frozen-lockfile')
        Invoke-Native $pnpm @('-C', 'web', 'build')
        Write-Ok 'web\build is ready'
    }

    # --- 6. the API binary ---------------------------------------------------------------------------------
    Write-Step 'API (cargo build --release -p jarvis-api)'
    if ($SkipBuild) {
        Write-Hint 'skipped (-SkipBuild)'
    } elseif (-not $Whisper) {
        if ($PSCmdlet.ShouldProcess('target\release\jarvis-api.exe', 'cargo build --release -p jarvis-api')) {
            Invoke-Native (Get-Command cargo).Source @('build', '--release', '-p', 'jarvis-api')
            Write-Ok 'built target\release\jarvis-api.exe'
        }
    } elseif ($PSCmdlet.ShouldProcess('target\release\jarvis-api.exe', 'cargo build --release -p jarvis-api --features whisper (inside the MSVC environment)')) {
        $vcvars = Join-Path $vsPath 'VC\Auxiliary\Build\vcvars64.bat'
        if (-not (Test-Path -LiteralPath $vcvars)) { throw "vcvars64.bat not found under $vsPath" }
        # A throw-away batch file: it loads the MSVC environment and builds in the same cmd.exe, so
        # nothing leaks into this PowerShell session.
        $cmdFile = Join-Path ([System.IO.Path]::GetTempPath()) ('arcmt-whisper-build-' + [guid]::NewGuid().ToString('N') + '.cmd')
        $lines = @(
            '@echo off',
            "call `"$vcvars`" >nul || exit /b 1",
            'set "CMAKE_C_FLAGS_RELEASE=/O2 /Ob2 /DNDEBUG"',
            'set "CMAKE_CXX_FLAGS_RELEASE=/O2 /Ob2 /DNDEBUG"',
            "set `"LIBCLANG_PATH=$llvmBin`"",
            "cd /d `"$Root`"",
            'cargo build --release -p jarvis-api --features whisper'
        )
        [System.IO.File]::WriteAllText($cmdFile, (($lines -join "`r`n") + "`r`n"), [System.Text.Encoding]::ASCII)
        try {
            Invoke-Native (Join-Path $env:SystemRoot 'System32\cmd.exe') @('/d', '/c', "`"$cmdFile`"")
        } finally {
            Remove-Item -LiteralPath $cmdFile -Force -ErrorAction SilentlyContinue
        }
        Write-Ok 'built target\release\jarvis-api.exe with speech-to-text (whisper)'
    }

    # --- 7. the EA -------------------------------------------------------------------------------------------
    Write-Step 'MetaTrader 5 EA'
    $dataPath = $null
    $metaEditor = $null
    if ($SkipEa) {
        Write-Hint 'skipped (-SkipEa)'
    } else {
        if ($DataFolder) {
            $dataPath = Resolve-DataFolderArgument $DataFolder
            if (-not (Test-Path -LiteralPath (Join-Path $dataPath 'MQL5'))) {
                Write-Fail "not an MT5 data folder (no MQL5 subfolder): $dataPath"
                $failures.Add('-DataFolder is not an MT5 data folder')
                $dataPath = $null
            }
        } else {
            $terminals = @(Get-RunningTerminals)
            if ($terminals.Count -eq 1 -and $terminals[0].DataFolders.Count -eq 1) {
                $dataPath = $terminals[0].DataFolders[0]
                Write-Ok "exactly one terminal is running (pid $($terminals[0].ProcessId), $($terminals[0].Path)): using its data folder"
            } elseif ($terminals.Count -eq 0) {
                Write-Warn 'no MT5 terminal is running and no -DataFolder was given -- EA step skipped'
            } else {
                Write-Warn "$($terminals.Count) terminal(s) are running (or a data folder is ambiguous); not guessing which one is yours:"
                foreach ($t in $terminals) { Write-Hint "  pid $($t.ProcessId)  $($t.Path)  data: $(if ($t.DataFolders.Count) { $t.DataFolders -join ', ' } else { 'not found' })" }
            }
            if (-not $dataPath) {
                Write-Hint 'Pass -DataFolder <data folder or 32-character id>  (MT5: File > Open Data Folder).'
                $attention.Add('install the EA: scripts\setup.ps1 -DataFolder <id>  (or scripts\install-ea.ps1 + compile-ea.ps1)')
            }
        }
        if ($dataPath) {
            foreach ($t in @(Get-RunningTerminals)) {
                if ($t.DataFolders -contains $dataPath) { $metaEditor = Join-Path (Split-Path -Parent $t.Path) 'MetaEditor64.exe' }
            }
            if ($PSCmdlet.ShouldProcess($dataPath, 'scripts\install-ea.ps1 and scripts\compile-ea.ps1')) {
                Invoke-Native $PowerShellExe @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $ScriptsDir 'install-ea.ps1'), '-DataFolder', $dataPath)
                $compileArgs = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $ScriptsDir 'compile-ea.ps1'), '-DataFolder', $dataPath)
                if ($metaEditor -and (Test-Path -LiteralPath $metaEditor)) { $compileArgs += @('-MetaEditor', $metaEditor) }
                try {
                    Invoke-Native $PowerShellExe $compileArgs
                    Write-Ok 'EA installed and compiled'
                } catch {
                    Write-Fail "EA compile failed: $($_.Exception.Message)"
                    $attention.Add('fix the EA compile errors (scripts\compile-ea.ps1)')
                }
            }

            $keyPath = Join-Path $dataPath 'MQL5\Files\JarvisMT\bridge.key'
            Write-Host ''
            Write-Host "  bridge key file:  $keyPath" -ForegroundColor White
            Write-Hint '  (the key itself is never printed; keep this file private)'
            if (Test-Path -LiteralPath $ConfigFile) {
                $cfgNow = Get-JarvisConfig -Path $ConfigFile
                $current = [string]$cfgNow.KeyFile
                $isPlaceholder = $current -match '<[^>]+>'
                if ($current -and -not $isPlaceholder -and ($current -ieq $keyPath)) {
                    Write-Ok '[bridge] key_file in the config already points at it'
                } elseif ($current -and -not $isPlaceholder) {
                    Write-Warn "[bridge] key_file in the config points somewhere else ($current) -- left as it is; change it if this is the terminal you will use"
                    $attention.Add('check [bridge] key_file in config\jarvis.toml')
                } else {
                    $literal = if ($keyPath.Contains("'")) { '"' + ($keyPath.Replace('\', '\\').Replace('"', '\"')) + '"' } else { "'$keyPath'" }
                    if ($WriteKeyFile) {
                        if ($PSCmdlet.ShouldProcess($ConfigFile, "set [bridge] key_file = $literal")) {
                            $newText = Set-TomlKeyText -Text ([System.IO.File]::ReadAllText($ConfigFile)) -Section 'bridge' -Key 'key_file' -Literal $literal
                            Write-TextNoBom -Path $ConfigFile -Text $newText
                            Write-Ok "wrote [bridge] key_file to $ConfigFile"
                        }
                    } else {
                        Write-Warn '[bridge] key_file is missing (or still the example placeholder) in config\jarvis.toml. Add:'
                        Write-Host ''
                        Write-Host '      [bridge]' -ForegroundColor White
                        Write-Host "      key_file = $literal" -ForegroundColor White
                        Write-Host ''
                        Write-Hint 'or re-run with -WriteKeyFile and this script writes the line for you.'
                        $attention.Add('set [bridge] key_file in config\jarvis.toml (setup.ps1 -WriteKeyFile does it)')
                    }
                }
            }
        }
    }

    # --- 8. Ollama ----------------------------------------------------------------------------------------------
    Write-Step 'Ollama (local language model)'
    $cfgFinal = if (Test-Path -LiteralPath $ConfigFile) { Get-JarvisConfig -Path $ConfigFile } else { Get-JarvisConfig -Path $ConfigExample }
    $ollama = Get-OllamaState $cfgFinal.OllamaUrl $cfgFinal.OllamaModel
    if (-not $ollama.Reachable) {
        $haveExe = Find-Ollama
        if ($haveExe) { Write-Warn "installed but not answering at $($cfgFinal.OllamaUrl) -- start it (ollama serve, or run.ps1 -StartOllama)" }
        else { Write-Warn 'not installed (https://ollama.com). Fine if you use a cloud provider (ANTHROPIC_API_KEY / OPENAI_API_KEY in .env).' }
        if ($PullModel) { Write-Warn '-PullModel needs the Ollama server running; start it and run again' }
    } elseif ($ollama.HasModel) {
        Write-Ok "answers at $($cfgFinal.OllamaUrl); model $($cfgFinal.OllamaModel) is present"
    } else {
        Write-Warn "answers, but model $($cfgFinal.OllamaModel) is not pulled"
        if ($PullModel) {
            if ($PSCmdlet.ShouldProcess($cfgFinal.OllamaModel, 'ollama pull')) {
                Invoke-Native (Find-Ollama) @('pull', $cfgFinal.OllamaModel)
                Write-Ok 'model pulled'
            }
        } else {
            Write-Hint "pull it with: ollama pull $($cfgFinal.OllamaModel)   (or re-run with -PullModel)"
            $attention.Add("ollama pull $($cfgFinal.OllamaModel)")
        }
    }

    # --- 9. summary --------------------------------------------------------------------------------------------
    Write-Host ''
    if ($failures.Count -gt 0) {
        Write-Fail "setup finished with $($failures.Count) problem(s): $($failures -join '; ')"
    } elseif ($attention.Count -gt 0) {
        Write-Warn "setup finished, but $($attention.Count) item(s) need attention:"
        foreach ($a in $attention) { Write-Hint "- $a" }
    } else {
        Write-Ok 'setup finished'
    }
    Write-Host ''
    Write-Host '  Next steps' -ForegroundColor White
    Write-Host '    1. Create the first login user (asks for a password and shows the authenticator QR code):' -ForegroundColor DarkGray
    Write-Host '         target\release\jarvis-api.exe user add <name>' -ForegroundColor DarkGray
    Write-Host '    2. In MT5, attach JarvisBridge to a chart (Navigator > Expert Advisors > JarvisMT) with "Allow DLL imports"' -ForegroundColor DarkGray
    Write-Host '       ticked for this EA only, and the Algo Trading button on. Use a DEMO account first.' -ForegroundColor DarkGray
    Write-Host '    3. scripts\run.ps1            start the stack (Ctrl+C stops it); scripts\status.ps1 checks it; scripts\stop.ps1 is the fallback' -ForegroundColor DarkGray
    Write-Host '    4. scripts\install-autostart.ps1   optional: start at Windows logon (or `jarvis-api service install`, see docs\windows-service.md)' -ForegroundColor DarkGray
    if ($WhatIfPreference) {
        Write-Host ''
        Write-Host '  -WhatIf run: nothing was changed.' -ForegroundColor Yellow
    }
    Write-Host ''
    if ($failures.Count -gt 0) { exit 1 }
} finally {
    Pop-Location
}
