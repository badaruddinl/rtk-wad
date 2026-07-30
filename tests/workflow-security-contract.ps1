[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Join-Path $PSScriptRoot "..")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$verifier = Join-Path $RepositoryRoot "scripts\verify-workflow-shell-boundaries.ps1"
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("xuva-workflow-security-" + [guid]::NewGuid())
try {
    & $verifier | Out-Null

    New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
    $unsafe = @'
name: unsafe
jobs:
  release:
    runs-on: windows-latest
    steps:
      - run: Write-Output "${{ inputs.tag }}"
'@
    Set-Content -LiteralPath (Join-Path $temporaryRoot "unsafe.yml") -Value $unsafe
    $rejected = $false
    try {
        & $verifier -WorkflowDirectory $temporaryRoot | Out-Null
    } catch {
        $rejected = $_.Exception.Message.Contains("Pass it through step env instead")
    }
    if (-not $rejected) {
        throw "Direct workflow-input interpolation into shell source was not rejected."
    }

    $safe = @'
name: safe
jobs:
  release:
    runs-on: windows-latest
    steps:
      - env:
          RELEASE_TAG: ${{ inputs.tag }}
        run: Write-Output $env:RELEASE_TAG
'@
    Set-Content -LiteralPath (Join-Path $temporaryRoot "unsafe.yml") -Value $safe
    & $verifier -WorkflowDirectory $temporaryRoot | Out-Null
    Write-Output "Workflow security contract passed."
} finally {
    if ((Test-Path -LiteralPath $temporaryRoot) -and
        $temporaryRoot.StartsWith(
            [System.IO.Path]::GetTempPath(),
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
