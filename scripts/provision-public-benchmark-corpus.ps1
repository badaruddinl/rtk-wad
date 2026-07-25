[CmdletBinding()]
param(
    [ValidateSet("pytest-8.4.0", "typescript-5.9.3", "ripgrep-14.1.1", "all")]
    [string]$Corpus = "all",
    [string]$Destination = (Join-Path $env:LOCALAPPDATA "rtk-wad\benchmark-corpora"),
    [string]$Manifest = (Join-Path $PSScriptRoot "..\benchmarks\public-corpora.json"),
    [string[]]$SparsePath = @()
)

$ErrorActionPreference = "Stop"

function Invoke-GitChecked([string[]]$Arguments) {
    $priorErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        $output = & git @Arguments 2>&1
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $priorErrorActionPreference
    }
    if ($exitCode -ne 0) {
        throw "git $($Arguments -join ' ') failed: $($output | Select-Object -Last 1)"
    }
    return ($output | Out-String).Trim()
}

function Assert-ExistingCorpus([pscustomobject]$Entry, [string]$Path) {
    $head = Invoke-GitChecked @("-C", $Path, "rev-parse", "HEAD")
    if ($head -ne $Entry.commit) {
        throw "Existing corpus '$($Entry.id)' is at $head, not pinned commit $($Entry.commit). Use a new destination; this script never overwrites a corpus."
    }
    $origin = Invoke-GitChecked @("-C", $Path, "remote", "get-url", "origin")
    if ($origin -ne $Entry.repository) {
        throw "Existing corpus '$($Entry.id)' has origin '$origin', not '$($Entry.repository)'."
    }
    foreach ($sparseEntry in $SparsePath) {
        if (-not (Test-Path -LiteralPath (Join-Path $Path $sparseEntry) -PathType Leaf)) {
            throw "Existing corpus '$($Entry.id)' does not contain requested sparse path '$sparseEntry'. Use a new destination; this script never overwrites a corpus."
        }
    }
}

if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    throw "Git is required to provision a public benchmark corpus. Install Git separately and rerun."
}
if (-not (Test-Path -LiteralPath $Manifest -PathType Leaf)) {
    throw "Public corpus manifest was not found: $Manifest"
}

$root = [System.IO.Path]::GetFullPath($Destination)
$entries = (Get-Content -Raw -LiteralPath $Manifest | ConvertFrom-Json).corpora
if ($Corpus -ne "all") { $entries = @($entries | Where-Object { $_.id -eq $Corpus }) }
if ($entries.Count -eq 0) { throw "No public corpus matched '$Corpus'." }
New-Item -ItemType Directory -Path $root -Force | Out-Null

$result = foreach ($entry in $entries) {
    $target = Join-Path $root $entry.id
    if (Test-Path -LiteralPath $target) {
        Assert-ExistingCorpus -Entry $entry -Path $target
        [pscustomobject]@{ id = $entry.id; path = $target; commit = $entry.commit; status = "reused" }
        continue
    }

    $staging = "$target.staging-$PID"
    try {
        $cloneArguments = @("clone", "--depth", "1", "--branch", $entry.tag, "--single-branch")
        if ($SparsePath.Count -gt 0) {
            $cloneArguments += @("--filter=blob:none", "--no-checkout")
        }
        $cloneArguments += @($entry.repository, $staging)
        Invoke-GitChecked $cloneArguments | Out-Null
        if ($SparsePath.Count -gt 0) {
            Invoke-GitChecked @("-C", $staging, "sparse-checkout", "init", "--no-cone") | Out-Null
            Invoke-GitChecked (@("-C", $staging, "sparse-checkout", "set", "--no-cone") + $SparsePath) | Out-Null
            Invoke-GitChecked @("-C", $staging, "checkout", "--detach", $entry.commit) | Out-Null
        }
        Assert-ExistingCorpus -Entry $entry -Path $staging
        Move-Item -LiteralPath $staging -Destination $target
        [pscustomobject]@{ id = $entry.id; path = $target; commit = $entry.commit; status = "cloned"; sparse_paths = @($SparsePath) }
    } finally {
        if (Test-Path -LiteralPath $staging) {
            Remove-Item -LiteralPath $staging -Recurse -Force
        }
    }
}

$result | ConvertTo-Json -Depth 3
