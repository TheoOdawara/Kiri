[CmdletBinding(SupportsShouldProcess)]
param(
    [ValidateSet('Ubuntu-22.04', 'Ubuntu-24.04', 'Ubuntu-26.04', 'Debian', 'Debian-12', 'Debian-13')]
    [string]$Distro
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$SupportedDistros = @(
    'Ubuntu-24.04',
    'Ubuntu-22.04',
    'Ubuntu-26.04',
    'Debian',
    'Debian-12',
    'Debian-13'
)

function Invoke-CheckedWsl {
    param(
        [Parameter(Mandatory)][string]$WslPath,
        [Parameter(Mandatory)][string[]]$Arguments,
        [Parameter(Mandatory)][string]$Operation
    )

    & $WslPath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Operation failed with exit code $LASTEXITCODE"
    }
}

function Invoke-WslOutput {
    param(
        [Parameter(Mandatory)][string]$WslPath,
        [Parameter(Mandatory)][string[]]$Arguments,
        [Parameter(Mandatory)][string]$Operation
    )

    $Output = & $WslPath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Operation failed with exit code $LASTEXITCODE"
    }
    return ($Output -join "`n").Trim([char]0).Trim()
}

function Select-WslDistro {
    param(
        [Parameter(Mandatory)][string]$WslPath,
        [string]$RequestedDistro
    )

    if ($RequestedDistro) {
        return $RequestedDistro
    }
    $Installed = @(& $WslPath --list --quiet) |
        ForEach-Object { $_.Trim([char]0).Trim() } |
        Where-Object { $_ -and $_ -notmatch '(?i)docker' }
    foreach ($Candidate in $SupportedDistros) {
        if ($Installed -contains $Candidate) {
            return $Candidate
        }
    }
    throw 'No supported WSL2 distribution found. Run `wsl --install -d Ubuntu-24.04`, complete its first launch, then retry.'
}

$RepositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$ManifestPath = Join-Path $RepositoryRoot 'Cargo.toml'
$CargoCommand = Get-Command cargo -ErrorAction Stop
$MetadataJson = & $CargoCommand.Source metadata --no-deps --format-version 1 --manifest-path $ManifestPath
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed with exit code $LASTEXITCODE"
}
$Metadata = $MetadataJson | ConvertFrom-Json
$Package = $Metadata.packages |
    Where-Object { [IO.Path]::GetFullPath($_.manifest_path) -eq $ManifestPath } |
    Select-Object -First 1
if (-not $Package -or $Package.version -notmatch '^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$') {
    throw 'Cargo.toml contains an invalid package version'
}
$Version = $Package.version

if ($WhatIfPreference) {
    Write-Output "Windows launcher: build Kiri $Version with cargo --release --locked"
    Write-Output "Linux payload: build Kiri $Version inside $Distro with cargo --release --locked"
    Write-Output "WSL runtime: cache the payload, select $Distro, and verify bubblewrap"
    return
}

if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw 'setup-windows-dev.ps1 must run on Windows'
}
$SystemRootValue = [Environment]::GetEnvironmentVariable('SystemRoot', 'Process')
if (-not $SystemRootValue) {
    throw 'SystemRoot is required to locate the trusted wsl.exe'
}
$WslPath = Join-Path $SystemRootValue 'System32\wsl.exe'
if (-not (Test-Path -LiteralPath $WslPath -PathType Leaf)) {
    throw 'WSL2 is required. Run `wsl --install -d Ubuntu-24.04`, reboot if requested, then retry.'
}
$LocalAppData = [Environment]::GetEnvironmentVariable('LOCALAPPDATA', 'Process')
if (-not $LocalAppData) {
    throw 'LOCALAPPDATA is required for the Kiri payload cache'
}
$SelectedDistro = Select-WslDistro -WslPath $WslPath -RequestedDistro $Distro

if (-not $PSCmdlet.ShouldProcess($SelectedDistro, 'Install development dependencies and build both Kiri runtimes')) {
    return
}

