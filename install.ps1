#!/usr/bin/env pwsh

<#
.SYNOPSIS
Installs Nebula from a verified Windows GitHub release.
.EXAMPLE
.\install.ps1
.EXAMPLE
.\install.ps1 -NoCli -Agent codex,claude
.EXAMPLE
.\install.ps1 -RepairHooks -Agent codex,claude
#>
[CmdletBinding()]
param(
    [ValidatePattern('^v?[0-9]+\.[0-9]+\.[0-9]+(?:[-.][A-Za-z0-9.-]+)?$|^latest$')]
    [string]$Version = $(if ($env:NEBULA_VERSION) { $env:NEBULA_VERSION } else { 'latest' }),

    [ValidatePattern('^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$')]
    [string]$Repo = $(if ($env:NEBULA_REPO) { $env:NEBULA_REPO } else { 'ennerperez/nebula' }),

    [string]$BinDir = $(if ($env:NEBULA_INSTALL_DIR) { $env:NEBULA_INSTALL_DIR } else { Join-Path $HOME '.local\bin' }),

    [ValidateSet('auto', 'all', 'codex', 'claude', 'gemini', 'opencode', 'generic', 'none')]
    [string[]]$Agent = $(if ($env:NEBULA_AGENT) { @($env:NEBULA_AGENT) } else { @('none') }),

    [string[]]$SkillDir = @(),
    [switch]$NoCli,
    [switch]$NoHooks,
    [switch]$RepairHooks
)

$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($BinDir)) { throw 'BinDir cannot be empty.' }

$architecture = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()) {
    'X64' { 'x86_64' }
    'Arm64' { 'aarch64' }
    default { throw 'Unsupported Windows architecture. Only x86_64 and aarch64 releases are available.' }
}
$archiveName = "nebula-windows-$architecture.zip"
$releaseBase = if ($Version -eq 'latest') {
    "https://github.com/$Repo/releases/latest/download"
} else {
    "https://github.com/$Repo/releases/download/$Version"
}

$roots = New-Object 'System.Collections.Generic.List[string]'
$selected = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)

function Add-SkillRoot([string]$Root) {
    if ([string]::IsNullOrWhiteSpace($Root)) { throw 'SkillDir cannot be empty.' }
    $fullPath = [System.IO.Path]::GetFullPath($Root)
    if (-not $script:roots.Contains($fullPath)) { $script:roots.Add($fullPath) }
}

function Add-Agent([string]$Name) {
    $null = $script:selected.Add($Name)
    switch ($Name) {
        'codex' {
            $homeDir = if ($env:CODEX_HOME) { $env:CODEX_HOME } else { Join-Path $HOME '.codex' }
            Add-SkillRoot (Join-Path $homeDir 'skills')
        }
        'claude' {
            $homeDir = if ($env:CLAUDE_CONFIG_DIR) { $env:CLAUDE_CONFIG_DIR } else { Join-Path $HOME '.claude' }
            Add-SkillRoot (Join-Path $homeDir 'skills')
        }
        'gemini' {
            $homeDir = if ($env:GEMINI_CLI_HOME) { $env:GEMINI_CLI_HOME } else { $HOME }
            Add-SkillRoot (Join-Path $homeDir '.gemini\skills')
        }
        'opencode' {
            $configHome = if ($env:XDG_CONFIG_HOME) { $env:XDG_CONFIG_HOME } else { Join-Path $HOME '.config' }
            Add-SkillRoot (Join-Path $configHome 'opencode\skills')
        }
        'generic' { Add-SkillRoot (Join-Path $HOME '.agents\skills') }
        'all' {
            foreach ($knownAgent in @('codex', 'claude', 'gemini', 'opencode')) { Add-Agent $knownAgent }
        }
    }
}

$installSkill = $true
if ($installSkill) {
    foreach ($root in $SkillDir) { Add-SkillRoot $root }
    foreach ($name in $Agent) {
        if ($name -ne 'auto' -and $name -ne 'none') { Add-Agent $name }
    }
    if ($roots.Count -eq 0 -and $Agent -contains 'auto') {
        $detected = @(
            @{ Name = 'codex'; Command = 'codex'; Home = $(if ($env:CODEX_HOME) { $env:CODEX_HOME } else { Join-Path $HOME '.codex' }) },
            @{ Name = 'claude'; Command = 'claude'; Home = $(if ($env:CLAUDE_CONFIG_DIR) { $env:CLAUDE_CONFIG_DIR } else { Join-Path $HOME '.claude' }) },
            @{ Name = 'gemini'; Command = 'gemini'; Home = $(if ($env:GEMINI_CLI_HOME) { Join-Path $env:GEMINI_CLI_HOME '.gemini' } else { Join-Path $HOME '.gemini' }) },
            @{ Name = 'opencode'; Command = 'opencode'; Home = $(if ($env:XDG_CONFIG_HOME) { Join-Path $env:XDG_CONFIG_HOME 'opencode' } else { Join-Path $HOME '.config\opencode' }) }
        )
        foreach ($candidate in $detected) {
            if ((Get-Command $candidate.Command -ErrorAction SilentlyContinue) -or (Test-Path -LiteralPath $candidate.Home -PathType Container)) {
                Add-Agent $candidate.Name
            }
        }
        if ($roots.Count -eq 0) {
            Add-Agent 'generic'
            Write-Host 'No supported agent detected; using generic Agent Skills directory.'
        }
    }
    if ($roots.Count -eq 0) { $installSkill = $false }
}
if ($NoCli -and -not $installSkill) { throw 'Nothing to install: select an agent or skill directory.' }

