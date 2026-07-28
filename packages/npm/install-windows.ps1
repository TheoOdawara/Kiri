[CmdletBinding()]
param(
    [ValidatePattern('^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$')]
    [string]$Version,
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
$Repository = 'TheoOdawara/Kiri'
$WindowsAsset = 'kiri-windows-x86_64.exe'
$LinuxAsset = 'kiri-linux-x86_64'

function Invoke-CheckedWsl {
    param([Parameter(Mandatory)][string[]]$Arguments)

    & $script:WslPath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "wsl.exe failed with exit code ${LASTEXITCODE}: $($Arguments -join ' ')"
    }
}

function Get-PackageVersion {
    if ($Version) {
        return $Version
    }
    $Manifest = Join-Path $PSScriptRoot 'package.json'
    if (-not (Test-Path -LiteralPath $Manifest -PathType Leaf)) {
        throw 'Version is required when the installer is not run from the npm package.'
    }
    $Package = Get-Content -LiteralPath $Manifest -Raw | ConvertFrom-Json
    if (-not ($Package.version -is [string]) -or $Package.version -notmatch '^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$') {
        throw 'package.json contains an invalid version.'
    }
    return $Package.version
}

function Select-Distro {
    if ($Distro) {
        return $Distro
    }
    $Installed = @(& $script:WslPath --list --quiet) |
        ForEach-Object { $_.Trim([char]0).Trim() } |
        Where-Object { $_ -and $_ -notmatch '(?i)docker' }
    foreach ($Candidate in $SupportedDistros) {
        if ($Installed -contains $Candidate) {
            return $Candidate
        }
    }
    throw 'No supported WSL2 distribution found. Run `wsl --install -d Ubuntu-24.04`, complete its first launch, then retry.'
}

function Assert-Checksum {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$ChecksumFile
    )

    $EscapedName = [Regex]::Escape($Name)
    $Line = Get-Content -LiteralPath $ChecksumFile |
        Where-Object { $_ -match "^[0-9a-fA-F]{64}\s+\*?$EscapedName$" } |
        Select-Object -First 1
    if (-not $Line) {
        throw "SHA256SUMS has no entry for $Name"
    }
    $Expected = ($Line -split '\s+')[0].ToLowerInvariant()
    $Actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($Actual -ne $Expected) {
        throw "SHA-256 mismatch for $Name"
    }
}

function Install-FileAtomically {
    param(
        [Parameter(Mandatory)][string]$Source,
        [Parameter(Mandatory)][string]$Destination
    )

    $Temporary = "$Destination.tmp.$([Guid]::NewGuid().ToString('N'))"
    try {
        Copy-Item -LiteralPath $Source -Destination $Temporary
        Move-Item -LiteralPath $Temporary -Destination $Destination -Force
    }
    finally {
        if (Test-Path -LiteralPath $Temporary) {
            Remove-Item -LiteralPath $Temporary -Force
        }
    }
}

$SystemRootValue = [Environment]::GetEnvironmentVariable('SystemRoot', 'Process')
if (-not $SystemRootValue) {
    throw 'SystemRoot is required to locate the trusted wsl.exe.'
}
$script:WslPath = Join-Path $SystemRootValue 'System32\wsl.exe'
if (-not (Test-Path -LiteralPath $script:WslPath -PathType Leaf)) {
    throw 'WSL2 is required. Run `wsl --install -d Ubuntu-24.04`, reboot if requested, then retry.'
}
if (-not $env:LOCALAPPDATA) {
    throw 'LOCALAPPDATA is required to install Kiri.'
}
if (-not $env:TEMP) {
    throw 'TEMP is required to stage the Kiri installation.'
}

$ResolvedVersion = Get-PackageVersion
$SelectedDistro = Select-Distro
Invoke-CheckedWsl @('--distribution', $SelectedDistro, '--exec', '/usr/bin/true')

$WorkDirectory = Join-Path $env:TEMP ("kiri-install-" + [Guid]::NewGuid().ToString('N'))
$WorkDirectory = [IO.Path]::GetFullPath($WorkDirectory)
$TempRoot = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\'
if (-not $WorkDirectory.StartsWith($TempRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Refusing to stage outside TEMP.'
}
New-Item -ItemType Directory -Path $WorkDirectory | Out-Null

try {
    $ReleaseBase = "https://github.com/$Repository/releases/download/v$ResolvedVersion"
    $WindowsDownload = Join-Path $WorkDirectory $WindowsAsset
    $LinuxDownload = Join-Path $WorkDirectory $LinuxAsset
    $Checksums = Join-Path $WorkDirectory 'SHA256SUMS'
    Invoke-WebRequest -UseBasicParsing -Uri "$ReleaseBase/$WindowsAsset" -OutFile $WindowsDownload
    Invoke-WebRequest -UseBasicParsing -Uri "$ReleaseBase/$LinuxAsset" -OutFile $LinuxDownload
    Invoke-WebRequest -UseBasicParsing -Uri "$ReleaseBase/SHA256SUMS" -OutFile $Checksums
    Assert-Checksum -Path $WindowsDownload -Name $WindowsAsset -ChecksumFile $Checksums
    Assert-Checksum -Path $LinuxDownload -Name $LinuxAsset -ChecksumFile $Checksums

    $InstallRoot = Join-Path $env:LOCALAPPDATA 'Kiri'
    $BinDirectory = Join-Path $InstallRoot 'bin'
    $PayloadDirectory = Join-Path (Join-Path $InstallRoot 'payloads') $ResolvedVersion
    New-Item -ItemType Directory -Force -Path $BinDirectory, $PayloadDirectory | Out-Null
    $Launcher = Join-Path $BinDirectory 'kiri.exe'
    Install-FileAtomically -Source $WindowsDownload -Destination $Launcher
    Install-FileAtomically -Source $LinuxDownload -Destination (Join-Path $PayloadDirectory 'kiri')

    Invoke-CheckedWsl @(
        '--distribution', $SelectedDistro, '--user', 'root', '--exec',
        '/usr/bin/env', 'DEBIAN_FRONTEND=noninteractive', '/usr/bin/apt-get', 'update'
    )
    Invoke-CheckedWsl @(
        '--distribution', $SelectedDistro, '--user', 'root', '--exec',
        '/usr/bin/env', 'DEBIAN_FRONTEND=noninteractive', '/usr/bin/apt-get', 'install', '-y', '--no-install-recommends',
        'bubblewrap', 'iproute2'
    )

    & $Launcher wsl use $SelectedDistro
    if ($LASTEXITCODE -ne 0) {
        throw "Kiri launcher setup failed with exit code $LASTEXITCODE"
    }

    $UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $PathEntries = @($UserPath -split ';' | Where-Object { $_ })
    if ($PathEntries -notcontains $BinDirectory) {
        $UpdatedPath = (@($PathEntries) + $BinDirectory) -join ';'
        [Environment]::SetEnvironmentVariable('Path', $UpdatedPath, 'User')
    }
    Write-Host "Kiri $ResolvedVersion installed for $SelectedDistro. Open a new terminal and run: kiri wsl status"
}
finally {
    if (Test-Path -LiteralPath $WorkDirectory) {
        Remove-Item -LiteralPath $WorkDirectory -Recurse -Force
    }
}
