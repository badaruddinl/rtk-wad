[CmdletBinding()]
param(
    [string[]]$Tools = @(
        "git", "rg", "go", "node", "npm", "npx", "pnpm", "python", "python3",
        "cargo", "dotnet", "dart", "flutter", "java", "mvn", "gradle"
    ),
    [string[]]$SearchRoots = @(
        (Join-Path $env:USERPROFILE ".local\\bin")
    ),
    [switch]$DeepSearch,
    [switch]$ProbeToolMetadata,
    [ValidateRange(1, 300)]
    [int]$MetadataBudgetSeconds = 3,
    [string[]]$WslRtkOverride = @(),
    [string]$ManifestPath = (Join-Path $PSScriptRoot "..\\benchmarks\\command-manifest.json"),
    [string]$OutputPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false

function Get-FirstLine {
    param([object[]]$Lines)
    return (@($Lines | ForEach-Object { "$_" } | Where-Object { $_.Trim() }) | Select-Object -First 1)
}

function Get-WindowsHash {
    param([Parameter(Mandatory)] [string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return $null
    }
    try {
        return (Get-FileHash -LiteralPath $Path -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
    } catch {
        return $null
    }
}

function Invoke-WslCapture {
    param(
        [Parameter(Mandatory)] [string]$Distro,
        [Parameter(Mandatory)] [string[]]$Arguments
    )

    $previousErrorAction = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $lines = @(
            & wsl.exe -d $Distro --exec @Arguments 2>$null |
                ForEach-Object { "$_" -split "`r?`n" } |
                Where-Object { $_ -ne "" }
        )
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorAction
    }
    return [pscustomobject]@{
        ExitCode = $exitCode
        Lines = $lines
    }
}

function Get-WslCommandPaths {
    param(
        [Parameter(Mandatory)] [string]$Distro,
        [Parameter(Mandatory)] [string[]]$Tool
    )

    $script = @(
        foreach ($name in $Tool) {
            if ($name -notmatch "^[a-zA-Z0-9_.-]+$") {
                throw "Tool name '$name' is not safe for WSL provider discovery."
            }
            "if command -v $name >/dev/null 2>&1; then printf '$name|'; command -v $name; else printf '$name|missing\n'; fi"
        }
    ) -join "; "
    $result = Invoke-WslCapture -Distro $Distro -Arguments @("/bin/sh", "-c", $script)
    $paths = @{}
    foreach ($line in $result.Lines) {
        $lineMatch = [regex]::Match("$line", "^([^|]+)\|(.*)$")
        if ($lineMatch.Success) {
            $name = $lineMatch.Groups[1].Value
            $path = $lineMatch.Groups[2].Value
            $paths[$name] = if ($path -eq "missing") { $null } else { $path }
        }
    }
    return $paths
}

function Get-WslRtkOverrides {
    param([string[]]$Entries = @())

    $overrides = @{}
    foreach ($entry in $Entries) {
        $separator = $entry.IndexOf("=")
        if ($separator -le 0 -or $separator -eq ($entry.Length - 1)) {
            throw "WSL RTK overrides must use 'Distro=/absolute/linux/path'."
        }
        $distro = $entry.Substring(0, $separator)
        $path = $entry.Substring($separator + 1)
        if ($distro -match "[\r\n]" -or $path -notmatch "^/[^\r\n]+$") {
            throw "WSL RTK overrides require a single distro name and an absolute Linux path."
        }
        if ($overrides.ContainsKey($distro)) {
            throw "WSL RTK override repeats distro '$distro'."
        }
        $overrides[$distro] = $path
    }
    return $overrides
}

function Get-WslVersion {
    param(
        [Parameter(Mandatory)] [string]$Distro,
        [Parameter(Mandatory)] [string]$Path
    )

    $result = Invoke-WslCapture -Distro $Distro -Arguments @("timeout", "3s", $Path, "--version")
    return [pscustomobject]@{
        ExitCode = $result.ExitCode
        FirstLine = Get-FirstLine -Lines $result.Lines
    }
}

function Get-WslHash {
    param(
        [Parameter(Mandatory)] [string]$Distro,
        [Parameter(Mandatory)] [string]$Path
    )

    $result = Invoke-WslCapture -Distro $Distro -Arguments @("timeout", "5s", "sha256sum", "--", $Path)
    if ($result.ExitCode -ne 0) {
        return $null
    }
    $line = Get-FirstLine -Lines $result.Lines
    if ($line -match "^([a-fA-F0-9]{64})\s") {
        return $Matches[1].ToLowerInvariant()
    }
    return $null
}

function Get-RtkCommands {
    param(
        [Parameter(Mandatory)] [string]$Distro,
        [Parameter(Mandatory)] [string]$Path
    )

    $result = Invoke-WslCapture -Distro $Distro -Arguments @("timeout", "10s", $Path, "--help")
    if ($result.ExitCode -ne 0) {
        return @()
    }
    return @(
        $result.Lines |
            ForEach-Object {
                if ($_ -match "^\s{2}([a-z][a-z0-9-]*)\s{2,}") { $Matches[1] }
            } |
            Where-Object { $_ -and $_ -ne "help" } |
            Sort-Object -Unique
    )
}

function Invoke-WindowsCapture {
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] [string[]]$Arguments
    )

    $previousErrorAction = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $lines = @(
            & $Path @Arguments 2>&1 |
                ForEach-Object { "$_" -split "`r?`n" } |
                Where-Object { $_ -ne "" }
        )
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorAction
    }
    return [pscustomobject]@{
        ExitCode = $exitCode
        Lines = $lines
    }
}

