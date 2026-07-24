[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$NativeRtk,
    [string]$ManifestPath = (Join-Path $PSScriptRoot "command-manifest.json")
)

$ErrorActionPreference = "Stop"
$rtkManifest = Get-Content -Raw -LiteralPath $ManifestPath | ConvertFrom-Json
$rtkHelp = & $NativeRtk --help
if ($LASTEXITCODE -ne 0) {
    throw "Unable to read RTK help from $NativeRtk."
}

$rtkCommands = $rtkHelp |
    Where-Object { $_ -match '^  ([a-z][a-z0-9-]*)\s{2,}' } |
    ForEach-Object { ([regex]::Match($_, '^  ([a-z][a-z0-9-]*)\s{2,}')).Groups[1].Value } |
    Where-Object { $_ -ne "help" }
$manifestCommands = [System.Collections.Generic.List[string]]::new()
foreach ($command in $rtkManifest.native_structured) { $manifestCommands.Add([string]$command) }
foreach ($command in $rtkManifest.wsl1_conservative) { $manifestCommands.Add([string]$command) }
foreach ($command in $rtkManifest.wad_internal) {
    if ($command -notmatch '^-' -and $command -ne "stats") { $manifestCommands.Add([string]$command) }
}
$manifestCommands = [string[]]$manifestCommands.ToArray()
$differences = Compare-Object -ReferenceObject $rtkCommands -DifferenceObject $manifestCommands
if ($differences) {
    $detail = $differences | ForEach-Object { "$($_.SideIndicator)$($_.InputObject)" }
    throw "Command manifest mismatch: $($detail -join ', ')."
}

Write-Output "Command manifest covers $($rtkCommands.Count) RTK command families."
