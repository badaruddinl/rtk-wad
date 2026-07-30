[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Join-Path $PSScriptRoot ".."),
    [string]$Cargo = "cargo.exe"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = (Resolve-Path -LiteralPath $RepositoryRoot).Path
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("xuva-build-identity-" + [guid]::NewGuid())
$contractTarget = Join-Path $temporaryRoot "target"
$fakeCommit = "0123456789abcdef0123456789abcdef01234567"
try {
    New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
    Push-Location $root

    $env:XUVA_BUILD_COMMIT_OVERRIDE = "not-a-complete-object-id"
    $ErrorActionPreference = "Continue"
    $invalidOutput = @(& $Cargo check --locked --bin xuva --target-dir $contractTarget 2>&1)
    $invalidExit = $LASTEXITCODE
    $ErrorActionPreference = "Stop"
    if ($invalidExit -eq 0 -or
        -not (($invalidOutput | Out-String).Contains("must be a complete hexadecimal Git object ID"))) {
        throw "Malformed release commit override was not rejected by build.rs."
    }

    $env:XUVA_BUILD_COMMIT_OVERRIDE = $fakeCommit
    & $Cargo build --locked --bin xuva --target-dir $contractTarget
    if ($LASTEXITCODE -ne 0) {
        throw "Valid build identity override failed to compile."
    }
    $binary = Join-Path $contractTarget "debug\xuva.exe"
    $verbose = @(& $binary --version --verbose)
    if ($LASTEXITCODE -ne 0 -or $verbose -notcontains "commit=$fakeCommit") {
        throw "Built binary did not carry the exact reviewed commit override."
    }
    Write-Output "Build identity contract passed."
} finally {
    Remove-Item Env:XUVA_BUILD_COMMIT_OVERRIDE -ErrorAction SilentlyContinue
    Pop-Location -ErrorAction SilentlyContinue
    if ((Test-Path -LiteralPath $temporaryRoot) -and
        $temporaryRoot.StartsWith(
            [System.IO.Path]::GetTempPath(),
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
