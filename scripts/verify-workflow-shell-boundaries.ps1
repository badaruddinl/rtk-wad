[CmdletBinding()]
param(
    [string]$WorkflowDirectory = (Join-Path $PSScriptRoot "..\.github\workflows")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$forbidden = '\$\{\{\s*(?:inputs\.|github\.event\.)'
$violations = [System.Collections.Generic.List[string]]::new()
foreach ($workflow in Get-ChildItem -LiteralPath $WorkflowDirectory -File |
    Where-Object Extension -in ".yml", ".yaml") {
    $lines = @(Get-Content -LiteralPath $workflow.FullName)
    $runIndent = $null
    for ($index = 0; $index -lt $lines.Count; $index++) {
        $line = $lines[$index]
        $indent = $line.Length - $line.TrimStart().Length
        if ($null -ne $runIndent -and $line.Trim() -and $indent -le $runIndent) {
            $runIndent = $null
        }
        if ($null -ne $runIndent -and $line -match $forbidden) {
            $violations.Add("$($workflow.Name):$($index + 1)")
        }
        if ($line -match '^(\s*)(?:-\s*)?run:\s*(.*)$') {
            $runIndent = $Matches[1].Length
            if ($Matches[2] -match $forbidden) {
                $violations.Add("$($workflow.Name):$($index + 1)")
            }
        }
    }
}
if ($violations.Count) {
    throw "Untrusted workflow expression is embedded in shell source at: $($violations -join ', '). Pass it through step env instead."
}

Write-Output "Workflow shell-expression boundary contract passed."
