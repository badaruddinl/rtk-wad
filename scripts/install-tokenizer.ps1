[CmdletBinding()]
param(
    [string]$Root,
    [string]$Python,
    [string]$Requirements = (Join-Path $PSScriptRoot "..\requirements\xuva-tokenizer.txt"),
    [switch]$PlanPythonBootstrap,
    [switch]$InstallPython,
    [switch]$ConfirmPythonInstall,
    [switch]$ForcePythonBootstrap,
    [string]$Winget = "winget.exe"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-PythonRuntime {
    param([string]$ExplicitPython)

    if ($ExplicitPython) {
        $resolved = (Resolve-Path -LiteralPath $ExplicitPython -ErrorAction Stop).Path
        return (Assert-PythonRuntime -PythonPath $resolved)
    }
    $launcher = Get-Command py.exe -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($launcher) {
        $resolved = (& $launcher.Source -3.12 -c "import sys; print(sys.executable)").Trim()
        if ($LASTEXITCODE -eq 0 -and $resolved) {
            return (Assert-PythonRuntime -PythonPath $resolved)
        }
    }
    $python = Get-Command python.exe -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($python) {
        return (Assert-PythonRuntime -PythonPath $python.Source)
    }
    foreach ($candidate in @(
        (Join-Path $env:LOCALAPPDATA "Programs\Python\Python312\python.exe"),
        (Join-Path ${env:ProgramFiles} "Python312\python.exe")
    )) {
        if (Test-Path -LiteralPath $candidate) {
            return (Assert-PythonRuntime -PythonPath $candidate)
        }
    }
    return $null
}

function Assert-PythonRuntime {
    param([Parameter(Mandatory)] [string]$PythonPath)

    $version = (& $PythonPath -c "import sys; print('.'.join(map(str, sys.version_info[:2])))").Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "The selected Python runtime could not be queried: $PythonPath"
    }
    $parts = $version.Split('.')
    if ($parts.Count -ne 2 -or [int]$parts[0] -ne 3 -or [int]$parts[1] -lt 9) {
        throw "The selected Python runtime must be Python 3.9 or later: $PythonPath ($version)"
    }
    return [pscustomobject]@{ File = $PythonPath; Version = $version }
}

function Get-PythonBootstrapPlan {
    param([Parameter(Mandatory)] [string]$WingetPath)

    $resolvedWinget = Get-Command $WingetPath -ErrorAction SilentlyContinue | Select-Object -First 1
    [pscustomobject]@{
        status = if ($resolvedWinget) { "planned" } else { "blocked" }
        package = "Python.Python.3.12"
        source = "winget"
        executable = if ($resolvedWinget) { $resolvedWinget.Source } else { $WingetPath }
        arguments = @("install", "--id", "Python.Python.3.12", "--exact", "--source", "winget", "--accept-package-agreements", "--accept-source-agreements")
        reason = if ($resolvedWinget) { "Python is needed only to provision the pinned XUVA tokenizer dependency." } else { "winget is unavailable; XUVA will not select an alternate installer automatically." }
    }
}

function Install-PythonRuntime {
    param(
        [Parameter(Mandatory)] [object]$Plan,
        [switch]$Install,
        [switch]$Confirm
    )

    if (-not $Install) {
        throw "No usable Python 3.9+ runtime was found. Review the plan with -PlanPythonBootstrap, or rerun with -InstallPython -ConfirmPythonInstall to authorize the documented Python.Python.3.12 winget installation."
    }
    if (-not $Confirm) {
        throw "Python bootstrap requires explicit confirmation. Re-run with -InstallPython -ConfirmPythonInstall after reviewing the plan."
    }
    if ($Plan.status -ne "planned") {
        throw "Python bootstrap is blocked: $($Plan.reason)"
    }
    & $Plan.executable @($Plan.arguments)
    if ($LASTEXITCODE -ne 0) {
        throw "Python bootstrap failed with exit code $LASTEXITCODE. The XUVA launcher was not activated."
    }
    $runtime = Resolve-PythonRuntime
    if (-not $runtime) {
        throw "Python installation completed but no usable runtime is visible in this process. Open a new terminal and rerun the XUVA installer; the launcher was not activated."
    }
    return $runtime
}

