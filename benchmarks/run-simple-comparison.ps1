# Full Benchmark Comparison: Raw Windows vs Stock RTK vs XUVA
$xuvaExe = "C:\Users\90174228\AppData\Local\Programs\XUVA\xuva.exe"
$rtkExe  = "C:\Users\90174228\.cargo\bin\rtk.exe"
$gitExe  = "C:\Program Files\Git\cmd\git.exe"
$rgExe   = "C:\Users\90174228\.cargo\bin\rg.exe"
$repoDir = "d:\luthfi\project\rtk-wad"

$workloads = @(
    @{ Name = "git status";      RawCmd = $gitExe; RawArgs = @("status", "--short", "--branch");  RtkArgs = @("git", "status", "--short", "--branch");  XuvaArgs = @("git", "status", "--short", "--branch") },
    @{ Name = "git log 100";     RawCmd = $gitExe; RawArgs = @("log", "--oneline", "-100");       RtkArgs = @("git", "log", "--oneline", "-100");       XuvaArgs = @("git", "log", "--oneline", "-100") },
    @{ Name = "ripgrep focused"; RawCmd = $rgExe;  RawArgs = @("-n", "struct", "src");            RtkArgs = @("rg", "-n", "struct", "src");             XuvaArgs = @("rg", "-n", "struct", "src") },
    @{ Name = "ripgrep broad";   RawCmd = $rgExe;  RawArgs = @("-n", "pub|fn|struct", "src");     RtkArgs = @("rg", "-n", "pub|fn|struct", "src");      XuvaArgs = @("rg", "-n", "pub|fn|struct", "src") }
)

Write-Host "=== THREE-WAY BENCHMARK COMPARISON ===" -ForegroundColor Cyan
Write-Host "XUVA Version : "$(& $xuvaExe --version)
Write-Host "RTK Version  : "$(& $rtkExe --version)
Write-Host "Rounds per workload: 10`n"

foreach ($w in $workloads) {
    Write-Host "Workload: $($w.Name)" -ForegroundColor Yellow
    
    # 1. Measure Raw Windows
    $rawTimes = @()
    for ($i = 0; $i -lt 10; $i++) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $p = Start-Process -FilePath $w.RawCmd -ArgumentList $w.RawArgs -WorkingDirectory $repoDir -NoNewWindow -PassThru -Wait -RedirectStandardOutput "$env:TEMP\bench_raw.out"
        $sw.Stop()
        $rawTimes += $sw.Elapsed.TotalMilliseconds
    }
    $rawSorted = $rawTimes | Sort-Object
    $rawMedian = $rawSorted[4]
    
    # 2. Measure Stock RTK
    $rtkTimes = @()
    for ($i = 0; $i -lt 10; $i++) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $p = Start-Process -FilePath $rtkExe -ArgumentList $w.RtkArgs -WorkingDirectory $repoDir -NoNewWindow -PassThru -Wait -RedirectStandardOutput "$env:TEMP\bench_rtk.out"
        $sw.Stop()
        $rtkTimes += $sw.Elapsed.TotalMilliseconds
    }
    $rtkSorted = $rtkTimes | Sort-Object
    $rtkMedian = $rtkSorted[4]

    # 3. Measure XUVA Dispatcher
    $xuvaTimes = @()
    for ($i = 0; $i -lt 10; $i++) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $p = Start-Process -FilePath $xuvaExe -ArgumentList $w.XuvaArgs -WorkingDirectory $repoDir -NoNewWindow -PassThru -Wait -RedirectStandardOutput "$env:TEMP\bench_xuva.out"
        $sw.Stop()
        $xuvaTimes += $sw.Elapsed.TotalMilliseconds
    }
    $xuvaSorted = $xuvaTimes | Sort-Object
    $xuvaMedian = $xuvaSorted[4]
    
    Write-Host "  Raw Windows Median Latency : $([math]::Round($rawMedian, 2)) ms"
    Write-Host "  Stock RTK Median Latency   : $([math]::Round($rtkMedian, 2)) ms"
    Write-Host "  XUVA Median Latency        : $([math]::Round($xuvaMedian, 2)) ms"
    $diffRaw = $xuvaMedian - $rawMedian
    $diffRtk = $xuvaMedian - $rtkMedian
    Write-Host "  Overhead vs Raw            : $(if ($diffRaw -ge 0) {'+'} else {''})$([math]::Round($diffRaw, 2)) ms"
    Write-Host "  Overhead vs Stock RTK      : $(if ($diffRtk -ge 0) {'+'} else {''})$([math]::Round($diffRtk, 2)) ms`n"
}