function Get-WindowsRtkEvidence {
    param([Parameter(Mandatory)] [object]$Candidate)

    $version = Invoke-WindowsCapture -Path $Candidate.Path -Arguments @("--version")
    $help = Invoke-WindowsCapture -Path $Candidate.Path -Arguments @("--help")
    $commands = if ($help.ExitCode -eq 0) {
        @(
            $help.Lines |
                ForEach-Object {
                    if ($_ -match "^\s{2}([a-z][a-z0-9-]*)\s{2,}") { $Matches[1] }
                } |
                Where-Object { $_ -and $_ -ne "help" } |
                Sort-Object -Unique
        )
    } else {
        @()
    }
    return [pscustomobject]@{
        Path = $Candidate.Path
        CommandType = $Candidate.CommandType
        Sha256 = $Candidate.Sha256
        Version = Get-FirstLine -Lines $version.Lines
        VersionExitCode = $version.ExitCode
        HelpExitCode = $help.ExitCode
        Commands = $commands
    }
}

function Get-ManifestCoverage {
    param(
        [Parameter(Mandatory)] [string[]]$ManifestCommands,
        [Parameter(Mandatory)] [string[]]$ObservedCommands
    )

    $observed = @($ObservedCommands | Sort-Object -Unique)
    return [pscustomobject]@{
        ObservedCount = $observed.Count
        ObservedOnly = @($observed | Where-Object { $_ -notin $ManifestCommands })
        ManifestOnly = @($ManifestCommands | Where-Object { $_ -notin $observed })
        ExactMatch = (@($observed | Where-Object { $_ -notin $ManifestCommands }).Count -eq 0) -and
            (@($ManifestCommands | Where-Object { $_ -notin $observed }).Count -eq 0)
    }
}

function Get-WindowsCandidate {
    param([Parameter(Mandatory)] [string]$Tool)

    $matches = @(Get-Command -Name $Tool, "$Tool.exe", "$Tool.cmd", "$Tool.bat" -All -ErrorAction SilentlyContinue)
    return @(
        $matches |
            Where-Object { $_.Source -or $_.Path -or $_.Definition } |
            ForEach-Object {
                $path = if ($_.Path) { $_.Path } elseif ($_.Source) { $_.Source } else { $_.Definition }
                [pscustomobject]@{
                    Tool = $Tool
                    Path = $path
                    CommandType = "$($_.CommandType)"
                    Version = if ($_.Version) { "$($_.Version)" } else { $null }
                    Sha256 = Get-WindowsHash -Path $path
                }
            } |
            Sort-Object Path -Unique
    )
}

function Get-DeepWindowsRtkCandidates {
    param([string[]]$Roots)

    $found = [System.Collections.Generic.List[object]]::new()
    foreach ($root in $Roots) {
        if (-not (Test-Path -LiteralPath $root -PathType Container)) {
            continue
        }
        $items = if ($DeepSearch) {
            Get-ChildItem -LiteralPath $root -Filter "rtk.exe" -File -Recurse -ErrorAction SilentlyContinue
        } else {
            Get-ChildItem -LiteralPath $root -Filter "rtk.exe" -File -ErrorAction SilentlyContinue
        }
        foreach ($item in $items) {
            $found.Add([pscustomobject]@{
                Tool = "rtk"
                Path = $item.FullName
                CommandType = "filesystem_candidate"
                Version = $null
                Sha256 = Get-WindowsHash -Path $item.FullName
            })
        }
    }
    return @($found | Sort-Object Path -Unique)
}

