#Requires -RunAsAdministrator

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

Write-Output "Enabling the Windows Subsystem for Linux optional component required by WSL 1..."
& wsl.exe --install --enable-wsl1 --no-distribution
if ($LASTEXITCODE -ne 0) {
    throw "WSL 1 feature enablement failed with exit code $LASTEXITCODE."
}

Write-Output "WSL 1 feature enablement completed. Restart Windows if the command requested it, then run provision-wsl1.ps1."
