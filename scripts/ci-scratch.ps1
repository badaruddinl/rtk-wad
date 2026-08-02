[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('prepare', 'cleanup')]
    [string]$Mode,

    [ValidateRange(1048576, [long]::MaxValue)]
    [long]$MinimumFreeBytes = 1GB
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$namespace = 'xuva-ci-scratch'

function Assert-ScratchPath {
    param([Parameter(Mandatory = $true)][string]$Path)

    $resolved = [System.IO.Path]::GetFullPath($Path)
    $parent = Split-Path -Parent $resolved
    if ((Split-Path -Leaf $parent) -ne $namespace) {
        throw "Refusing CI scratch path outside the $namespace namespace: $resolved"
    }
    if ((Split-Path -Leaf $resolved) -notmatch '^[0-9]+-[A-Za-z0-9_.-]+-[0-9]+$') {
        throw "Refusing malformed CI scratch leaf: $resolved"
    }
    return $resolved
}

if ($Mode -eq 'prepare') {
    if ($env:GITHUB_RUN_ID -notmatch '^[0-9]+$' -or
        $env:GITHUB_RUN_ATTEMPT -notmatch '^[0-9]+$' -or
        $env:GITHUB_JOB -notmatch '^[A-Za-z0-9_.-]+$') {
        throw 'GitHub run identity contains an unsafe scratch-path component.'
    }
    if ([string]::IsNullOrWhiteSpace($env:GITHUB_ENV)) {
        throw 'GITHUB_ENV is required to export the CI scratch environment.'
    }

    $drive = Get-PSDrive -PSProvider FileSystem |
        Where-Object { $_.Root -and $_.Free -ge $MinimumFreeBytes } |
        Sort-Object Free -Descending |
        Select-Object -First 1
    if ($null -eq $drive) {
        throw "No local filesystem has the required $MinimumFreeBytes bytes free for the XUVA process contract."
    }

    $namespaceRoot = Join-Path $drive.Root $namespace
    $scratch = Assert-ScratchPath (Join-Path $namespaceRoot "$($env:GITHUB_RUN_ID)-$($env:GITHUB_JOB)-$($env:GITHUB_RUN_ATTEMPT)")
    $target = Join-Path $scratch 'target'
    $temporary = Join-Path $scratch 'temp'
    [System.IO.Directory]::CreateDirectory($target) | Out-Null
    [System.IO.Directory]::CreateDirectory($temporary) | Out-Null

    $environment = @(
        "XUVA_CI_SCRATCH=$scratch"
        "CARGO_TARGET_DIR=$target"
        "TEMP=$temporary"
        "TMP=$temporary"
    )
    [System.IO.File]::AppendAllText(
        $env:GITHUB_ENV,
        (($environment -join [System.Environment]::NewLine) + [System.Environment]::NewLine),
        [System.Text.UTF8Encoding]::new($false)
    )

    Write-Host "Prepared isolated CI scratch on drive $($drive.Name) with $([math]::Round($drive.Free / 1MB)) MiB free."
    exit 0
}

if ([string]::IsNullOrWhiteSpace($env:XUVA_CI_SCRATCH)) {
    Write-Host 'CI scratch was not prepared; there is nothing to reclaim.'
    exit 0
}

$scratch = Assert-ScratchPath $env:XUVA_CI_SCRATCH
$target = Join-Path $scratch 'target'
if (Test-Path -LiteralPath $target -PathType Container) {
    $targetEntry = Get-ChildItem -LiteralPath $target -Force | Select-Object -First 1
    if ($null -eq $targetEntry) {
        [System.IO.Directory]::Delete($target, $false)
    } else {
        & rustup.exe run 1.97.1 cargo clean --target-dir $target
        if ($LASTEXITCODE -ne 0) {
            Write-Warning "Cargo rejected the verified target directory, so it was retained for diagnosis: $target"
        }
    }
}

foreach ($path in @((Join-Path $scratch 'target'), (Join-Path $scratch 'temp'), $scratch)) {
    if (Test-Path -LiteralPath $path -PathType Container) {
        try {
            [System.IO.Directory]::Delete($path, $false)
        } catch [System.IO.IOException] {
            Write-Warning "Verified CI scratch directory is not empty and was retained for diagnosis: $path"
        }
    }
}
