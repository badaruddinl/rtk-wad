[CmdletBinding()]
param(
    [string]$Source = (Join-Path (Split-Path -Parent $PSScriptRoot) "target\release\xuva.exe"),
    [string]$NativeRtk
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
    throw "Setup readiness source was not found: $Source"
}

$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("xuva-setup-readiness-" + [guid]::NewGuid())
$temporaryBase = [System.IO.Path]::GetTempPath()
$previousState = $env:XUVA_STATE_DIR
$previousPath = $env:Path
$previousNativeRtk = $env:XUVA_NATIVE_RTK_PATH

try {
    New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
    $binary = Join-Path $temporaryRoot "xuva.exe"
    Copy-Item -LiteralPath $Source -Destination $binary
    $env:XUVA_STATE_DIR = Join-Path $temporaryRoot "state"

    & $binary setup go --status
    if ($LASTEXITCODE -ne 0) { throw "Initial setup status failed: $LASTEXITCODE" }
    if (Test-Path -LiteralPath (Join-Path $env:XUVA_STATE_DIR "setup-transaction-v1.json")) {
        throw "Status must not create a setup transaction."
    }

    $ready = & $binary setup go --json --refresh | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "Setup planning failed: $LASTEXITCODE" }
    if ($ready.status -eq "ready") {
        & $binary setup go --apply --confirm
        if ($LASTEXITCODE -ne 0) { throw "Confirmed no-op setup failed: $LASTEXITCODE" }
        if (Test-Path -LiteralPath (Join-Path $env:XUVA_STATE_DIR "setup-transaction-v1.json")) {
            throw "A ready provider must not create a setup transaction."
        }
    }

    if ($NativeRtk) { $env:XUVA_NATIVE_RTK_PATH = $NativeRtk }
    $env:Path = (($previousPath -split ';' | Where-Object { $_ -notmatch '\\Go\\bin' }) -join ';')
    & $binary setup go --apply
    if ($LASTEXITCODE -ne 2) { throw "Unconfirmed setup apply must stop with exit 2, got $LASTEXITCODE" }
    if (Test-Path -LiteralPath (Join-Path $env:XUVA_STATE_DIR "setup-transaction-v1.json")) {
        throw "An unconfirmed setup apply must not create a setup transaction."
    }

    & $binary setup go --recover
    if ($LASTEXITCODE -ne 0) { throw "Recovery without a transaction failed: $LASTEXITCODE" }
    if (Test-Path -LiteralPath (Join-Path $env:XUVA_STATE_DIR "setup-transaction-v1.json")) {
        throw "Recovery without a transaction must not create one."
    }

    Write-Output "Setup readiness contract passed"
}
finally {
    $env:XUVA_STATE_DIR = $previousState
    $env:Path = $previousPath
    $env:XUVA_NATIVE_RTK_PATH = $previousNativeRtk
    if ((Test-Path -LiteralPath $temporaryRoot) -and $temporaryRoot.StartsWith($temporaryBase, [System.StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
