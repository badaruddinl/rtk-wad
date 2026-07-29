[CmdletBinding()]
param(
    [string]$VerifyScript,
    [string]$ManifestPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$VerifyScript = if ($VerifyScript) {
    $VerifyScript
} else {
    Join-Path $PSScriptRoot "..\benchmarks\verify-command-manifest.ps1"
}
$ManifestPath = if ($ManifestPath) {
    $ManifestPath
} else {
    Join-Path $PSScriptRoot "..\benchmarks\command-manifest.json"
}

$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("xuva-manifest-provider-" + [guid]::NewGuid())
$previousManifest = $env:XUVA_MANIFEST_CONTRACT_PATH
$previousUnverified = $env:XUVA_MANIFEST_CONTRACT_UNVERIFIED
try {
    New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
    $fakeXuva = Join-Path $temporaryRoot "xuva-provider-fixture.ps1"
    @'
[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments)]
    [string[]]$Arguments
)

$ErrorActionPreference = "Stop"
$global:LASTEXITCODE = 0
if ($Arguments.Count -ge 3 -and
    $Arguments[0] -eq "doctor" -and $Arguments[1] -eq "rtk" -and $Arguments[2] -eq "--json") {
    $identity = if ($env:XUVA_MANIFEST_CONTRACT_UNVERIFIED -eq "1") {
        $null
    } else {
        [pscustomobject]@{
            modified_unix_seconds = 1
            path = "/verified/rtk"
            size_bytes = 4096
        }
    }
    [pscustomobject]@{
        schema_version = 3
        tool = "rtk"
        recommended = 0
        candidates = @(
            [pscustomobject]@{
                kind = "wsl_rtk"
                distro = "Fixture-WSL2"
                wsl_version = 2
                executable = "/verified/rtk"
                usable = $true
            }
        )
        availability = [pscustomobject]@{
            windows = [pscustomobject]@{
                executable = $null
                executable_identity = $null
                executable_version = $null
            }
            wsl = @(
                [pscustomobject]@{
                    distro = "Fixture-WSL2"
                    wsl_version = 2
                    executable = "/verified/rtk"
                    executable_identity = $identity
                    executable_version = "rtk 0.43.0"
                }
            )
        }
    } | ConvertTo-Json -Depth 8
    return
}

if ($Arguments.Count -ge 7 -and
    $Arguments[0] -eq "provider" -and $Arguments[1] -eq "exec" -and
    $Arguments[2] -eq "rtk" -and $Arguments[3] -eq "--candidate" -and
    $Arguments[4] -eq "0" -and $Arguments[5] -eq "--") {
    if ($Arguments[6] -eq "--version") {
        Write-Output "rtk 0.43.0"
        return
    }
    if ($Arguments[6] -eq "--help") {
        $manifest = Get-Content -LiteralPath $env:XUVA_MANIFEST_CONTRACT_PATH -Raw | ConvertFrom-Json
        $commands = @(
            $manifest.native_structured
            $manifest.raw_native
            $manifest.wsl1_conservative
            @($manifest.wad_internal | Where-Object { $_ -notmatch "^-" -and $_ -ne "stats" })
        ) | Sort-Object -Unique
        foreach ($command in $commands) {
            Write-Output "  $command  verified fixture command"
        }
        return
    }
}

$global:LASTEXITCODE = 64
Write-Error "Unexpected fixture invocation: $($Arguments -join ' ')"
'@ | Set-Content -LiteralPath $fakeXuva -Encoding utf8

    $env:XUVA_MANIFEST_CONTRACT_PATH = (Resolve-Path -LiteralPath $ManifestPath).Path
    $env:XUVA_MANIFEST_CONTRACT_UNVERIFIED = "0"
    $verifiedOutput = @(& $VerifyScript -Xuva $fakeXuva -ManifestPath $ManifestPath)
    if (-not ($verifiedOutput -contains "provider_kind=wsl_rtk")) {
        throw "The manifest verifier did not report its verified WSL fallback route."
    }
    if (-not (@($verifiedOutput | Where-Object { $_ -eq "Command manifest covers 69 RTK command families." }))) {
        throw "The verified WSL fallback did not prove exact 69-command coverage."
    }

    $env:XUVA_MANIFEST_CONTRACT_UNVERIFIED = "1"
    $unverifiedRejected = $false
    try {
        & $VerifyScript -Xuva $fakeXuva -ManifestPath $ManifestPath | Out-Null
    } catch {
        $unverifiedRejected = $_.Exception.Message -match "lacks matching distro, identity, and version evidence"
    }
    if (-not $unverifiedRejected) {
        throw "The manifest verifier accepted a WSL provider without identity evidence."
    }

    Write-Output "Command manifest provider fallback contract passed."
} finally {
    $env:XUVA_MANIFEST_CONTRACT_PATH = $previousManifest
    $env:XUVA_MANIFEST_CONTRACT_UNVERIFIED = $previousUnverified
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
