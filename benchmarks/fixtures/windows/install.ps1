[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$Destination,
    [string]$Rustc = "rustc.exe"
)

$ErrorActionPreference = "Stop"
$source = Join-Path $PSScriptRoot "argv_fixture.rs"
$destination = [System.IO.Path]::GetFullPath($Destination)
$fixture = Join-Path $destination "rtk-fixture.exe"
$commands = "aws", "curl", "docker", "gh", "glab", "kubectl", "oc", "psql", "wget"
New-Item -ItemType Directory -Force -Path $destination | Out-Null
& $Rustc $source -O -o $fixture
if ($LASTEXITCODE -ne 0) { throw "Unable to build the Windows argv fixture." }
foreach ($command in $commands) {
    Copy-Item -LiteralPath $fixture -Destination (Join-Path $destination "$command.exe") -Force
}
Write-Output "Installed Windows fixtures for $($commands -join ', ')."