Invoke-CheckedWsl -WslPath $WslPath -Operation 'WSL initialization' -Arguments @(
    '--distribution', $SelectedDistro, '--exec', '/usr/bin/true'
)
Invoke-CheckedWsl -WslPath $WslPath -Operation 'apt update' -Arguments @(
    '--distribution', $SelectedDistro, '--user', 'root', '--exec',
    '/usr/bin/env', 'DEBIAN_FRONTEND=noninteractive', '/usr/bin/apt-get', 'update'
)
Invoke-CheckedWsl -WslPath $WslPath -Operation 'dependency installation' -Arguments @(
    '--distribution', $SelectedDistro, '--user', 'root', '--exec',
    '/usr/bin/env', 'DEBIAN_FRONTEND=noninteractive', '/usr/bin/apt-get', 'install', '-y',
    '--no-install-recommends', 'bubblewrap', 'iproute2', 'build-essential', 'pkg-config',
    'libdbus-1-dev', 'curl', 'ca-certificates'
)

& $WslPath --distribution $SelectedDistro --exec /bin/sh -c 'test -x "$HOME/.cargo/bin/cargo"'
if ($LASTEXITCODE -ne 0) {
    $RustupCommand = @(
        'set -eu',
        'installer="$(mktemp /tmp/kiri-rustup.XXXXXX)"',
        'trap ''rm -f "$installer"'' EXIT',
        '/usr/bin/curl --proto ''=https'' --tlsv1.2 --fail --silent --show-error https://sh.rustup.rs --output "$installer"',
        '/bin/sh "$installer" -y --profile minimal --default-toolchain stable'
    ) -join "`n"
    Invoke-CheckedWsl -WslPath $WslPath -Operation 'Rust installation' -Arguments @(
        '--distribution', $SelectedDistro, '--exec', '/bin/sh', '-c', $RustupCommand
    )
}

Push-Location $RepositoryRoot
try {
    & $CargoCommand.Source build --release --locked
    if ($LASTEXITCODE -ne 0) {
        throw "Windows launcher build failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}
Invoke-CheckedWsl -WslPath $WslPath -Operation 'Rust component installation' -Arguments @(
    '--distribution', $SelectedDistro, '--exec', '/bin/sh', '-c',
    '"$HOME/.cargo/bin/rustup" toolchain install stable --profile minimal --component rustfmt --component clippy'
)

$LinuxHome = Invoke-WslOutput -WslPath $WslPath -Operation 'WSL HOME discovery' -Arguments @(
    '--distribution', $SelectedDistro, '--exec', '/usr/bin/printenv', 'HOME'
)
$WslRepositoryRoot = Invoke-WslOutput -WslPath $WslPath -Operation 'repository path translation' -Arguments @(
    '--distribution', $SelectedDistro, '--exec', '/usr/bin/wslpath', '-a', '-u', $RepositoryRoot
)
$LinuxTargetRoot = "$LinuxHome/.cache/kiri/target"
$LinuxCargo = "$LinuxHome/.cargo/bin/cargo"
Invoke-CheckedWsl -WslPath $WslPath -Operation 'Linux payload build' -Arguments @(
    '--distribution', $SelectedDistro, '--exec', '/usr/bin/env',
    "CARGO_TARGET_DIR=$LinuxTargetRoot", $LinuxCargo, 'build',
    '--manifest-path', "$WslRepositoryRoot/Cargo.toml", '--release', '--locked'
)

$PayloadDirectory = Join-Path (Join-Path $LocalAppData 'Kiri\payloads') $Version
$CachedPayload = Join-Path $PayloadDirectory 'kiri'
New-Item -ItemType Directory -Force -Path $PayloadDirectory | Out-Null
$WslCachedPayload = Invoke-WslOutput -WslPath $WslPath -Operation 'payload path translation' -Arguments @(
    '--distribution', $SelectedDistro, '--exec', '/usr/bin/wslpath', '-a', '-u', $CachedPayload
)
Invoke-CheckedWsl -WslPath $WslPath -Operation 'payload cache installation' -Arguments @(
    '--distribution', $SelectedDistro, '--exec', '/usr/bin/install', '-D', '-m', '0755',
    "$LinuxTargetRoot/release/kiri", $WslCachedPayload
)

$WindowsLauncher = Join-Path $RepositoryRoot 'target\release\kiri.exe'
if (-not (Test-Path -LiteralPath $WindowsLauncher -PathType Leaf)) {
    throw 'Windows launcher build did not produce target\release\kiri.exe'
}
& $WindowsLauncher wsl use $SelectedDistro
if ($LASTEXITCODE -ne 0) {
    throw "WSL runtime repair failed with exit code $LASTEXITCODE"
}
& $WindowsLauncher wsl status
if ($LASTEXITCODE -ne 0) {
    throw "WSL runtime verification failed with exit code $LASTEXITCODE"
}

Write-Output 'Windows development runtime is ready. Run: cargo run'
