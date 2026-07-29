[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param(
        [Parameter(Mandatory)][bool]$Condition,
        [Parameter(Mandatory)][string]$Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

$SetupScript = Join-Path $PSScriptRoot 'setup-windows-dev.ps1'
$Tokens = $null
$ParseErrors = $null
[System.Management.Automation.Language.Parser]::ParseFile(
    $SetupScript,
    [ref]$Tokens,
    [ref]$ParseErrors
) | Out-Null
Assert-True ($ParseErrors.Count -eq 0) 'setup-windows-dev.ps1 must parse without errors'

$OriginalLocalAppData = $env:LOCALAPPDATA
$IsolatedLocalAppData = Join-Path ([IO.Path]::GetTempPath()) (
    'kiri-windows-dev-test-' + [Guid]::NewGuid().ToString('N')
)
try {
    $env:LOCALAPPDATA = $IsolatedLocalAppData
    $Plan = (& $SetupScript -Distro Ubuntu-24.04 -WhatIf | Out-String)

    Assert-True ($Plan -match 'Windows launcher') 'dry run must include the Windows launcher'
    Assert-True ($Plan -match 'Linux payload') 'dry run must include the Linux payload'
    Assert-True ($Plan -match 'WSL runtime') 'dry run must include WSL runtime repair'
    Assert-True (-not (Test-Path -LiteralPath $IsolatedLocalAppData)) 'dry run must not write the cache'

    $InvalidDistroRejected = $false
    try {
        & $SetupScript -Distro 'Ubuntu-24.04;echo injected' -WhatIf -ErrorAction Stop
    }
    catch {
        $InvalidDistroRejected = $true
    }
    Assert-True $InvalidDistroRejected 'unsupported distro input must be rejected'
}
finally {
    $env:LOCALAPPDATA = $OriginalLocalAppData
    if (Test-Path -LiteralPath $IsolatedLocalAppData) {
        $ResolvedTestPath = [IO.Path]::GetFullPath($IsolatedLocalAppData)
        $ResolvedTempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
        if (
            -not $ResolvedTestPath.StartsWith($ResolvedTempRoot, [StringComparison]::OrdinalIgnoreCase) -or
            (Split-Path -Leaf $ResolvedTestPath) -notlike 'kiri-windows-dev-test-*'
        ) {
            throw 'refusing to remove an unexpected test path'
        }
        Remove-Item -LiteralPath $ResolvedTestPath -Recurse -Force
    }
}

Write-Output 'setup-windows-dev tests passed'
