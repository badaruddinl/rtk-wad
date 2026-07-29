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
    Copy-Item -LiteralPath (Join-Path $rootPath "scripts\uninstall.ps1") -Destination $staging
    Copy-Item -LiteralPath (Join-Path $rootPath "scripts\xuva-wsl.sh") -Destination $staging
    Copy-Item -LiteralPath (Join-Path $rootPath "LICENSE") -Destination $staging
    Copy-Item -LiteralPath (Join-Path $rootPath "SECURITY.md") -Destination $staging
    Copy-Item -LiteralPath (Join-Path $rootPath "README.md") `
        -Destination (Join-Path $staging "README.txt")
    $payloadHashes = Get-ChildItem -LiteralPath $staging -File |
        Sort-Object Name |
        ForEach-Object {
            $digest = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            "$digest  $($_.Name)"
        }
    Set-Content -LiteralPath (Join-Path $staging "SHA256SUMS") `
        -Value $payloadHashes -Encoding ascii
    Compress-Archive -Path (Join-Path $staging "*") -DestinationPath $archive -Force
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -LiteralPath "$archive.sha256" -Value "$hash  $archiveName" -Encoding ascii
    Write-Output $archive
} finally {
    if (Test-Path -LiteralPath $staging) {
        Remove-Item -LiteralPath $staging -Recurse -Force
    }
}
