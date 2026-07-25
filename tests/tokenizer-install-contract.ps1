[CmdletBinding()]
param(
    [string]$Installer = (Join-Path $PSScriptRoot "..\scripts\install-tokenizer.ps1"),
    [string]$Python
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not $Python) {
    $launcher = Get-Command py.exe -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $launcher) {
        throw "The tokenizer installation contract requires the Python launcher or an explicit -Python path."
    }
    $Python = (& $launcher.Source -3.12 -c "import sys; print(sys.executable)").Trim()
    if ($LASTEXITCODE -ne 0 -or -not $Python) {
        throw "Python 3.12 is required for the tokenizer installation contract."
    }
}

$root = Join-Path ([System.IO.Path]::GetTempPath()) ("rtk-wad-tokenizer-" + [guid]::NewGuid())
try {
    $first = & $Installer -Root $root -Python $Python | ConvertFrom-Json
    if ($first.tokenizer -ne "tiktoken==0.12.0" -or $first.status -ne "installed") {
        throw "Fresh tokenizer installation did not report the pinned dependency."
    }
    $second = & $Installer -Root $root -Python $Python | ConvertFrom-Json
    if ($second.status -ne "ready") { throw "Existing tokenizer environment was not reused safely." }
    $venvPython = Join-Path $root "Scripts\python.exe"
    $version = & $venvPython -c "import tiktoken; print(tiktoken.__version__)"
    if ($LASTEXITCODE -ne 0 -or "$version".Trim() -ne "0.12.0") {
        throw "Installed tokenizer version is not pinned correctly."
    }
    Write-Output "Tokenizer installation contract passed"
}
finally {
    if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
}
