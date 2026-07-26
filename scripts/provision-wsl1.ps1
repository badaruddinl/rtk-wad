[CmdletBinding()]
param(
    [string]$DistroName = "Ubuntu-RTK-WSL1",
    [string]$InstallLocation = (Join-Path $env:LOCALAPPDATA "rtk-wad\Ubuntu-RTK-WSL1"),
    [string]$ImageDirectory = "E:\luthfi\wsl\images",
    [string]$LinuxUser = "rtk",
    [string]$RtkVersion = "0.43.0",
    [string]$RipgrepVersion = "15.1.0"
)

$ErrorActionPreference = "Stop"
$rootfsName = "ubuntu-jammy-wsl-amd64-ubuntu22.04lts.rootfs.tar.gz"
$rootfsUrl = "https://cloud-images.ubuntu.com/wsl/jammy/current/$rootfsName"
$rootfsChecksumsUrl = "https://cloud-images.ubuntu.com/wsl/jammy/current/SHA256SUMS"
$rtkArchiveName = "rtk-x86_64-unknown-linux-musl.tar.gz"
$rtkArchiveUrl = "https://github.com/rtk-ai/rtk/releases/download/v$RtkVersion/$rtkArchiveName"
$rtkChecksumsUrl = "https://github.com/rtk-ai/rtk/releases/download/v$RtkVersion/checksums.txt"
$rootfs = Join-Path $ImageDirectory $rootfsName
$rootfsChecksums = Join-Path $ImageDirectory "ubuntu-jammy-SHA256SUMS"
$rtkArchive = Join-Path $ImageDirectory "rtk-$RtkVersion-x86_64-unknown-linux-musl.tar.gz"
$rtkChecksums = Join-Path $ImageDirectory "rtk-$RtkVersion-checksums.txt"
$ripgrepDirectory = "ripgrep-$RipgrepVersion-x86_64-unknown-linux-musl"
$ripgrepArchiveName = "$ripgrepDirectory.tar.gz"
$ripgrepArchiveUrl = "https://github.com/BurntSushi/ripgrep/releases/download/$RipgrepVersion/$ripgrepArchiveName"
$ripgrepChecksumUrl = "$ripgrepArchiveUrl.sha256"
$ripgrepArchive = Join-Path $ImageDirectory $ripgrepArchiveName
$ripgrepChecksum = Join-Path $ImageDirectory "$ripgrepArchiveName.sha256"

function Get-WslList {
    return ((& wsl.exe --list --verbose | Out-String) -replace "`0", "")
}

function Get-DistroVersion([string]$Name) {
    $pattern = "^\s*\*?\s*$([regex]::Escape($Name))\s+\S+\s+([12])\s*$"
    foreach ($line in (Get-WslList) -split "`r?`n") {
        if ($line -match $pattern) {
            return [int]$Matches[1]
        }
    }
    return $null
}

function Download-File([string]$Url, [string]$Destination) {
    if (Test-Path -LiteralPath $Destination) {
        return
    }
    & curl.exe --fail --location --output $Destination $Url
    if ($LASTEXITCODE -ne 0) {
        throw "Download failed: $Url"
    }
}

function Assert-Checksum(
    [string]$File,
    [string]$ChecksumsFile,
    [string]$PublishedName
) {
    $entry = Select-String -LiteralPath $ChecksumsFile -Pattern ([regex]::Escape($PublishedName)) |
        Select-Object -First 1
    if (-not $entry) {
        throw "No checksum entry found for $PublishedName."
    }
    $expected = ($entry.Line -split "\s+")[0].ToUpperInvariant()
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $File).Hash
    if ($actual -ne $expected) {
        throw "Checksum mismatch for $File. Expected $expected, got $actual."
    }
}

function Convert-ToWslPath([string]$WindowsPath) {
    $fullPath = [System.IO.Path]::GetFullPath($WindowsPath)
    if ($fullPath -notmatch "^([A-Za-z]):\\(.*)$") {
        throw "Only Windows drive paths can be mapped automatically: $fullPath"
    }
    return "/mnt/$($Matches[1].ToLowerInvariant())/$($Matches[2] -replace '\\', '/')"
}

New-Item -ItemType Directory -Path $ImageDirectory -Force | Out-Null
Download-File $rootfsUrl $rootfs
& curl.exe --fail --location --output $rootfsChecksums $rootfsChecksumsUrl
if ($LASTEXITCODE -ne 0) {
    throw "Unable to download Ubuntu checksums."
}
Assert-Checksum $rootfs $rootfsChecksums $rootfsName

