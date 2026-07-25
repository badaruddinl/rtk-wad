[CmdletBinding()]
param(
    [string]$AuditScript = (Join-Path $PSScriptRoot "..\scripts\audit-provider-baseline.ps1")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("rtk-wad-p18-preflight-" + [guid]::NewGuid())
try {
    New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
    $output = Join-Path $temporaryRoot "preflight.json"
    & $AuditScript -OutputPath $output
    if ($LASTEXITCODE -ne 0) { throw "Provider audit exited with $LASTEXITCODE." }
    $report = Get-Content -LiteralPath $output -Raw | ConvertFrom-Json
    if ($report.SchemaVersion -ne 1) { throw "Unexpected provider-audit schema." }
    if ($report.BenchmarkPreflight.Protocol -ne "benchmark-matrix-preflight-v1") {
        throw "P18 benchmark preflight protocol was not reported."
    }
    if ($report.BenchmarkPreflight.ManifestCommandCount -ne 69) {
        throw "Expected 69 manifest command families, got $($report.BenchmarkPreflight.ManifestCommandCount)."
    }
    if ($null -eq $report.BenchmarkPreflight.WindowsNativeRtkReady -or
        $null -eq $report.BenchmarkPreflight.Wsl1RtkReady -or
        $null -eq $report.BenchmarkPreflight.Wsl2RtkReady) {
        throw "P18 backend readiness fields are incomplete."
    }
    if ($null -eq $report.Manifest.WindowsCoverage) {
        throw "Native Windows RTK manifest evidence is missing."
    }
    if ($null -eq $report.SearchScope.WslRtkOverride) {
        throw "WSL RTK override audit evidence is missing."
    }
    Write-Output "P18 benchmark preflight contract passed."
}
finally {
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
