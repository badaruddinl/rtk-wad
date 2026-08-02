[CmdletBinding()]
param(
    [string]$NativeRtk,
    [string]$Xuva,
    [string]$ManifestPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false

$Xuva = if ($Xuva) { $Xuva } else { Join-Path $PSScriptRoot "..\target\release\xuva.exe" }
$ManifestPath = if ($ManifestPath) { $ManifestPath } else { Join-Path $PSScriptRoot "command-manifest.json" }

function Invoke-ProviderCapture {
    param(
        [Parameter(Mandatory)] [string]$Executable,
        [Parameter(Mandatory)] [string[]]$Arguments
    )

    $previousErrorAction = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $lines = @(
            & $Executable @Arguments |
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

function Resolve-CommandPath {
    param(
        [Parameter(Mandatory)] [string]$Value,
        [Parameter(Mandatory)] [string]$Label
    )

    if (Test-Path -LiteralPath $Value -PathType Leaf) {
        return (Resolve-Path -LiteralPath $Value -ErrorAction Stop).Path
    }
    $command = Get-Command $Value -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($command -and ($command.Source -or $command.Path)) {
        return $(if ($command.Path) { $command.Path } else { $command.Source })
    }
    throw "$Label was not found: $Value"
}

$rtkManifest = Get-Content -Raw -LiteralPath $ManifestPath | ConvertFrom-Json
if ($rtkManifest.schema_version -ne 3 -or
    $rtkManifest.adapter.name -ne "rtk" -or
    $rtkManifest.adapter.protocol_version -ne 1 -or
    -not $rtkManifest.adapter.version -or
    @($rtkManifest.adapter.compatible_versions).Count -eq 0 -or
    @($rtkManifest.adapter.compatible_versions) -notcontains $rtkManifest.adapter.version) {
    throw "Command manifest does not declare a supported RTK adapter protocol."
}
$readOnlyGit = @($rtkManifest.raw_read_only_subcommands.git)
$mutationGit = @($rtkManifest.raw_mutation_subcommands.git)
if ($readOnlyGit.Count -eq 0 -or $mutationGit.Count -eq 0 -or
    @($readOnlyGit | Where-Object { $mutationGit -contains $_ }).Count -ne 0) {
    throw "Command manifest Git subcommand contracts are empty or overlapping."
}
$providerKind = $null
$providerDescription = $null
$versionResult = $null
$helpResult = $null

if ($NativeRtk) {
    $nativeRtkPath = Resolve-CommandPath -Value $NativeRtk -Label "Native RTK"
    $versionResult = Invoke-ProviderCapture -Executable $nativeRtkPath -Arguments @("--version")
    $helpResult = Invoke-ProviderCapture -Executable $nativeRtkPath -Arguments @("--help")
    $providerKind = "windows-native"
    $providerDescription = $nativeRtkPath
} else {
    $xuvaPath = Resolve-CommandPath -Value $Xuva -Label "XUVA launcher"
    $doctorResult = Invoke-ProviderCapture -Executable $xuvaPath -Arguments @("doctor", "rtk", "--json")
    if ($doctorResult.ExitCode -ne 0) {
        throw "XUVA could not discover a verified RTK provider (exit $($doctorResult.ExitCode))."
    }
    try {
        $doctor = ($doctorResult.Lines -join [Environment]::NewLine) | ConvertFrom-Json
    } catch {
        throw "XUVA returned invalid RTK provider evidence: $($_.Exception.Message)"
    }
    if ($doctor.schema_version -lt 4 -or $doctor.tool -ne "rtk") {
        throw "XUVA returned an unsupported RTK provider-evidence schema."
    }

    $candidates = @($doctor.candidates)
    if ($null -eq $doctor.recommended -or $doctor.recommended -lt 0 -or
        $doctor.recommended -ge $candidates.Count) {
        throw "XUVA did not recommend a verified RTK provider."
    }
    $candidateIndex = [int]$doctor.recommended
    $candidate = $candidates[$candidateIndex]
    $candidateAdapters = @($candidate.adapters)
    if (-not [bool]$candidate.usable -or
        $candidate.host -notin @("windows", "wsl1", "wsl2") -or
        $candidateAdapters -notcontains "raw" -or
        -not $candidate.executable) {
        throw "XUVA recommended an unusable or unsupported RTK provider."
    }

    if ($candidate.host -eq "windows") {
        $evidence = $doctor.availability.windows
        if (-not $evidence.executable_identity -or -not $evidence.executable_version -or
            $evidence.executable -ne $candidate.executable) {
            throw "The recommended Windows RTK provider lacks matching identity and version evidence."
        }
    } else {
        $matchingEvidence = @(
            $doctor.availability.wsl |
                Where-Object {
                    $_.distro -eq $candidate.distro -and
                    $_.executable -eq $candidate.executable -and
                    $_.wsl_version -eq $candidate.wsl_version
                }
        )
        if ($matchingEvidence.Count -ne 1 -or
            -not $matchingEvidence[0].executable_identity -or
            -not $matchingEvidence[0].executable_version -or
            $candidate.wsl_version -notin @(1, 2)) {
            throw "The recommended WSL RTK provider lacks matching distro, identity, and version evidence."
        }
        $evidence = $matchingEvidence[0]
    }

    $providerArguments = @("provider", "exec", "rtk", "--candidate", "$candidateIndex", "--")
    $versionResult = Invoke-ProviderCapture -Executable $xuvaPath -Arguments ($providerArguments + "--version")
    $helpResult = Invoke-ProviderCapture -Executable $xuvaPath -Arguments ($providerArguments + "--help")
    $observedVersion = @($versionResult.Lines | Where-Object { $_.Trim() } | Select-Object -First 1)
    if ($observedVersion.Count -ne 1 -or $observedVersion[0] -ne $evidence.executable_version) {
        throw "The executed RTK provider version does not match XUVA's verified provider evidence."
    }
    $providerKind = "$($candidate.host)"
    $providerDescription = if ($candidate.distro) {
        "xuva candidate $candidateIndex ($($candidate.distro):$($candidate.executable))"
    } else {
        "xuva candidate $candidateIndex ($($candidate.executable))"
    }
}

if ($versionResult.ExitCode -ne 0 -or -not @($versionResult.Lines | Where-Object { $_.Trim() })) {
    throw "Unable to read RTK version from $providerDescription."
}
$observedAdapterVersion = @($versionResult.Lines | Where-Object { $_.Trim() } | Select-Object -First 1)[0]
$observedAdapterMatch = [regex]::Match($observedAdapterVersion, '^rtk v?([0-9]+(?:\.[0-9]+)+)$', [System.Text.RegularExpressions.RegexOptions]::IgnoreCase)
$compatibleVersions = @($rtkManifest.adapter.compatible_versions | ForEach-Object { [string]$_ })
if (-not $observedAdapterMatch.Success -or $compatibleVersions -notcontains $observedAdapterMatch.Groups[1].Value) {
    throw "RTK adapter version mismatch: expected one of '$($compatibleVersions -join ', ')', observed '$observedAdapterVersion'."
}
if ($helpResult.ExitCode -ne 0) {
    throw "Unable to read RTK help from $providerDescription."
}

$rtkCommands = $helpResult.Lines |
    Where-Object { $_ -match '^  ([a-z][a-z0-9-]*)\s{2,}' } |
    ForEach-Object { ([regex]::Match($_, '^  ([a-z][a-z0-9-]*)\s{2,}')).Groups[1].Value } |
    Where-Object { $_ -ne "help" } |
    Sort-Object -Unique
$manifestCommands = [System.Collections.Generic.List[string]]::new()
foreach ($command in $rtkManifest.native_structured) { $manifestCommands.Add([string]$command) }
foreach ($command in $rtkManifest.raw_native) { $manifestCommands.Add([string]$command) }
foreach ($command in $rtkManifest.wsl1_conservative) { $manifestCommands.Add([string]$command) }
foreach ($command in $rtkManifest.core_internal) {
    if ($command -notmatch '^-' -and $command -ne "stats") { $manifestCommands.Add([string]$command) }
}
$manifestCommands = [string[]]@($manifestCommands.ToArray() | Sort-Object -Unique)
$differences = Compare-Object -ReferenceObject $rtkCommands -DifferenceObject $manifestCommands
if ($differences) {
    $detail = $differences | ForEach-Object { "$($_.SideIndicator)$($_.InputObject)" }
    throw "Command manifest mismatch: $($detail -join ', ')."
}

Write-Output "provider_kind=$providerKind"
Write-Output "provider=$providerDescription"
Write-Output "Command manifest covers $($rtkCommands.Count) RTK command families."
