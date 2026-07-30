[CmdletBinding()]
param(
    [string]$Destination = (Join-Path $env:USERPROFILE ".local\bin"),
    [string]$Source,
    [string]$TokenizerRoot,
    [string]$TokenizerPython,
    [switch]$InstallTokenizer,
    [switch]$InstallPython,
    [switch]$ConfirmPythonInstall,
    [switch]$SkipTokenizer,
    [switch]$SkipProviderScan,
    [switch]$AddToPath,
    [switch]$Status,
    [switch]$Rollback,
    [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$resolvedDestination = [System.IO.Path]::GetFullPath($Destination)
$filesystemRoot = [System.IO.Path]::GetPathRoot($resolvedDestination)
if ($resolvedDestination.TrimEnd("\") -eq $filesystemRoot.TrimEnd("\")) {
    throw "Destination must be a dedicated XUVA bundle directory, not a filesystem root."
}
$targetDirectory = $resolvedDestination.TrimEnd("\")
$parentDirectory = Split-Path -Parent $targetDirectory
$bundleName = Split-Path -Leaf $targetDirectory
$target = Join-Path $targetDirectory "xuva.exe"
$previousDirectory = Join-Path $parentDirectory "$bundleName.previous"
$nonce = "$PID-$([guid]::NewGuid().ToString('N'))"
$stageDirectory = Join-Path $parentDirectory ".$bundleName.stage-$nonce"
$retiredPreviousDirectory = Join-Path $parentDirectory ".$bundleName.previous-$nonce"
$failedDirectory = Join-Path $parentDirectory ".$bundleName.failed-$nonce"

function Get-NormalizedFullPath([string]$Value) {
    if (-not $Value) { return $null }
    try {
        return [System.IO.Path]::GetFullPath($Value).TrimEnd("\")
    } catch {
        return $null
    }
}

function Test-DirectoryOnUserPath([string]$Directory) {
    $normalized = Get-NormalizedFullPath -Value $Directory
    return @(
        [Environment]::GetEnvironmentVariable("Path", "User") -split ";" |
            Where-Object { (Get-NormalizedFullPath -Value $_) -eq $normalized }
    ).Count -gt 0
}

function Invoke-TestFailure([string]$Point) {
    if ($env:XUVA_TEST_MODE -eq "1" -and $env:XUVA_TEST_INSTALL_FAILURE -eq $Point) {
        throw "Injected XUVA installer failure at $Point."
    }
}

function Get-CopyFailurePoint([string]$Name) {
    switch ($Name.ToLowerInvariant()) {
        "xuva.exe" { "copy-binary" }
        "install.ps1" { "copy-install" }
        "uninstall.ps1" { "copy-uninstall" }
        "xuva-wsl.sh" { "copy-shim" }
        default { "copy-$($Name.ToLowerInvariant())" }
    }
}

function Move-Bundle([string]$From, [string]$To) {
    Move-Item -LiteralPath $From -Destination $To -ErrorAction Stop
}

if ($Status) {
    [pscustomobject]@{
        Installed = Test-Path -LiteralPath $target -PathType Leaf
        Target = $target
        BackupAvailable = Test-Path -LiteralPath (Join-Path $previousDirectory "xuva.exe") -PathType Leaf
        BackupDirectory = $previousDirectory
        OnUserPath = Test-DirectoryOnUserPath -Directory $targetDirectory
    } | ConvertTo-Json
    return
}

if ($Rollback) {
    if (-not (Test-Path -LiteralPath (Join-Path $previousDirectory "xuva.exe") -PathType Leaf)) {
        throw "No previous XUVA bundle backup found beside $targetDirectory."
    }
    if (-not (Test-Path -LiteralPath $target -PathType Leaf)) {
        throw "No current XUVA bundle found in $targetDirectory."
    }
    $swapDirectory = Join-Path $parentDirectory ".$bundleName.rollback-$nonce"
    Move-Bundle -From $targetDirectory -To $swapDirectory
    $previousActivated = $false
    try {
        Invoke-TestFailure -Point "rollback-after-current-move"
        Move-Bundle -From $previousDirectory -To $targetDirectory
        $previousActivated = $true
        Invoke-TestFailure -Point "rollback-after-previous-activate"
        Move-Bundle -From $swapDirectory -To $previousDirectory
    } catch {
        if ($previousActivated -and
            (Test-Path -LiteralPath $targetDirectory) -and
            -not (Test-Path -LiteralPath $previousDirectory)) {
            Move-Bundle -From $targetDirectory -To $previousDirectory
            $previousActivated = $false
        }
        if (-not (Test-Path -LiteralPath $targetDirectory) -and
            (Test-Path -LiteralPath $swapDirectory)) {
            Move-Bundle -From $swapDirectory -To $targetDirectory
        }
        throw
    }
    Write-Output "Restored the previous complete XUVA bundle."
    return
}

$bundledSource = Join-Path $PSScriptRoot "xuva.exe"
if (-not $Source) {
    $Source = if (Test-Path -LiteralPath $bundledSource -PathType Leaf) {
        $bundledSource
    } else {
        Join-Path $PSScriptRoot "..\target\release\xuva.exe"
    }
}
$sourcePath = (Resolve-Path -LiteralPath $Source).Path
$isBundledPackage = (Test-Path -LiteralPath $bundledSource -PathType Leaf) -and
    ((Get-NormalizedFullPath $sourcePath) -eq (Get-NormalizedFullPath $bundledSource))
if (Test-Path -LiteralPath (Join-Path $PSScriptRoot "SHA256SUMS")) {
    if (-not $isBundledPackage) {
        throw "A verified release package cannot install a different -Source binary."
    }
    $verifier = Join-Path $PSScriptRoot "verify-package.ps1"
    if (-not (Test-Path -LiteralPath $verifier -PathType Leaf)) {
        throw "Release package verifier is missing."
    }
    & $verifier -PackageDirectory $PSScriptRoot
    if ($LASTEXITCODE -ne 0) {
        throw "Release package integrity verification failed."
    }
}

if ($InstallTokenizer -and $SkipTokenizer) {
    throw "Choose either -InstallTokenizer or the legacy -SkipTokenizer switch, not both."
}
if (-not $InstallTokenizer -and
    ($TokenizerRoot -or $TokenizerPython -or $InstallPython -or $ConfirmPythonInstall)) {
    throw "Tokenizer options require -InstallTokenizer. The core XUVA launcher has no Python or tokenizer dependency."
}
if ($SkipTokenizer) {
    Write-Warning "-SkipTokenizer is no longer needed: tokenizer installation is opt-in."
}

New-Item -ItemType Directory -Path $parentDirectory -Force | Out-Null
if ((Test-Path -LiteralPath $targetDirectory) -and
    -not (Test-Path -LiteralPath $target -PathType Leaf)) {
    throw "Destination exists but is not an installed XUVA bundle: $targetDirectory"
}
$hadExisting = Test-Path -LiteralPath $target -PathType Leaf
if ($hadExisting -and -not $Force) {
    throw "Refusing to overwrite existing $target. Re-run with -Force after reviewing it."
}

$originalUserPath = [Environment]::GetEnvironmentVariable("Path", "User")
$pathChanged = $false
$activated = $false
$retiredPrevious = $false
$rotatedCurrent = $false
try {
    New-Item -ItemType Directory -Path $stageDirectory -ErrorAction Stop | Out-Null

    if ($isBundledPackage) {
        $bundleFiles = @(
            "xuva.exe",
            "install.ps1",
            "install-tokenizer.ps1",
            "uninstall.ps1",
            "verify-package.ps1",
            "xuva-tokenizer.txt",
            "xuva-wsl.sh",
            "LICENSE",
            "SECURITY.md",
            "README.txt",
            "RELEASE-METADATA.json",
            "SHA256SUMS"
        )
        foreach ($name in $bundleFiles) {
            $file = Get-Item -LiteralPath (Join-Path $PSScriptRoot $name) -ErrorAction SilentlyContinue
            if (-not $file) { continue }
            Invoke-TestFailure -Point (Get-CopyFailurePoint $file.Name)
            Copy-Item -LiteralPath $file.FullName -Destination $stageDirectory -ErrorAction Stop
        }
    } else {
        Invoke-TestFailure -Point "copy-binary"
        Copy-Item -LiteralPath $sourcePath -Destination (Join-Path $stageDirectory "xuva.exe") -ErrorAction Stop
        foreach ($companion in @(
            "install.ps1",
            "uninstall.ps1",
            "verify-package.ps1",
            "xuva-wsl.sh",
            "install-tokenizer.ps1"
        )) {
            $companionSource = Join-Path $PSScriptRoot $companion
            if (Test-Path -LiteralPath $companionSource -PathType Leaf) {
                $failureName = Get-CopyFailurePoint $companion
                Invoke-TestFailure -Point $failureName
                Copy-Item -LiteralPath $companionSource -Destination $stageDirectory -ErrorAction Stop
            }
        }
        $requirements = Join-Path $PSScriptRoot "..\requirements\xuva-tokenizer.txt"
        if (Test-Path -LiteralPath $requirements -PathType Leaf) {
            Copy-Item -LiteralPath $requirements `
                -Destination (Join-Path $stageDirectory "xuva-tokenizer.txt") -ErrorAction Stop
        }
    }

    $stagedTarget = Join-Path $stageDirectory "xuva.exe"
    foreach ($requiredCompanion in @("install.ps1", "uninstall.ps1", "xuva-wsl.sh")) {
        if (-not (Test-Path -LiteralPath (Join-Path $stageDirectory $requiredCompanion) -PathType Leaf)) {
            throw "Candidate bundle is missing required companion $requiredCompanion."
        }
    }
    $versionOutput = @(& $stagedTarget --version)
    if (($LASTEXITCODE -ne 0) -or
        (($versionOutput -join "`n") -notmatch '^xuva \d+\.\d+\.\d+')) {
        throw "Candidate launcher failed its local version smoke check."
    }

    if ($InstallTokenizer) {
        $stagedTokenizerInstaller = Join-Path $stageDirectory "install-tokenizer.ps1"
        if (-not (Test-Path -LiteralPath $stagedTokenizerInstaller -PathType Leaf)) {
            throw "Tokenizer installer is not present in the candidate bundle."
        }
        $tokenizerArguments = @{
            Python = $TokenizerPython
            InstallPython = $InstallPython
            ConfirmPythonInstall = $ConfirmPythonInstall
            Requirements = (Join-Path $stageDirectory "xuva-tokenizer.txt")
        }
        if ($TokenizerRoot) { $tokenizerArguments.Root = $TokenizerRoot }
        & $stagedTokenizerInstaller @tokenizerArguments
        if ($LASTEXITCODE -ne 0) {
            throw "Optional XUVA benchmark tokenizer installation failed."
        }
    }

    if (Test-Path -LiteralPath $previousDirectory) {
        Move-Bundle -From $previousDirectory -To $retiredPreviousDirectory
        $retiredPrevious = $true
    }
    if ($hadExisting) {
        Move-Bundle -From $targetDirectory -To $previousDirectory
        $rotatedCurrent = $true
    }
    Move-Bundle -From $stageDirectory -To $targetDirectory
    $activated = $true

    if (-not $SkipProviderScan) {
        Invoke-TestFailure -Point "provider-scan"
        & $target scan
        if ($LASTEXITCODE -ne 0) {
            throw "Installed launcher capability scan failed with exit code $LASTEXITCODE."
        }
    }
    if ($AddToPath -and -not (Test-DirectoryOnUserPath -Directory $targetDirectory)) {
        Invoke-TestFailure -Point "path-update"
        $entries = @($originalUserPath -split ";" | Where-Object { $_ })
        [Environment]::SetEnvironmentVariable(
            "Path",
            ((@($entries) + $targetDirectory) -join ";"),
            "User"
        )
        $pathChanged = $true
        Write-Output "Added $targetDirectory to the user PATH. Open a new terminal to use it."
    }
    if ($retiredPrevious -and (Test-Path -LiteralPath $retiredPreviousDirectory)) {
        Remove-Item -LiteralPath $retiredPreviousDirectory -Recurse -Force
        $retiredPrevious = $false
    }
} catch {
    if ($pathChanged) {
        [Environment]::SetEnvironmentVariable("Path", $originalUserPath, "User")
    }
    if ($activated -and (Test-Path -LiteralPath $targetDirectory)) {
        Move-Bundle -From $targetDirectory -To $failedDirectory
    }
    if ($rotatedCurrent -and (Test-Path -LiteralPath $previousDirectory)) {
        Move-Bundle -From $previousDirectory -To $targetDirectory
        $rotatedCurrent = $false
    }
    if ($retiredPrevious -and (Test-Path -LiteralPath $retiredPreviousDirectory)) {
        Move-Bundle -From $retiredPreviousDirectory -To $previousDirectory
        $retiredPrevious = $false
    }
    if (Test-Path -LiteralPath $failedDirectory) {
        Remove-Item -LiteralPath $failedDirectory -Recurse -Force
    }
    throw
} finally {
    foreach ($temporaryDirectory in @($stageDirectory, $failedDirectory, $retiredPreviousDirectory)) {
        if (Test-Path -LiteralPath $temporaryDirectory) {
            Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force
        }
    }
}

Write-Output "Installed complete XUVA bundle at $targetDirectory"
