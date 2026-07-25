[CmdletBinding()]
param(
    [string]$Installer = (Join-Path $PSScriptRoot "..\scripts\install-tokenizer.ps1")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = Join-Path ([System.IO.Path]::GetTempPath()) ("rtk-wad-tokenizer-bootstrap-" + [guid]::NewGuid())
try {
    $plan = & $Installer -PlanPythonBootstrap | ConvertFrom-Json
    if ($plan.package -ne "Python.Python.3.12" -or $plan.source -ne "winget") {
        throw "Python bootstrap plan does not identify the approved package-manager dependency."
    }
    if (@($plan.arguments) -notcontains "--exact" -or @($plan.arguments) -notcontains "--accept-package-agreements") {
        throw "Python bootstrap plan is not sufficiently constrained."
    }

    $unconfirmed = $false
    try {
        & $Installer -Root $root -ForcePythonBootstrap -InstallPython
    } catch {
        $unconfirmed = $_.Exception.Message -like "*explicit confirmation*"
    }
    if (-not $unconfirmed) { throw "Unconfirmed Python bootstrap was not rejected." }
    if (Test-Path -LiteralPath $root) { throw "Unconfirmed Python bootstrap created a tokenizer environment." }

    Write-Output "Tokenizer bootstrap contract passed"
}
finally {
    if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
}