$wslVerbose = @(& wsl.exe --list --verbose 2>&1 | ForEach-Object { ("$_" -replace "`0", "") })
$wslQuiet = @(& wsl.exe --list --quiet 2>$null | ForEach-Object { ("$_" -replace "`0", "").Trim() } | Where-Object { $_ })
$wslRtkOverrides = Get-WslRtkOverrides -Entries $WslRtkOverride
$unknownOverrideDistros = @($wslRtkOverrides.Keys | Where-Object { $_ -notin $wslQuiet })
if ($unknownOverrideDistros.Count -gt 0) {
    throw "WSL RTK overrides name undiscovered distributions: $($unknownOverrideDistros -join ', ')."
}
$wslProviders = [System.Collections.Generic.List[object]]::new()
$auditTools = @("rtk") + $Tools
foreach ($distro in $wslQuiet) {
    $versionLine = $wslVerbose | Where-Object { $_ -match "\b$([regex]::Escape($distro))\s+\S+\s+([12])\s*$" } | Select-Object -First 1
    $wslVersion = if ($versionLine -match "\s([12])\s*$") { [int]$Matches[1] } else { $null }
    $toolPaths = Get-WslCommandPaths -Distro $distro -Tool $auditTools
    $rtkPath = if ($wslRtkOverrides.ContainsKey($distro)) { $wslRtkOverrides[$distro] } else { $toolPaths["rtk"] }
    $toolProviders = [System.Collections.Generic.List[object]]::new()
    $metadataDeadline = [DateTime]::UtcNow.AddSeconds($MetadataBudgetSeconds)
    foreach ($tool in $Tools) {
        $path = $toolPaths[$tool]
        if (-not $path) {
            continue
        }
        $metadataAllowed = $ProbeToolMetadata -and [DateTime]::UtcNow -lt $metadataDeadline
        $version = if ($metadataAllowed) { Get-WslVersion -Distro $distro -Path $path } else { $null }
        $hashAllowed = $metadataAllowed -and [DateTime]::UtcNow -lt $metadataDeadline
        $toolProviders.Add([pscustomobject]@{
            Tool = $tool
            Path = $path
            Version = if ($version) { $version.FirstLine } else { $null }
            VersionExitCode = if ($version) { $version.ExitCode } else { $null }
            Sha256 = if ($hashAllowed) { Get-WslHash -Distro $distro -Path $path } else { $null }
            MetadataStatus = if (-not $ProbeToolMetadata) {
                "path_only"
            } elseif ($version -and $hashAllowed) {
                "probed"
            } elseif ($version) {
                "version_only"
            } else {
                "budget_exhausted"
            }
        })
    }
    $rtkVersion = if ($rtkPath) { Get-WslVersion -Distro $distro -Path $rtkPath } else { $null }
    $wslProviders.Add([pscustomobject]@{
        Distro = $distro
        WslVersion = $wslVersion
        Rtk = if ($rtkPath) {
            [pscustomobject]@{
                Path = $rtkPath
                Version = $rtkVersion.FirstLine
                VersionExitCode = $rtkVersion.ExitCode
                Sha256 = Get-WslHash -Distro $distro -Path $rtkPath
                Commands = Get-RtkCommands -Distro $distro -Path $rtkPath
            }
        } else {
            $null
        }
        Tools = @($toolProviders)
    })
}

$windowsRtk = @(
    Get-WindowsCandidate -Tool "rtk"
    Get-DeepWindowsRtkCandidates -Roots $SearchRoots
) | Sort-Object Path -Unique
$windowsRtkEvidence = @($windowsRtk | ForEach-Object { Get-WindowsRtkEvidence -Candidate $_ })
$windowsLaunchers = @(
    Get-WindowsCandidate -Tool "rtk-wad"
) | Sort-Object Path -Unique
$windowsTools = foreach ($tool in $Tools) { Get-WindowsCandidate -Tool $tool }