function Invoke-RuntimePython {
    param(
        [Parameter(Mandatory)] [object]$Runtime,
        [Parameter(Mandatory)] [string[]]$Arguments
    )

    & $Runtime.File @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Python command failed with exit code $LASTEXITCODE."
    }
}

function Get-TokenizerDependency {
    param([Parameter(Mandatory)] [string]$RequirementsPath)

    $dependencyMatches = @(
        Get-Content -LiteralPath $RequirementsPath |
            ForEach-Object {
                if ($_ -match '^\s*tiktoken==(?<version>[0-9]+(?:\.[0-9]+)+)\s*(?:#.*)?$') {
                    [pscustomobject]@{
                        Requirement = "tiktoken==$($Matches.version)"
                        Version = $Matches.version
                    }
                }
            }
    )
    if ($dependencyMatches.Count -ne 1) {
        throw "The official XUVA tokenizer manifest must declare exactly one tiktoken==<version> dependency."
    }
    return $dependencyMatches[0]
}

function Assert-Tokenizer {
    param(
        [Parameter(Mandatory)] [string]$PythonPath,
        [Parameter(Mandatory)] [string]$ExpectedVersion
    )

    $version = & $PythonPath -c "import tiktoken; print(tiktoken.__version__)"
    if ($LASTEXITCODE -ne 0 -or "$version".Trim() -ne $ExpectedVersion) {
        throw "Private tokenizer environment does not provide tiktoken==$ExpectedVersion."
    }
}

$requirementsPath = (Resolve-Path -LiteralPath $Requirements -ErrorAction Stop).Path
$tokenizer = Get-TokenizerDependency -RequirementsPath $requirementsPath
if (-not $Root) {
    $Root = Join-Path $env:LOCALAPPDATA "xuva\tokenizer\tiktoken-$($tokenizer.Version)"
}
$rootPath = [System.IO.Path]::GetFullPath($Root)
if ($PlanPythonBootstrap) {
    Get-PythonBootstrapPlan -WingetPath $Winget | ConvertTo-Json -Compress
    return
}
$venvPython = Join-Path $rootPath "Scripts\python.exe"
if (Test-Path -LiteralPath $venvPython) {
    Assert-Tokenizer -PythonPath $venvPython -ExpectedVersion $tokenizer.Version
    [pscustomobject]@{ tokenizer = $tokenizer.Requirement; root = $rootPath; status = "ready" } | ConvertTo-Json -Compress
    return
}
if (Test-Path -LiteralPath $rootPath) {
    throw "Tokenizer root already exists but is incomplete: $rootPath. Review or remove it deliberately before retrying."
}

$parent = Split-Path -Parent $rootPath
New-Item -ItemType Directory -Path $parent -Force | Out-Null
$staging = "$rootPath.$PID.new"
$runtime = if ($ForcePythonBootstrap) { $null } else { Resolve-PythonRuntime -ExplicitPython $Python }
if (-not $runtime) {
    $plan = Get-PythonBootstrapPlan -WingetPath $Winget
    $runtime = Install-PythonRuntime -Plan $plan -Install:$InstallPython -Confirm:$ConfirmPythonInstall
}
try {
    Invoke-RuntimePython -Runtime $runtime -Arguments @("-m", "venv", $staging)
    $stagingPython = Join-Path $staging "Scripts\python.exe"
    $pipOutput = @(& $stagingPython -m pip install --disable-pip-version-check --no-input --requirement $requirementsPath 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "pip could not install the XUVA tokenizer dependency: $($pipOutput | Select-Object -Last 1)"
    }
    Assert-Tokenizer -PythonPath $stagingPython -ExpectedVersion $tokenizer.Version
    Move-Item -LiteralPath $staging -Destination $rootPath -ErrorAction Stop
} finally {
    if (Test-Path -LiteralPath $staging) {
        Remove-Item -LiteralPath $staging -Recurse -Force
    }
}
[pscustomobject]@{ tokenizer = $tokenizer.Requirement; root = $rootPath; status = "installed" } | ConvertTo-Json -Compress
