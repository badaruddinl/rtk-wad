[CmdletBinding()]
param(
    [string]$Destination = (Join-Path ([Environment]::GetFolderPath("LocalApplicationData")) "Programs\XUVA"),
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
    [switch]$Recover,
    [switch]$Rollback,
    [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$lifecycleLibrary = Join-Path $PSScriptRoot "install-lifecycle.ps1"
if (-not (Test-Path -LiteralPath $lifecycleLibrary -PathType Leaf)) {
    throw "XUVA installation lifecycle library is missing."
}
. $lifecycleLibrary

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
$journalPath = Get-XuvaTransactionPath -TargetDirectory $targetDirectory
$legacyDirectory = Get-XuvaNormalizedFullPath -Value (Join-Path $env:USERPROFILE ".local\bin")
$legacyTarget = Join-Path $legacyDirectory "xuva.exe"
$usingDefaultDestination = $targetDirectory -eq
    (Get-XuvaNormalizedFullPath -Value (Get-XuvaDefaultDestination))

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
    $owned = $false
    $installationId = $null
    if (Test-Path -LiteralPath $targetDirectory -PathType Container) {
        try {
            $marker = Get-XuvaOwnedBundle -Directory $targetDirectory
            $owned = $true
            $installationId = [string]$marker.installation_id
        } catch {
            $owned = $false
        }
    }
    [pscustomobject]@{
        Installed = Test-Path -LiteralPath $target -PathType Leaf
        Owned = $owned
        InstallationId = $installationId
        Target = $target
        BackupAvailable = (Test-Path -LiteralPath (Join-Path $previousDirectory "xuva.exe") -PathType Leaf) -and
            (Test-Path -LiteralPath (Join-Path $previousDirectory $script:XuvaOwnershipMarkerName) -PathType Leaf)
        BackupDirectory = $previousDirectory
        OnUserPath = Test-DirectoryOnUserPath -Directory $targetDirectory
        RecoveryRequired = Test-Path -LiteralPath $journalPath -PathType Leaf
        RecoveryJournal = $journalPath
        LegacyInstallationDetected = $usingDefaultDestination -and
            (Test-Path -LiteralPath $legacyTarget -PathType Leaf)
        LegacyInstallation = $legacyTarget
    } | ConvertTo-Json
    return
}

function Invoke-TestCrash([string]$Point) {
    if ($env:XUVA_TEST_MODE -eq "1" -and $env:XUVA_TEST_INSTALL_CRASH -eq $Point) {
        Stop-Process -Id $PID -Force
    }
}

if ($Recover) {
    if (Invoke-XuvaTransactionRecovery -TargetDirectory $targetDirectory `
        -PreviousDirectory $previousDirectory) {
        Write-Output "Recovered the interrupted XUVA installation transaction."
    } else {
        Write-Output "No interrupted XUVA installation transaction was found."
    }
    return
}

if (Test-Path -LiteralPath $journalPath -PathType Leaf) {
    throw "An interrupted XUVA transaction requires recovery. Run this installer with -Recover first."
}

if ($Rollback) {
    if (-not (Test-Path -LiteralPath $previousDirectory -PathType Container)) {
        throw "No previous XUVA bundle backup found beside $targetDirectory."
    }
    if (-not (Test-Path -LiteralPath $targetDirectory -PathType Container)) {
        throw "No current XUVA bundle found in $targetDirectory."
    }
    $currentMarker = Get-XuvaOwnedBundle -Directory $targetDirectory
    $previousMarker = Get-XuvaOwnedBundle -Directory $previousDirectory
    if ([string]$currentMarker.installation_id -ne [string]$previousMarker.installation_id) {
        throw "Current and previous bundles do not belong to the same XUVA installation."
    }
    $swapDirectory = Join-Path $parentDirectory ".$bundleName.rollback-$nonce"
    $rollbackState = @{
        operation = "rollback"
        phase = "prepared"
        target = $targetDirectory
        previous = $previousDirectory
        stage = ""
        auxiliary = $swapDirectory
        had_existing = $true
        installation_id = [string]$currentMarker.installation_id
    }
    Write-XuvaTransaction -JournalPath $journalPath -State $rollbackState
    Move-Bundle -From $targetDirectory -To $swapDirectory
    $rollbackState.phase = "current_rotated"
    Write-XuvaTransaction -JournalPath $journalPath -State $rollbackState
    $previousActivated = $false
    try {
        Invoke-TestFailure -Point "rollback-after-current-move"
        Move-Bundle -From $previousDirectory -To $targetDirectory
        $previousActivated = $true
        $rollbackState.phase = "previous_activated"
        Write-XuvaTransaction -JournalPath $journalPath -State $rollbackState
        Invoke-TestFailure -Point "rollback-after-previous-activate"
        Move-Bundle -From $swapDirectory -To $previousDirectory
        $rollbackState.phase = "committed"
        Write-XuvaTransaction -JournalPath $journalPath -State $rollbackState
        Remove-Item -LiteralPath $journalPath -Force
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
        Remove-Item -LiteralPath $journalPath -Force -ErrorAction SilentlyContinue
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
$existingMarker = $null
if (Test-Path -LiteralPath $targetDirectory) {
    $existingMarker = Get-XuvaOwnedBundle -Directory $targetDirectory
}
$hadExisting = $null -ne $existingMarker
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
            "install-lifecycle.ps1",
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
            "install-lifecycle.ps1",
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
    foreach ($requiredCompanion in @("install.ps1", "install-lifecycle.ps1", "uninstall.ps1", "xuva-wsl.sh")) {
        if (-not (Test-Path -LiteralPath (Join-Path $stageDirectory $requiredCompanion) -PathType Leaf)) {
            throw "Candidate bundle is missing required companion $requiredCompanion."
        }
    }
    $versionOutput = @(& $stagedTarget --version)
    if (($LASTEXITCODE -ne 0) -or
        (($versionOutput -join "`n") -notmatch '^xuva \d+\.\d+\.\d+')) {
        throw "Candidate launcher failed its local version smoke check."
    }
    $installationId = if ($existingMarker) {
        [string]$existingMarker.installation_id
    } else {
        [guid]::NewGuid().ToString()
    }
    New-XuvaOwnershipMarker -Directory $stageDirectory -InstallationId $installationId

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

    $installState = @{
        operation = "install"
        phase = "prepared"
        target = $targetDirectory
        previous = $previousDirectory
        stage = $stageDirectory
        auxiliary = $retiredPreviousDirectory
        had_existing = $hadExisting
        installation_id = $installationId
    }
    Write-XuvaTransaction -JournalPath $journalPath -State $installState
    if (Test-Path -LiteralPath $previousDirectory) {
        $previousMarker = Get-XuvaOwnedBundle -Directory $previousDirectory
        if ([string]$previousMarker.installation_id -ne $installationId) {
            throw "Previous bundle does not belong to this XUVA installation."
        }
        Move-Bundle -From $previousDirectory -To $retiredPreviousDirectory
        $retiredPrevious = $true
        $installState.phase = "previous_retired"
        Write-XuvaTransaction -JournalPath $journalPath -State $installState
    }
    if ($hadExisting) {
        Move-Bundle -From $targetDirectory -To $previousDirectory
        $rotatedCurrent = $true
        $installState.phase = "current_rotated"
        Write-XuvaTransaction -JournalPath $journalPath -State $installState
        Invoke-TestCrash -Point "after-current-move"
    }
    Move-Bundle -From $stageDirectory -To $targetDirectory
    $activated = $true
    $installState.phase = "candidate_activated"
    Write-XuvaTransaction -JournalPath $journalPath -State $installState
    Invoke-TestCrash -Point "after-candidate-activate"

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
            ((@($targetDirectory) + $entries) -join ";"),
            "User"
        )
        $pathChanged = $true
        Write-Output "Added $targetDirectory to the user PATH. Open a new terminal to use it."
    }
    if ($retiredPrevious -and (Test-Path -LiteralPath $retiredPreviousDirectory)) {
        Remove-XuvaOwnedDirectory -Directory $retiredPreviousDirectory
        $retiredPrevious = $false
    }
    $installState.phase = "committed"
    Write-XuvaTransaction -JournalPath $journalPath -State $installState
    Remove-Item -LiteralPath $journalPath -Force
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
        Remove-XuvaEphemeralDirectory -Directory $failedDirectory `
            -ParentDirectory $parentDirectory -BundleName $bundleName
    }
    Remove-Item -LiteralPath $journalPath -Force -ErrorAction SilentlyContinue
    throw
} finally {
    foreach ($temporaryDirectory in @($stageDirectory, $failedDirectory, $retiredPreviousDirectory)) {
        if (Test-Path -LiteralPath $temporaryDirectory) {
            Remove-XuvaEphemeralDirectory -Directory $temporaryDirectory `
                -ParentDirectory $parentDirectory -BundleName $bundleName
        }
    }
}

Write-Output "Installed complete XUVA bundle at $targetDirectory"
if ($usingDefaultDestination -and (Test-Path -LiteralPath $legacyTarget -PathType Leaf)) {
    Write-Warning "A legacy XUVA executable remains at $legacyTarget. It was not moved or deleted because .local\bin is a shared directory. The dedicated XUVA directory was placed first only when -AddToPath was requested."
}
