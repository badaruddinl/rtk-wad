[CmdletBinding()]
param(
    [string]$NativeRtk,
    [string]$Wsl1Distro,
    [string]$Wsl1Rtk,
    [string]$Wsl2Distro,
    [string]$Wsl2Rtk,
    [string]$Cargo = "cargo.exe",
    [string]$TestBinary,
    [string]$Root,
    [switch]$RequireBenchmarkMatrix,
    [switch]$AllowDirtyVerification
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = if ($Root) { $Root } else { Join-Path $PSScriptRoot ".." }
$rootPath = (Resolve-Path -LiteralPath $Root -ErrorAction Stop).Path
$nativeRtkPath = if ($NativeRtk) {
    (Resolve-Path -LiteralPath $NativeRtk -ErrorAction Stop).Path
} else {
    $null
}
if ([bool]$Wsl1Distro -ne [bool]$Wsl1Rtk) {
    throw "Pass both -Wsl1Distro and -Wsl1Rtk, or neither."
}
if ([bool]$Wsl2Distro -ne [bool]$Wsl2Rtk) {
    throw "Pass both -Wsl2Distro and -Wsl2Rtk, or neither."
}
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
$source = Join-Path $rootPath "target\release\xuva.exe"
$testBinaryPath = if ($TestBinary) {
    (Resolve-Path -LiteralPath $TestBinary -ErrorAction Stop).Path
} else {
    $null
}
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("xuva-release-gate-" + [guid]::NewGuid())

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
    if ($dirty.Count -gt 0) {
        Assert-Condition ([bool]$AllowDirtyVerification) "The release gate requires a clean Git worktree. Use -AllowDirtyVerification only for a non-publishing verification of reviewed local changes."
        $staged = @(& git diff --cached --name-only)
        Assert-Condition ($staged.Count -eq 0) "Dirty verification requires an empty staging area so its source scope is unambiguous."
        Write-Output "verification_mode=reviewed-dirty-worktree"
        Write-Output "dirty_paths=$($dirty.Count)"
    } else {
        Write-Output "verification_mode=clean-release"
    }

    Invoke-Checked -Name "workflow shell-expression boundary" -Action {
        & .\scripts\verify-workflow-shell-boundaries.ps1
    }
    Invoke-Checked -Name "workflow security regression contract" -Action {
        & .\tests\workflow-security-contract.ps1
    }
    Invoke-Checked -Name "build identity regression contract" -Action {
        & .\tests\build-identity-contract.ps1 -Cargo $cargoPath
    }
    Invoke-Checked -Name "format" -Action { & $cargoPath fmt --all --check }
    Invoke-Checked -Name "clippy" -Action { & $cargoPath clippy --locked --all-targets -- -D warnings }
    Invoke-Checked -Name "unit tests" -Action { & $cargoPath test --locked --lib --bins -- --test-threads=1 }
    Invoke-Checked -Name "release build" -Action { & $cargoPath build --locked --release --bins }
    Invoke-Checked -Name "WSL process contract" -Action {
        $previousTestBinary = $env:XUVA_TEST_BINARY
        try {
            if ($testBinaryPath) {
                $env:XUVA_TEST_BINARY = $testBinaryPath
                Write-Output "process_contract_binary=$testBinaryPath"
            } else {
                Remove-Item Env:XUVA_TEST_BINARY -ErrorAction SilentlyContinue
            }
            & $cargoPath test --locked --test process_contract -- --test-threads=1
        } finally {
            if ($null -eq $previousTestBinary) {
                Remove-Item Env:XUVA_TEST_BINARY -ErrorAction SilentlyContinue
            } else {
                $env:XUVA_TEST_BINARY = $previousTestBinary
            }
        }
    }
    Invoke-Checked -Name "tokenizer bootstrap contract" -Action { & .\tests\tokenizer-bootstrap-contract.ps1 }
    Invoke-Checked -Name "tokenizer installation contract" -Action { & .\tests\tokenizer-install-contract.ps1 }
    Invoke-Checked -Name "package/recovery contract" -Action { & .\tests\packaging-contract.ps1 }
    Invoke-Checked -Name "setup readiness contract" -Action { & .\tests\setup-readiness-contract.ps1 -Source $source }
    Invoke-Checked -Name "manifest provider fallback contract" -Action { & .\tests\command-manifest-contract.ps1 }

    New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
    $preflightPath = Join-Path $temporaryRoot "benchmark-preflight.json"
    Invoke-Checked -Name "benchmark provider preflight" -Action {
        $auditArguments = @{
            OutputPath = $preflightPath
        }
        if ($nativeRtkPath) {
            $auditArguments.SearchRoots = @(Split-Path -Parent $nativeRtkPath)
        }
        $wslOverrides = @(
            if ($Wsl1Distro) { "$Wsl1Distro=$Wsl1Rtk" }
            if ($Wsl2Distro) { "$Wsl2Distro=$Wsl2Rtk" }
        )
        if ($wslOverrides.Count -gt 0) {
            $auditArguments.WslRtkOverride = $wslOverrides
        }
        & .\scripts\audit-provider-baseline.ps1 @auditArguments
    }
    $preflight = Get-Content -LiteralPath $preflightPath -Raw | ConvertFrom-Json
    Assert-Condition ($preflight.BenchmarkPreflight.Protocol -eq "benchmark-matrix-preflight-v1") "Unexpected P18 benchmark-preflight protocol."
    Assert-Condition ($preflight.BenchmarkPreflight.ManifestCommandCount -eq 69) "The benchmark preflight does not cover all 69 RTK command families."
    Assert-Condition ([bool]$preflight.BenchmarkPreflight.ManifestProviderReady) "No verified Windows or WSL RTK provider is manifest-compatible."
    $manifestProvider = $preflight.BenchmarkPreflight.ManifestProvider
    Assert-Condition ([bool]$manifestProvider.Path) "The selected manifest provider has no executable path."
    Assert-Condition ([bool]$manifestProvider.Version) "The selected manifest provider has no version evidence."
    Assert-Condition ([bool]$manifestProvider.Sha256) "The selected manifest provider has no SHA-256 identity evidence."
    Write-Output "manifest_provider=$($manifestProvider.Kind):$($manifestProvider.Distro):$($manifestProvider.Path)"
    if ($RequireBenchmarkMatrix) {
        Assert-Condition ([bool]$preflight.BenchmarkPreflight.WindowsNativeRtkReady) "The exact native Windows RTK provider is not manifest-compatible."
        Assert-Condition ([bool]$preflight.BenchmarkPreflight.Wsl1RtkReady) "The exact WSL1 RTK provider is not manifest-compatible."
        Assert-Condition ([bool]$preflight.BenchmarkPreflight.Wsl2RtkReady) "The exact WSL2 RTK provider is not manifest-compatible."
    }

    Invoke-Checked -Name "verified RTK command manifest" -Action {
        if ($nativeRtkPath) {
            & .\benchmarks\verify-command-manifest.ps1 -NativeRtk $nativeRtkPath
        } else {
            & .\benchmarks\verify-command-manifest.ps1 -Xuva $source
        }
    }
    Invoke-Checked -Name "cargo package" -Action {
        if ($AllowDirtyVerification) {
            & $cargoPath package --locked --allow-dirty
        } else {
            & $cargoPath package --locked
        }
    }

    $archive = Get-ChildItem -LiteralPath (Join-Path $rootPath "target\package") -Filter "*.crate" |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    Assert-Condition ($null -ne $archive) "cargo package did not produce a crate archive."
    $forbidden = tar -tf $archive.FullName |
        Select-String -Pattern '(^|/)target/|\.log$|(^|/)(\.env|config\.toml)$|[A-Za-z]:/'
    Assert-Condition ($null -eq $forbidden) "The crate archive contains workstation or build artifacts."

    Write-Output "Release gate passed. No tag, push, GitHub Release, or installed launcher was changed."
}
finally {
    Pop-Location -ErrorAction SilentlyContinue
    if ((Test-Path -LiteralPath $temporaryRoot) -and
        $temporaryRoot.StartsWith([System.IO.Path]::GetTempPath(), [System.StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
