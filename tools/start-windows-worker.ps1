# SPDX-License-Identifier: AGPL-3.0-or-later
# Run under the Windows account that owns the profile data and keyring.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$AgentPath,
    [Parameter(Mandatory = $true)][string]$CorePath,
    [Parameter(Mandatory = $true)][string]$DataDirectory,
    [ValidateRange(1, 65535)][int]$ApiPort = 35000
)

$ErrorActionPreference = 'Stop'
foreach ($Executable in @($AgentPath, $CorePath)) {
    if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
        throw "Executable does not exist: $Executable"
    }
}
if ([string]::IsNullOrWhiteSpace($DataDirectory)) {
    throw 'DataDirectory must not be empty.'
}
$Agent = (Resolve-Path -LiteralPath $AgentPath).Path
$Core = (Resolve-Path -LiteralPath $CorePath).Path
$Data = [System.IO.Path]::GetFullPath($DataDirectory)
if (Test-Path -LiteralPath $Data -PathType Leaf) {
    throw "DataDirectory is a file: $Data"
}
[System.IO.Directory]::CreateDirectory($Data) | Out-Null
$env:FURY_HOME = $Data
$env:FURY_CORE = $Core
$env:FURY_API_PORT = [string]$ApiPort
& $Agent serve
exit $LASTEXITCODE
