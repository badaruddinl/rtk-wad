$ErrorActionPreference = "Stop"

$repository = Split-Path -Parent $PSScriptRoot
$statefulRunners = @(
    "run-core-three-way.mjs",
    "run-toolchain-three-way.mjs",
    "run-npm-run-list-three-way.mjs",
    "run-wsl-bridge-core.mjs"
)

foreach ($runner in $statefulRunners) {
    $path = Join-Path $repository "benchmarks\$runner"
    $contents = Get-Content -LiteralPath $path -Raw
    if ($contents -notmatch 'isolatedBenchmarkState\(settings\.output\)') {
        throw "$runner must derive state through the ACL-protected benchmark-state helper."
    }
}

$benchmarkSources = Get-ChildItem -LiteralPath (Join-Path $repository "benchmarks") -File |
    Where-Object { $_.Extension -in @(".mjs", ".ps1") }
foreach ($source in $benchmarkSources) {
    $contents = Get-Content -LiteralPath $source.FullName -Raw
    if ($contents -match 'RTK_WAD_STATE_DIR|RTK_WAD_NATIVE_RTK_PATH|RTK_WSL_(?:BACKEND|DISTRO|RTK_PATH|EXTRA_PATH)') {
        throw "$($source.Name) still uses a retired runtime environment variable."
    }
}

$probe = @'
import { isolatedBenchmarkState } from "./benchmarks/isolated-state.mjs";
const root = "C:\\Users\\benchmark\\AppData\\Local";
const first = isolatedBenchmarkState("E:\\results\\core.json", root);
const second = isolatedBenchmarkState("E:\\results\\core.json", root);
if (first !== second || !first.startsWith(`${root}\\xuva\\benchmark-state\\`)) process.exit(1);
'@
$probe | & node --input-type=module -
if ($LASTEXITCODE -ne 0) {
    throw "The isolated benchmark-state helper is not deterministic under LOCALAPPDATA."
}

Write-Output "Benchmark runner state contract passed."
