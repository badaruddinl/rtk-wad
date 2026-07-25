[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Wad,
    [Parameter(Mandatory = $true)]
    [string]$RawGo,
    [Parameter(Mandatory = $true)]
    [string]$Output,
    [int]$WarmRuns = 10
)

$ErrorActionPreference = "Stop"
if ($WarmRuns -lt 5) { throw "WarmRuns must be at least 5." }
foreach ($pathItem in @($Wad, $RawGo)) {
    if (-not (Test-Path -LiteralPath $pathItem -PathType Leaf)) {
        throw "Executable was not found: $pathItem"
    }
}

function Invoke-Measured([string]$File, [string[]]$Arguments) {
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    & $File @Arguments *> $null
    $exitCode = $LASTEXITCODE
    $watch.Stop()
    if ($exitCode -ne 0) { throw "Invocation failed with ${exitCode}: $File $Arguments" }
    [math]::Round($watch.Elapsed.TotalMilliseconds, 3)
}

$outputPath = [System.IO.Path]::GetFullPath($Output)
$outputDirectory = Split-Path -Parent $outputPath
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("rtk-wad-provider-cache-" + [guid]::NewGuid())
$temporaryBase = [System.IO.Path]::GetTempPath()
$previousState = $env:RTK_WAD_STATE_DIR

try {
    New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
    $env:RTK_WAD_STATE_DIR = $temporaryRoot
    $cold = Invoke-Measured $Wad @('go', 'version')
    $warm = 1..$WarmRuns | ForEach-Object { Invoke-Measured $Wad @('go', 'version') }
    $raw = 1..$WarmRuns | ForEach-Object { Invoke-Measured $RawGo @('version') }
    $sortedWarm = $warm | Sort-Object
    $sortedRaw = $raw | Sort-Object
    $result = [ordered]@{
        schema_version = 2
        protocol = "provider-cache-profile-v2"
        adaptive_policy_eligible = $false
        command = @('go', 'version')
        warm_runs = $WarmRuns
        cold_wad_auto_ms = $cold
        warm_wad_auto_median_ms = [math]::Round($sortedWarm[[int]($sortedWarm.Count / 2)], 3)
        raw_median_ms = [math]::Round($sortedRaw[[int]($sortedRaw.Count / 2)], 3)
        warm_wad_auto_samples_ms = $warm
        raw_samples_ms = $raw
    }
    $result | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $outputPath -Encoding utf8
    Write-Output "Wrote $outputPath"
}
finally {
    $env:RTK_WAD_STATE_DIR = $previousState
    if ((Test-Path -LiteralPath $temporaryRoot) -and $temporaryRoot.StartsWith($temporaryBase, [System.StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
