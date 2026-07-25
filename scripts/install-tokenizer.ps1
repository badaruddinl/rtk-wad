[CmdletBinding()]
param(
    [string]$Root = (Join-Path $env:LOCALAPPDATA "rtk-wad\tokenizer\tiktoken-0.12.0"),
    [string]$Python,
    [string]$Requirements = (Join-Path $PSScriptRoot "..\requirements\wad-tokenizer.txt")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-PythonRuntime {
    param([string]$ExplicitPython)

    if ($ExplicitPython) {
        $resolved = (Resolve-Path -LiteralPath $ExplicitPython -ErrorAction Stop).Path
        return [pscustomobject]@{ File = $resolved; Prefix = @() }
    }
    $launcher = Get-Command py.exe -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($launcher) {
        return [pscustomobject]@{ File = $launcher.Source; Prefix = @("-3.12") }
    }
    $python = Get-Command python.exe -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($python) {
        return [pscustomobject]@{ File = $python.Source; Prefix = @() }
    }
    throw "Python 3.12 is required to install the WAD tokenizer dependency. Install Python first, then rerun this installer."
}

function Invoke-RuntimePython {
    param(
        [Parameter(Mandatory)] [object]$Runtime,
        [Parameter(Mandatory)] [string[]]$Arguments
    )

    & $Runtime.File @($Runtime.Prefix + $Arguments)
    if ($LASTEXITCODE -ne 0) {
        throw "Python command failed with exit code $LASTEXITCODE."
    }
}

function Assert-Tokenizer {
    param([Parameter(Mandatory)] [string]$PythonPath)

    $version = & $PythonPath -c "import tiktoken; print(tiktoken.__version__)"
    if ($LASTEXITCODE -ne 0 -or "$version".Trim() -ne "0.12.0") {
        throw "Private tokenizer environment does not provide tiktoken==0.12.0."
    }
}

$requirementsPath = (Resolve-Path -LiteralPath $Requirements -ErrorAction Stop).Path
$rootPath = [System.IO.Path]::GetFullPath($Root)
$venvPython = Join-Path $rootPath "Scripts\python.exe"
if (Test-Path -LiteralPath $venvPython) {
    Assert-Tokenizer -PythonPath $venvPython
    [pscustomobject]@{ tokenizer = "tiktoken==0.12.0"; root = $rootPath; status = "ready" } | ConvertTo-Json -Compress
    return
}
if (Test-Path -LiteralPath $rootPath) {
    throw "Tokenizer root already exists but is incomplete: $rootPath. Review or remove it deliberately before retrying."
}

$parent = Split-Path -Parent $rootPath
New-Item -ItemType Directory -Path $parent -Force | Out-Null
$staging = "$rootPath.$PID.new"
$runtime = Resolve-PythonRuntime -ExplicitPython $Python
try {
    Invoke-RuntimePython -Runtime $runtime -Arguments @("-m", "venv", $staging)
    $stagingPython = Join-Path $staging "Scripts\python.exe"
    $pipOutput = @(& $stagingPython -m pip install --disable-pip-version-check --no-input --requirement $requirementsPath 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "pip could not install the WAD tokenizer dependency: $($pipOutput | Select-Object -Last 1)"
    }
    Assert-Tokenizer -PythonPath $stagingPython
    Move-Item -LiteralPath $staging -Destination $rootPath -ErrorAction Stop
} finally {
    if (Test-Path -LiteralPath $staging) {
        Remove-Item -LiteralPath $staging -Recurse -Force
    }
}
[pscustomobject]@{ tokenizer = "tiktoken==0.12.0"; root = $rootPath; status = "installed" } | ConvertTo-Json -Compress
