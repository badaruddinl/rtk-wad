Set-StrictMode -Version Latest

$script:XuvaOwnershipMarkerName = ".xuva-installation.json"
$script:XuvaOwnershipSchemaVersion = 1
$script:XuvaTransactionSchemaVersion = 1

function Get-XuvaDefaultDestination {
    $localAppData = [Environment]::GetFolderPath("LocalApplicationData")
    if (-not $localAppData) {
        throw "Windows did not provide a LocalApplicationData directory."
    }
    return Join-Path $localAppData "Programs\XUVA"
}

function Get-XuvaNormalizedFullPath([string]$Value) {
    if (-not $Value) { return $null }
    try {
        return [System.IO.Path]::GetFullPath($Value).TrimEnd("\")
    } catch {
        return $null
    }
}

function Get-XuvaRelativeFiles([string]$Directory) {
    $root = Get-XuvaNormalizedFullPath -Value $Directory
    if (-not $root) {
        throw "Unable to normalize bundle directory $Directory."
    }
    return @(
        Get-ChildItem -LiteralPath $root -File -Recurse -Force |
            ForEach-Object {
                $_.FullName.Substring($root.Length).TrimStart("\").Replace("\", "/")
            } |
            Where-Object { $_ -ne $script:XuvaOwnershipMarkerName } |
            Sort-Object
    )
}

function Test-XuvaSafeRelativePath([string]$Value) {
    if (-not $Value -or [System.IO.Path]::IsPathRooted($Value)) {
        return $false
    }
    $normalized = $Value.Replace("\", "/")
    if ($normalized.StartsWith("/") -or $normalized.EndsWith("/")) {
        return $false
    }
    return -not @($normalized.Split("/") | Where-Object { $_ -in @("", ".", "..") }).Count
}

function New-XuvaOwnershipMarker(
    [string]$Directory,
    [string]$InstallationId = ([guid]::NewGuid().ToString())
) {
    $resolved = Get-XuvaNormalizedFullPath -Value $Directory
    if (-not (Test-Path -LiteralPath (Join-Path $resolved "xuva.exe") -PathType Leaf)) {
        throw "Candidate bundle has no xuva.exe."
    }
    $managed = @(Get-XuvaRelativeFiles -Directory $resolved)
    foreach ($required in @(
        "xuva.exe",
        "install.ps1",
        "install-lifecycle.ps1",
        "uninstall.ps1",
        "xuva-wsl.sh"
    )) {
        if ($required -notin $managed) {
            throw "Candidate bundle is missing required managed file $required."
        }
    }
    [ordered]@{
        schema_version = $script:XuvaOwnershipSchemaVersion
        product = "xuva"
        installation_id = $InstallationId
        managed_files = $managed
    } | ConvertTo-Json -Depth 4 |
        Set-Content -LiteralPath (Join-Path $resolved $script:XuvaOwnershipMarkerName) -Encoding utf8
}

function Get-XuvaOwnedBundle([string]$Directory) {
    $resolved = Get-XuvaNormalizedFullPath -Value $Directory
    if (-not $resolved -or -not (Test-Path -LiteralPath $resolved -PathType Container)) {
        throw "XUVA bundle directory does not exist: $Directory"
    }
    $markerPath = Join-Path $resolved $script:XuvaOwnershipMarkerName
    if (-not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
        throw "Refusing to manage an unowned directory without $($script:XuvaOwnershipMarkerName): $resolved"
    }
    $directories = @(Get-ChildItem -LiteralPath $resolved -Directory -Recurse -Force)
    if ($directories.Count) {
        throw "Refusing to manage a bundle containing directories not covered by its flat ownership contract."
    }
    $reparsePoints = @(
        Get-ChildItem -LiteralPath $resolved -Force |
            Where-Object { $_.Attributes -band [System.IO.FileAttributes]::ReparsePoint }
    )
    if ($reparsePoints.Count) {
        throw "Refusing to manage a bundle containing reparse points."
    }
    try {
        $marker = Get-Content -LiteralPath $markerPath -Raw | ConvertFrom-Json
    } catch {
        throw "XUVA ownership marker is not valid JSON: $markerPath"
    }
    if ($marker.schema_version -ne $script:XuvaOwnershipSchemaVersion -or
        $marker.product -ne "xuva") {
        throw "XUVA ownership marker has an unsupported product or schema: $markerPath"
    }
    $installationId = [guid]::Empty
    if (-not [guid]::TryParse([string]$marker.installation_id, [ref]$installationId)) {
        throw "XUVA ownership marker has an invalid installation_id: $markerPath"
    }
    $managed = @($marker.managed_files | ForEach-Object { [string]$_ } | Sort-Object)
    if (-not $managed.Count -or @($managed | Select-Object -Unique).Count -ne $managed.Count) {
        throw "XUVA ownership marker has an empty or duplicate managed file list."
    }
    foreach ($relative in $managed) {
        if (-not (Test-XuvaSafeRelativePath -Value $relative)) {
            throw "XUVA ownership marker contains unsafe path $relative."
        }
    }
    $actual = @(Get-XuvaRelativeFiles -Directory $resolved)
    if (($actual -join "`n") -ne ($managed -join "`n")) {
        $foreign = @($actual | Where-Object { $_ -notin $managed })
        $missing = @($managed | Where-Object { $_ -notin $actual })
        throw "Refusing to manage bundle with foreign or missing files. Foreign=[$($foreign -join ', ')] missing=[$($missing -join ', ')]."
    }
    foreach ($required in @("xuva.exe", "install.ps1", "install-lifecycle.ps1", "uninstall.ps1")) {
        if ($required -notin $managed) {
            throw "Owned XUVA bundle does not declare required file $required."
        }
    }
    return $marker
}

function Get-XuvaTransactionPath([string]$TargetDirectory) {
    $target = Get-XuvaNormalizedFullPath -Value $TargetDirectory
    $parent = Split-Path -Parent $target
    $name = Split-Path -Leaf $target
    return Join-Path $parent ".$name.transaction.json"
}

function Write-XuvaTransaction(
    [string]$JournalPath,
    [hashtable]$State
) {
    $payload = [ordered]@{
        schema_version = $script:XuvaTransactionSchemaVersion
        product = "xuva"
        operation = $State.operation
        phase = $State.phase
        target = $State.target
        previous = $State.previous
        stage = $State.stage
        auxiliary = $State.auxiliary
        had_existing = [bool]$State.had_existing
        installation_id = $State.installation_id
        updated_at_utc = [datetime]::UtcNow.ToString("o")
    }
    $temporary = "$JournalPath.tmp"
    $payload | ConvertTo-Json -Depth 4 |
        Set-Content -LiteralPath $temporary -Encoding utf8
    Move-Item -LiteralPath $temporary -Destination $JournalPath -Force
}

function Read-XuvaTransaction(
    [string]$JournalPath,
    [string]$ExpectedTarget,
    [string]$ExpectedPrevious
) {
    if (-not (Test-Path -LiteralPath $JournalPath -PathType Leaf)) {
        return $null
    }
    try {
        $state = Get-Content -LiteralPath $JournalPath -Raw | ConvertFrom-Json
    } catch {
        throw "XUVA transaction journal is not valid JSON: $JournalPath"
    }
    if ($state.schema_version -ne $script:XuvaTransactionSchemaVersion -or
        $state.product -ne "xuva" -or
        $state.operation -notin @("install", "rollback", "uninstall")) {
        throw "XUVA transaction journal has an unsupported contract."
    }
    $target = Get-XuvaNormalizedFullPath -Value ([string]$state.target)
    $previous = Get-XuvaNormalizedFullPath -Value ([string]$state.previous)
    if ($target -ne (Get-XuvaNormalizedFullPath -Value $ExpectedTarget) -or
        $previous -ne (Get-XuvaNormalizedFullPath -Value $ExpectedPrevious)) {
        throw "XUVA transaction journal does not belong to the requested destination."
    }
    $parent = Split-Path -Parent $target
    $bundleName = Split-Path -Leaf $target
    foreach ($field in @("stage", "auxiliary")) {
        $value = [string]$state.$field
        if (-not $value) { continue }
        $normalized = Get-XuvaNormalizedFullPath -Value $value
        if ((Split-Path -Parent $normalized) -ne $parent -or
            -not (Split-Path -Leaf $normalized).StartsWith(".$bundleName.")) {
            throw "XUVA transaction journal contains an unsafe $field path."
        }
    }
    return $state
}

function Remove-XuvaOwnedDirectory([string]$Directory) {
    if (-not $Directory -or -not (Test-Path -LiteralPath $Directory)) {
        return
    }
    Get-XuvaOwnedBundle -Directory $Directory | Out-Null
    Remove-Item -LiteralPath $Directory -Recurse -Force -ErrorAction Stop
}

function Remove-XuvaEphemeralDirectory(
    [string]$Directory,
    [string]$ParentDirectory,
    [string]$BundleName
) {
    if (-not $Directory -or -not (Test-Path -LiteralPath $Directory)) {
        return
    }
    $resolved = Get-XuvaNormalizedFullPath -Value $Directory
    $parent = Get-XuvaNormalizedFullPath -Value $ParentDirectory
    if ((Split-Path -Parent $resolved) -ne $parent -or
        -not (Split-Path -Leaf $resolved).StartsWith(".$BundleName.")) {
        throw "Refusing to remove an unsafe XUVA transaction directory: $resolved"
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction Stop
}

function Invoke-XuvaTransactionRecovery(
    [string]$TargetDirectory,
    [string]$PreviousDirectory
) {
    $journalPath = Get-XuvaTransactionPath -TargetDirectory $TargetDirectory
    $state = Read-XuvaTransaction -JournalPath $journalPath `
        -ExpectedTarget $TargetDirectory -ExpectedPrevious $PreviousDirectory
    if (-not $state) {
        return $false
    }
    $target = [string]$state.target
    $previous = [string]$state.previous
    $stage = [string]$state.stage
    $auxiliary = [string]$state.auxiliary

    switch ([string]$state.operation) {
        "install" {
            if ($state.phase -eq "committed") {
                if ($stage) { Remove-XuvaOwnedDirectory -Directory $stage }
                if ($auxiliary) { Remove-XuvaOwnedDirectory -Directory $auxiliary }
            } else {
                if (Test-Path -LiteralPath $target) {
                    Get-XuvaOwnedBundle -Directory $target | Out-Null
                }
                if ($state.phase -eq "candidate_activated") {
                    if ([bool]$state.had_existing) {
                        if (-not (Test-Path -LiteralPath $previous)) {
                            throw "Cannot recover interrupted upgrade because its previous bundle is missing."
                        }
                        $failed = Join-Path (Split-Path -Parent $target) `
                            ".$(Split-Path -Leaf $target).recovery-$([guid]::NewGuid().ToString('N'))"
                        Move-Item -LiteralPath $target -Destination $failed
                        Move-Item -LiteralPath $previous -Destination $target
                        Remove-XuvaOwnedDirectory -Directory $failed
                    } else {
                        Remove-XuvaOwnedDirectory -Directory $target
                    }
                } elseif (-not (Test-Path -LiteralPath $target) -and
                    (Test-Path -LiteralPath $previous)) {
                    Get-XuvaOwnedBundle -Directory $previous | Out-Null
                    Move-Item -LiteralPath $previous -Destination $target
                }
                if ($auxiliary -and (Test-Path -LiteralPath $auxiliary) -and
                    -not (Test-Path -LiteralPath $previous)) {
                    Get-XuvaOwnedBundle -Directory $auxiliary | Out-Null
                    Move-Item -LiteralPath $auxiliary -Destination $previous
                }
                if ($stage) { Remove-XuvaOwnedDirectory -Directory $stage }
            }
        }
        "rollback" {
            if ($state.phase -ne "committed") {
                if ((Test-Path -LiteralPath $target) -and
                    -not (Test-Path -LiteralPath $previous) -and
                    $auxiliary -and (Test-Path -LiteralPath $auxiliary)) {
                    Get-XuvaOwnedBundle -Directory $target | Out-Null
                    Move-Item -LiteralPath $target -Destination $previous
                }
                if (-not (Test-Path -LiteralPath $target) -and
                    $auxiliary -and (Test-Path -LiteralPath $auxiliary)) {
                    Get-XuvaOwnedBundle -Directory $auxiliary | Out-Null
                    Move-Item -LiteralPath $auxiliary -Destination $target
                }
            }
        }
        "uninstall" {
            if ($state.phase -eq "committed") {
                if ($stage) { Remove-XuvaOwnedDirectory -Directory $stage }
                if ($auxiliary) { Remove-XuvaOwnedDirectory -Directory $auxiliary }
            } else {
                if (-not (Test-Path -LiteralPath $target) -and
                    $stage -and (Test-Path -LiteralPath $stage)) {
                    Get-XuvaOwnedBundle -Directory $stage | Out-Null
                    Move-Item -LiteralPath $stage -Destination $target
                }
                if (-not (Test-Path -LiteralPath $previous) -and
                    $auxiliary -and (Test-Path -LiteralPath $auxiliary)) {
                    Get-XuvaOwnedBundle -Directory $auxiliary | Out-Null
                    Move-Item -LiteralPath $auxiliary -Destination $previous
                }
            }
        }
    }
    Remove-Item -LiteralPath $journalPath -Force
    return $true
}