function Repair-WindowsHookCommands([string]$SettingsPath, [string]$BinaryPath, [string]$AgentHome, [hashtable]$Events) {
    $settings = Get-Content -LiteralPath $SettingsPath -Raw | ConvertFrom-Json
    $binaryToken = if ([regex]::IsMatch($BinaryPath, '^[A-Za-z0-9:/\\._-]+$')) {
        $BinaryPath.Replace('\', '/')
    } else {
        '"' + $BinaryPath.Replace('\', '/') + '"'
    }
    $binaryCommand = $binaryToken + ' --agent-home "' + $AgentHome.Replace('\', '/') + '" hook '
    $changed = $false
    foreach ($event in $Events.Keys) {
        $found = $false
        $entries = New-Object 'System.Collections.Generic.List[object]'
        foreach ($entry in $settings.hooks.$event) {
            $hooks = New-Object 'System.Collections.Generic.List[object]'
            foreach ($hook in $entry.hooks) {
                if ($hook.command -match "(?:^|[\\/])nebula\.exe(?:'|`"|\s).*\shook\s+$($Events[$event])$") {
                    if ($found) { $changed = $true; continue }
                    $found = $true
                    $expected = $binaryCommand + $Events[$event]
                    if ($hook.command -ne $expected) {
                        $hook.command = $expected
                        $changed = $true
                    }
                }
                $hooks.Add($hook)
            }
            if ($hooks.Count -gt 0) {
                $entry.hooks = @($hooks.ToArray())
                $entries.Add($entry)
            }
        }
        $settings.hooks.$event = @($entries.ToArray())
    }
    if (-not $changed) { return }
    $backup = "$SettingsPath.bak-nebula"
    if (-not (Test-Path -LiteralPath $backup)) { Copy-Item -LiteralPath $SettingsPath -Destination $backup }
    $temporarySettings = "$SettingsPath.nebula-$([guid]::NewGuid().ToString('N'))"
    $temporaryBackup = "$SettingsPath.nebula-backup-$([guid]::NewGuid().ToString('N'))"
    try {
        $json = $settings | ConvertTo-Json -Depth 100
        [System.IO.File]::WriteAllText($temporarySettings, $json + "`n", [System.Text.UTF8Encoding]::new($false))
        [System.IO.File]::Replace($temporarySettings, $SettingsPath, $temporaryBackup)
    } finally {
        if (Test-Path -LiteralPath $temporarySettings) { Remove-Item -LiteralPath $temporarySettings -Force }
        if (Test-Path -LiteralPath $temporaryBackup) { Remove-Item -LiteralPath $temporaryBackup -Force }
    }
    Write-Host "Repaired Windows hook commands: $SettingsPath"
}

function Configure-AgentHooks([string]$BinaryPath) {
    if ($selected.Contains('codex')) {
        $codexHome = if ($env:CODEX_HOME) { $env:CODEX_HOME } else { Join-Path $HOME '.codex' }
        $hooksPath = Join-Path $codexHome 'hooks.json'
        & $BinaryPath configure codex --hooks $hooksPath --binary $BinaryPath
        if ($LASTEXITCODE -ne 0) { throw 'Codex hook configuration failed.' }
        Repair-WindowsHookCommands $hooksPath $BinaryPath $codexHome @{ UserPromptSubmit = 'user-prompt-submit'; Stop = 'stop'; PermissionRequest = 'notification' }
    }
    if ($selected.Contains('claude')) {
        $claudeHome = if ($env:CLAUDE_CONFIG_DIR) { $env:CLAUDE_CONFIG_DIR } else { Join-Path $HOME '.claude' }
        $settingsPath = Join-Path $claudeHome 'settings.json'
        & $BinaryPath configure claude --settings $settingsPath --binary $BinaryPath
        if ($LASTEXITCODE -ne 0) { throw 'Claude Code hook configuration failed.' }
        Repair-WindowsHookCommands $settingsPath $BinaryPath $claudeHome @{ UserPromptSubmit = 'user-prompt-submit'; Stop = 'stop'; Notification = 'notification' }
    }
}

if ($RepairHooks) {
    if ($NoHooks -or -not ($selected.Contains('codex') -or $selected.Contains('claude'))) {
        throw 'RepairHooks requires -Agent codex or claude and cannot be combined with -NoHooks.'
    }
    $command = Get-Command nebula -ErrorAction SilentlyContinue
    $installedBinary = if ($command) { $command.Source } else { Join-Path ([System.IO.Path]::GetFullPath($BinDir)) 'nebula.exe' }
    if (-not (Test-Path -LiteralPath $installedBinary -PathType Leaf)) { throw "Nebula CLI not found: $installedBinary" }
    Configure-AgentHooks $installedBinary
    return
}

$temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("nebula-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temporary | Out-Null
try {
    $archive = Join-Path $temporary $archiveName
    $checksums = Join-Path $temporary 'SHA256SUMS'
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    Write-Host "Downloading $archiveName..."
    Invoke-WebRequest -Uri "$releaseBase/$archiveName" -OutFile $archive -UseBasicParsing
    Invoke-WebRequest -Uri "$releaseBase/SHA256SUMS" -OutFile $checksums -UseBasicParsing

    $checksumLine = Get-Content -LiteralPath $checksums | Where-Object {
        $_ -match "^([A-Fa-f0-9]{64})\s+\*?$([regex]::Escape($archiveName))\s*$"
    } | Select-Object -First 1
    if (-not $checksumLine) { throw "$archiveName is missing from SHA256SUMS." }
    $expected = [regex]::Match($checksumLine, '^([A-Fa-f0-9]{64})').Groups[1].Value
    $actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
    if ($actual -ine $expected) { throw "Checksum mismatch for $archiveName." }
    Write-Host "Verified $archiveName"

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead($archive)
    try {
        foreach ($entry in $zip.Entries) {
            $path = $entry.FullName.Replace('\', '/')
            $parts = $path.Split('/')
            if ($path.StartsWith('/') -or $path -match '^[A-Za-z]:' -or $parts -contains '..') {
                throw "Archive contains an unsafe path: $($entry.FullName)"
            }
        }
    } finally {
        $zip.Dispose()
    }

    $release = Join-Path $temporary 'release'
    Expand-Archive -LiteralPath $archive -DestinationPath $release
    $binary = Join-Path $release 'nebula.exe'
    $skillSource = Join-Path $release 'skills\nebula'
    if (-not $NoCli -and -not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw 'Release does not contain nebula.exe.' }
    if ($installSkill -and -not (Test-Path -LiteralPath (Join-Path $skillSource 'SKILL.md') -PathType Leaf)) {
        throw 'Release does not contain skills/nebula/SKILL.md.'
    }

    $installedBinary = $null
    if (-not $NoCli) {
        $destination = [System.IO.Path]::GetFullPath($BinDir)
        New-Item -ItemType Directory -Path $destination -Force | Out-Null
        $installedBinary = Join-Path $destination 'nebula.exe'
        Copy-Item -LiteralPath $binary -Destination $installedBinary -Force
        $versionOutput = & $installedBinary --version
        if ($LASTEXITCODE -ne 0) { throw "Installed CLI failed verification: $installedBinary" }
        Write-Host "Installed CLI: $installedBinary"
        Write-Host "Verified CLI: $versionOutput"
        if (@($env:PATH -split ';' | ForEach-Object { $_.TrimEnd('\') }) -notcontains $destination.TrimEnd('\')) {
            Write-Host "Add $destination to PATH to run nebula from any directory."
        }
    }

    if ($installSkill) {
        foreach ($root in $roots) {
            $destination = Join-Path $root 'nebula'
            New-Item -ItemType Directory -Path $destination -Force | Out-Null
            Get-ChildItem -LiteralPath $skillSource -Force | Copy-Item -Destination $destination -Recurse -Force
            Write-Host "Installed Agent Skill: $destination"
        }
    }

    if ($installSkill -and -not $NoHooks -and ($selected.Contains('codex') -or $selected.Contains('claude'))) {
        if (-not $installedBinary) {
            $command = Get-Command nebula -ErrorAction SilentlyContinue
            if ($command) { $installedBinary = $command.Source }
            if (-not $installedBinary) {
                $candidate = Join-Path ([System.IO.Path]::GetFullPath($BinDir)) 'nebula.exe'
                if (Test-Path -LiteralPath $candidate -PathType Leaf) { $installedBinary = $candidate }
            }
        }
        if ($installedBinary) {
            Configure-AgentHooks $installedBinary
        } else {
            Write-Warning 'Lifecycle hooks were not configured because nebula is not installed or on PATH.'
        }
    }
} finally {
    $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (([System.IO.Path]::GetFullPath($temporary)).StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
    }
}