$version = Get-DistroVersion $DistroName
if ($null -eq $version) {
    $installPath = [System.IO.Path]::GetFullPath($InstallLocation)
    $installRoot = [System.IO.Path]::GetPathRoot($installPath)
    $installDrive = $installRoot.TrimEnd("\").TrimEnd(":")
    $volume = Get-Volume -DriveLetter $installDrive -ErrorAction Stop
    if ($volume.FileSystem -ne "NTFS") {
        throw "WSL1 requires an NTFS install location. $installPath is on $($volume.FileSystem)."
    }
    if (Test-Path -LiteralPath $installPath) {
        $entries = @(Get-ChildItem -LiteralPath $installPath -Force)
        if ($entries.Count -ne 0) {
            throw "Install location is not empty: $installPath"
        }
    } else {
        New-Item -ItemType Directory -Path $installPath -Force | Out-Null
    }

    & wsl.exe --import $DistroName $installPath $rootfs --version 1
    if ($LASTEXITCODE -ne 0) {
        throw "WSL 1 import failed. Run enable-wsl1.ps1 from an elevated PowerShell, restart Windows if requested, and retry."
    }
    $version = Get-DistroVersion $DistroName
}

if ($version -ne 1) {
    throw "Refusing to provision $DistroName because it is WSL $version instead of WSL 1."
}

Download-File $rtkArchiveUrl $rtkArchive
Download-File $rtkChecksumsUrl $rtkChecksums
Assert-Checksum $rtkArchive $rtkChecksums $rtkArchiveName
$rtkArchiveWsl = Convert-ToWslPath $rtkArchive
Download-File $ripgrepArchiveUrl $ripgrepArchive
Download-File $ripgrepChecksumUrl $ripgrepChecksum
Assert-Checksum $ripgrepArchive $ripgrepChecksum $ripgrepArchiveName
$ripgrepArchiveWsl = Convert-ToWslPath $ripgrepArchive
$ripgrepEntry = "$ripgrepDirectory/rg"

$provisionScript = @'
set -eu
user_name=$1
archive=$2
rg_archive=$3
rg_entry=$4
for command_path in /usr/bin/flock /usr/bin/setsid /usr/bin/tar /usr/bin/env /bin/sh; do
    if [ ! -x "$command_path" ]; then
        printf 'Missing required executable: %s\n' "$command_path" >&2
        exit 1
    fi
done
if ! id "$user_name" >/dev/null 2>&1; then
    /usr/sbin/useradd --create-home --shell /bin/bash "$user_name"
fi
home_dir=$(/usr/bin/getent passwd "$user_name" | /usr/bin/cut -d: -f6)
/usr/bin/install -d -o "$user_name" -g "$user_name" -m 0755 "$home_dir/.local/bin"
/usr/bin/tar -xOzf "$archive" rtk > "$home_dir/.local/bin/rtk"
/usr/bin/tar -xOzf "$rg_archive" "$rg_entry" > "$home_dir/.local/bin/rg"
/bin/chown "$user_name:$user_name" "$home_dir/.local/bin/rtk"
/bin/chown "$user_name:$user_name" "$home_dir/.local/bin/rg"
/bin/chmod 0755 "$home_dir/.local/bin/rtk" "$home_dir/.local/bin/rg"
printf '[automount]\nenabled=true\nroot=/mnt/\noptions=metadata,umask=22,fmask=11\n\n[user]\ndefault=%s\n' "$user_name" > /etc/wsl.conf
'@

& wsl.exe -d $DistroName -u root --exec /bin/sh -c $provisionScript "rtk-wad-wsl1-provision" $LinuxUser $rtkArchiveWsl $ripgrepArchiveWsl $ripgrepEntry
if ($LASTEXITCODE -ne 0) {
    throw "WSL 1 distro provisioning failed with exit code $LASTEXITCODE."
}

& wsl.exe --terminate $DistroName
if ($LASTEXITCODE -ne 0) {
    throw "Unable to restart the newly provisioned distro."
}

$actualUser = (& wsl.exe -d $DistroName --exec /usr/bin/id -un | Out-String).Trim()
$userExitCode = $LASTEXITCODE
if ($userExitCode -ne 0) {
    throw "The provisioned default user did not pass its smoke test."
}
$rtkVersionOutput = (& wsl.exe -d $DistroName --exec "/home/$LinuxUser/.local/bin/rtk" --version | Out-String).Trim()
$rtkExitCode = $LASTEXITCODE
$ripgrepVersionLines = & wsl.exe -d $DistroName --exec "/home/$LinuxUser/.local/bin/rg" --version
$ripgrepExitCode = $LASTEXITCODE
$ripgrepVersionOutput = ($ripgrepVersionLines | Select-Object -First 1 | Out-String).Trim()
if (
    $rtkExitCode -ne 0 -or
    $ripgrepExitCode -ne 0 -or
    $actualUser -ne $LinuxUser -or
    $rtkVersionOutput -notmatch "^rtk\s+$([regex]::Escape($RtkVersion))$" -or
    $ripgrepVersionOutput -notmatch "^ripgrep\s+$([regex]::Escape($RipgrepVersion))(\s|$)"
) {
    throw "The provisioned RTK runtime did not pass its smoke test."
}

Write-Output "Provisioned $DistroName as WSL 1."
Write-Output "user=$actualUser"
Write-Output "home=/home/$LinuxUser"
Write-Output $rtkVersionOutput
Write-Output $ripgrepVersionOutput
Write-Output "Use the provisioned route with: rtk-wad --route wsl1 <command>"