$manifest = $null
if (Test-Path -LiteralPath $ManifestPath -PathType Leaf) {
    $manifest = Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json
}
$manifestCommands = if ($manifest) {
    @(
        $manifest.native_structured +
        $manifest.raw_native +
        $manifest.wsl1_conservative +
        @($manifest.wad_internal | Where-Object { $_ -notmatch "^-" -and $_ -ne "stats" }) |
            Sort-Object -Unique
    )
} else {
    @()
}
$rtkCommandCoverage = foreach ($provider in $wslProviders | Where-Object { $_.Rtk }) {
    $coverage = Get-ManifestCoverage -ManifestCommands $manifestCommands -ObservedCommands @($provider.Rtk.Commands)
    [pscustomobject]@{
        Distro = $provider.Distro
        WslVersion = $provider.WslVersion
        ObservedCount = $coverage.ObservedCount
        ObservedOnly = $coverage.ObservedOnly
        ManifestOnly = $coverage.ManifestOnly
        ExactMatch = $coverage.ExactMatch
    }
}
$windowsRtkCoverage = foreach ($provider in $windowsRtkEvidence) {
    $coverage = Get-ManifestCoverage -ManifestCommands $manifestCommands -ObservedCommands @($provider.Commands)
    [pscustomobject]@{
        Path = $provider.Path
        Version = $provider.Version
        VersionExitCode = $provider.VersionExitCode
        HelpExitCode = $provider.HelpExitCode
        ObservedCount = $coverage.ObservedCount
        ObservedOnly = $coverage.ObservedOnly
        ManifestOnly = $coverage.ManifestOnly
        ExactMatch = $coverage.ExactMatch
    }
}
$benchmarkPreflight = [pscustomobject]@{
    Protocol = "benchmark-matrix-preflight-v1"
    ManifestCommandCount = $manifestCommands.Count
    WindowsNativeRtkReady = @($windowsRtkCoverage | Where-Object { $_.ExactMatch -and $_.VersionExitCode -eq 0 -and $_.HelpExitCode -eq 0 }).Count -gt 0
    Wsl1RtkReady = @($rtkCommandCoverage | Where-Object { $_.WslVersion -eq 1 -and $_.ExactMatch }).Count -gt 0
    Wsl2RtkReady = @($rtkCommandCoverage | Where-Object { $_.WslVersion -eq 2 -and $_.ExactMatch }).Count -gt 0
    BlockingReasons = @(
        if (@($windowsRtkCoverage | Where-Object { $_.ExactMatch -and $_.VersionExitCode -eq 0 -and $_.HelpExitCode -eq 0 }).Count -eq 0) {
            "No verified stock Windows RTK matches the embedded command manifest. Native three-way benchmark claims are blocked."
        }
        if (@($rtkCommandCoverage | Where-Object { $_.WslVersion -eq 1 -and $_.ExactMatch }).Count -eq 0) {
            "No verified WSL1 RTK matches the embedded command manifest. WSL1 RTK benchmark claims are blocked."
        }
        if (@($rtkCommandCoverage | Where-Object { $_.WslVersion -eq 2 -and $_.ExactMatch }).Count -eq 0) {
            "No verified WSL2 RTK matches the embedded command manifest. WSL2 RTK benchmark claims are blocked."
        }
    )
}

$processes = @(
    Get-CimInstance Win32_Process |
        Where-Object { $_.Name -in @("rtk.exe", "rtk-wad.exe", "wsl.exe") } |
        ForEach-Object {
            $process = Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue
            [pscustomobject]@{
                ProcessId = $_.ProcessId
                Name = $_.Name
                CreatedAt = $_.CreationDate
                WorkingSetBytes = if ($process) { $process.WorkingSet64 } else { $null }
                CommandLine = $_.CommandLine
            }
        }
)

$report = [pscustomobject]@{
    SchemaVersion = 1
    GeneratedAt = (Get-Date).ToUniversalTime().ToString("o")
    SearchScope = [pscustomobject]@{
        SearchRoots = $SearchRoots
        DeepSearch = [bool]$DeepSearch
        WslRtkOverride = @($WslRtkOverride)
        Limitation = "Windows discovery covers PATH, configured command resolution, and the declared search roots. It is not an unrestricted whole-disk crawl unless explicit roots are supplied with -DeepSearch."
    }
    Windows = [pscustomobject]@{
        RtkCandidates = @($windowsRtk)
        RtkEvidence = @($windowsRtkEvidence)
        Launchers = @($windowsLaunchers)
        ToolProviders = @($windowsTools)
    }
    Wsl = @($wslProviders)
    Manifest = [pscustomobject]@{
        Path = if ($manifest) { (Resolve-Path -LiteralPath $ManifestPath).Path } else { $null }
        UpstreamVersion = if ($manifest) { $manifest.upstream_rtk_version } else { $null }
        Coverage = @($rtkCommandCoverage)
        WindowsCoverage = @($windowsRtkCoverage)
    }
    BenchmarkPreflight = $benchmarkPreflight
    Processes = $processes
}

$json = $report | ConvertTo-Json -Depth 12
if ($OutputPath) {
    $destination = [System.IO.Path]::GetFullPath($OutputPath)
    $parent = Split-Path -Parent $destination
    if ($parent) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
    [System.IO.File]::WriteAllText($destination, $json + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
    Write-Output "Wrote provider audit to $destination"
} else {
    $json
}
