[CmdletBinding()]
param(
    [string]$Provisioner = (Join-Path $PSScriptRoot "..\scripts\provision-public-benchmark-corpus.ps1")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Invoke-Git([string[]]$Arguments) {
    $output = & git @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) { throw "git $($Arguments -join ' ') failed: $($output | Select-Object -Last 1)" }
    return ($output | Out-String).Trim()
}

$root = Join-Path ([System.IO.Path]::GetTempPath()) ("rtk-wad-public-corpus-" + [guid]::NewGuid())
try {
    $source = Join-Path $root "source"
    $destination = Join-Path $root "destination"
    $manifest = Join-Path $root "manifest.json"
    New-Item -ItemType Directory -Path $source | Out-Null
    Invoke-Git @("init", "-q", "--initial-branch", "main", $source) | Out-Null
    Invoke-Git @("-C", $source, "config", "user.name", "RTK-WAD contract") | Out-Null
    Invoke-Git @("-C", $source, "config", "user.email", "contract@example.invalid") | Out-Null
    Set-Content -LiteralPath (Join-Path $source "package.json") -Value '{"name":"fixture"}' -NoNewline
    Set-Content -LiteralPath (Join-Path $source "README.md") -Value "fixture" -NoNewline
    Invoke-Git @("-C", $source, "add", ".") | Out-Null
    Invoke-Git @("-C", $source, "commit", "-qm", "fixture") | Out-Null
    $commit = Invoke-Git @("-C", $source, "rev-parse", "HEAD")
    [pscustomobject]@{
        schema_version = 1
        corpora = @([pscustomobject]@{
            id = "pytest-8.4.0"
            repository = $source
            tag = "main"
            commit = $commit
            language = "fixture"
            use = "sparse provision contract"
        })
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifest -NoNewline

    $first = & $Provisioner -Corpus pytest-8.4.0 -Destination $destination -Manifest $manifest -SparsePath package.json | ConvertFrom-Json
    if ($first.status -ne "cloned" -or @($first.sparse_paths) -ne "package.json") {
        throw "Sparse corpus clone did not report the requested sparse path."
    }
    $clone = Join-Path $destination "pytest-8.4.0"
    if (-not (Test-Path -LiteralPath (Join-Path $clone "package.json") -PathType Leaf)) {
        throw "Sparse corpus checkout omitted the requested file."
    }
    if (Test-Path -LiteralPath (Join-Path $clone "README.md") -PathType Leaf) {
        throw "Sparse corpus checkout unexpectedly included an unrequested file."
    }
    if ((Invoke-Git @("-C", $clone, "rev-parse", "HEAD")) -ne $commit) {
        throw "Sparse corpus checkout did not retain the pinned commit."
    }
    if ((Invoke-Git @("-C", $clone, "remote", "get-url", "origin")) -ne $source) {
        throw "Sparse corpus checkout did not retain the declared Git origin."
    }

    $reused = & $Provisioner -Corpus pytest-8.4.0 -Destination $destination -Manifest $manifest -SparsePath package.json | ConvertFrom-Json
    if ($reused.status -ne "reused") { throw "Exact sparse corpus was not reused safely." }

    $rejected = $false
    try { & $Provisioner -Corpus pytest-8.4.0 -Destination $destination -Manifest $manifest -SparsePath README.md } catch { $rejected = $true }
    if (-not $rejected) { throw "Existing sparse corpus accepted a missing requested path." }
    Write-Output "Public corpus sparse provision contract passed"
}
finally {
    if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
}
