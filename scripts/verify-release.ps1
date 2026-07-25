[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$NativeRtk,
    [Parameter(Mandatory)]
    [string]$Wsl1Distro,
    [Parameter(Mandatory)]
    [string]$Wsl1Rtk,
    [Parameter(Mandatory)]
    [string]$Wsl2Distro,
    [Parameter(Mandatory)]
    [string]$Wsl2Rtk,
    [string]$Cargo = "cargo.exe",
    [string]$Root = (Join-Path $PSScriptRoot "..")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$rootPath = (Resolve-Path -LiteralPath $Root -ErrorAction Stop).Path
$nativeRtkPath = (Resolve-Path -LiteralPath $NativeRtk -ErrorAction Stop).Path
$cargoPath = if (Test-Path -LiteralPath $Cargo -PathType Leaf) {
    (Resolve-Path -LiteralPath $Cargo -ErrorAction Stop).Path
} else {
    $fromPath = Get-Command $Cargo -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($fromPath) {
        $fromPath.Source
    } else {
        $perUserCargo = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
        if (-not (Test-Path -LiteralPath $perUserCargo -PathType Leaf)) {
            throw "Cargo was not found. Pass -Cargo with an executable path or install the Rust toolchain for this user."
        }
        $perUserCargo
    }
}
$source = Join-Path $rootPath "target\release\rtk-wad.exe"
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("rtk-wad-release-gate-" + [guid]::NewGuid())

function Invoke-Checked {
    param(
        [Parameter(Mandatory)] [string]$Name,
        [Parameter(Mandatory)] [scriptblock]$Action
    )

    Write-Output "==> $Name"
    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "$Name failed with exit code $LASTEXITCODE."
    }
}

function Assert-Condition {
    param(
        [Parameter(Mandatory)] [bool]$Condition,
        [Parameter(Mandatory)] [string]$Message
    )

    if (-not $Condition) { throw $Message }
}

try {
    Push-Location $rootPath

    $dirty = @(& git status --porcelain)
    Assert-Condition ($dirty.Count -eq 0) "The release gate requires a clean Git worktree."

    Invoke-Checked -Name "format" -Action { & $cargoPath fmt --all --check }
    Invoke-Checked -Name "clippy" -Action { & $cargoPath clippy --all-targets -- -D warnings }
    Invoke-Checked -Name "unit tests" -Action { & $cargoPath test --bin rtk-wad -- --test-threads=1 }
    Invoke-Checked -Name "release build" -Action { & $cargoPath build --release }
    Invoke-Checked -Name "WSL process contract" -Action { & $cargoPath test --test process_contract -- --test-threads=1 }
    Invoke-Checked -Name "tokenizer bootstrap contract" -Action { & .\tests\tokenizer-bootstrap-contract.ps1 }
    Invoke-Checked -Name "tokenizer installation contract" -Action { & .\tests\tokenizer-install-contract.ps1 }
    Invoke-Checked -Name "package/recovery contract" -Action { & .\tests\packaging-contract.ps1 }
    Invoke-Checked -Name "setup readiness contract" -Action { & .\tests\setup-readiness-contract.ps1 -Source $source }

    New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
    $preflightPath = Join-Path $temporaryRoot "benchmark-preflight.json"
    Invoke-Checked -Name "benchmark provider preflight" -Action {
        & .\scripts\audit-provider-baseline.ps1 `
            -SearchRoots (Split-Path -Parent $nativeRtkPath) `
            -WslRtkOverride @("$Wsl1Distro=$Wsl1Rtk", "$Wsl2Distro=$Wsl2Rtk") `
            -OutputPath $preflightPath
    }
    $preflight = Get-Content -LiteralPath $preflightPath -Raw | ConvertFrom-Json
    Assert-Condition ($preflight.BenchmarkPreflight.Protocol -eq "benchmark-matrix-preflight-v1") "Unexpected P18 benchmark-preflight protocol."
    Assert-Condition ($preflight.BenchmarkPreflight.ManifestCommandCount -eq 69) "The benchmark preflight does not cover all 69 RTK command families."
    Assert-Condition ([bool]$preflight.BenchmarkPreflight.WindowsNativeRtkReady) "The exact native Windows RTK provider is not manifest-compatible."
    Assert-Condition ([bool]$preflight.BenchmarkPreflight.Wsl1RtkReady) "The exact WSL1 RTK provider is not manifest-compatible."
    Assert-Condition ([bool]$preflight.BenchmarkPreflight.Wsl2RtkReady) "The exact WSL2 RTK provider is not manifest-compatible."

    Invoke-Checked -Name "native RTK command manifest" -Action {
        & .\benchmarks\verify-command-manifest.ps1 -NativeRtk $nativeRtkPath
    }
    Invoke-Checked -Name "cargo package" -Action { & $cargoPath package }

    $archive = Get-ChildItem -LiteralPath (Join-Path $rootPath "target\package") -Filter "*.crate" |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    Assert-Condition ($null -ne $archive) "cargo package did not produce a crate archive."
    $forbidden = tar -tf $archive.FullName |
        Select-String -Pattern '(^|/)target/|\.log$|(^|/)(\.env|config\.toml)$|[A-Za-z]:/'
    Assert-Condition ($null -eq $forbidden) "The crate archive contains workstation or build artifacts."

    Write-Output "Release gate passed. No tag, push, GitHub Release, or installer was changed."
}
finally {
    Pop-Location -ErrorAction SilentlyContinue
    if ((Test-Path -LiteralPath $temporaryRoot) -and
        $temporaryRoot.StartsWith([System.IO.Path]::GetTempPath(), [System.StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
