[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$Version,
    [string]$Root = (Join-Path $PSScriptRoot ".."),
    [string]$OutputDirectory = (Join-Path $Root "dist")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$rootPath = (Resolve-Path -LiteralPath $Root).Path
$versionMatch = Select-String -LiteralPath (Join-Path $rootPath "Cargo.toml") `
    -Pattern '^version = "([^"]+)"$'
$cargoVersion = $versionMatch.Matches.Groups[1].Value
if ($Version.TrimStart("v") -ne $cargoVersion) {
    throw "Requested package version $Version does not match Cargo.toml version $cargoVersion."
}
$binary = Join-Path $rootPath "target\release\xuva.exe"
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw "Release binary is missing: $binary"
}

$staging = Join-Path ([System.IO.Path]::GetTempPath()) ("xuva-package-" + [guid]::NewGuid())
$archiveName = "xuva-v$cargoVersion-windows-x86_64.zip"
$archive = Join-Path $OutputDirectory $archiveName
try {
    New-Item -ItemType Directory -Path $staging -Force | Out-Null
    New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
    Copy-Item -LiteralPath $binary -Destination (Join-Path $staging "xuva.exe")
    Copy-Item -LiteralPath (Join-Path $rootPath "scripts\install.ps1") -Destination $staging
    Copy-Item -LiteralPath (Join-Path $rootPath "scripts\install-lifecycle.ps1") -Destination $staging
    Copy-Item -LiteralPath (Join-Path $rootPath "scripts\install-tokenizer.ps1") -Destination $staging
    Copy-Item -LiteralPath (Join-Path $rootPath "scripts\uninstall.ps1") -Destination $staging
    Copy-Item -LiteralPath (Join-Path $rootPath "scripts\verify-package.ps1") -Destination $staging
    Copy-Item -LiteralPath (Join-Path $rootPath "scripts\xuva-wsl.sh") -Destination $staging
    Copy-Item -LiteralPath (Join-Path $rootPath "requirements\xuva-tokenizer.txt") `
        -Destination $staging
    Copy-Item -LiteralPath (Join-Path $rootPath "LICENSE") -Destination $staging
    Copy-Item -LiteralPath (Join-Path $rootPath "SECURITY.md") -Destination $staging
    Copy-Item -LiteralPath (Join-Path $rootPath "README.md") `
        -Destination (Join-Path $staging "README.txt")
    $verbose = @(& $binary --version --verbose)
    if ($LASTEXITCODE -ne 0 -or $verbose[0] -ne "xuva $cargoVersion") {
        throw "Release binary version does not match Cargo.toml."
    }
    $identity = @{}
    foreach ($line in $verbose) {
        if ($line -match '^([^=]+)=(.*)$') {
            $identity[$Matches[1]] = $Matches[2]
        }
    }
    if ($identity["commit"] -notmatch '^[0-9a-f]{40}$' -or
        $identity["target"] -ne "x86_64-pc-windows-msvc" -or
        $identity["profile"] -ne "release") {
        throw "Release binary does not carry a verifiable commit, target, and profile."
    }
    [ordered]@{
        schema_version = 1
        product = "xuva"
        version = $cargoVersion
        commit = $identity["commit"]
        target = $identity["target"]
        profile = $identity["profile"]
        provenance = $identity["provenance"]
    } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $staging "RELEASE-METADATA.json") `
        -Encoding utf8
    $payloadHashes = Get-ChildItem -LiteralPath $staging -File |
        Sort-Object Name |
        ForEach-Object {
            $digest = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            "$digest  $($_.Name)"
        }
    Set-Content -LiteralPath (Join-Path $staging "SHA256SUMS") `
        -Value $payloadHashes -Encoding ascii
    & (Join-Path $staging "verify-package.ps1") -PackageDirectory $staging `
        -ExpectedVersion $cargoVersion -ExpectedCommit $identity["commit"] | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Staged package verification failed."
    }
    Compress-Archive -Path (Join-Path $staging "*") -DestinationPath $archive -Force
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -LiteralPath "$archive.sha256" -Value "$hash  $archiveName" -Encoding ascii
    Write-Output $archive
} finally {
    if (Test-Path -LiteralPath $staging) {
        Remove-Item -LiteralPath $staging -Recurse -Force
    }
}
