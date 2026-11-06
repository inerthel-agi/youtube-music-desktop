<#
.SYNOPSIS
  Private working set per WebView2 process type for a running app tree
  (browser, renderer, gpu-process, utility, ...). Samples N times and averages.
.EXAMPLE
  .\tools\mem-breakdown.ps1 -ProcessName baseline -Samples 5
#>
param([string]$ProcessName = 'baseline', [int]$Samples = 5, [int]$Interval = 3)
$root = (Get-Process $ProcessName -ErrorAction Stop | Sort-Object StartTime | Select-Object -First 1).Id
$acc = @{}
for ($n = 0; $n -lt $Samples; $n++) {
    $all = Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, CommandLine
    $ids = @($root)
    do { $c = $ids.Count; $ids = @($ids + ($all | Where-Object { $ids -contains $_.ParentProcessId }).ProcessId) | Sort-Object -Unique } while ($ids.Count -ne $c)
    $perf = Get-CimInstance Win32_PerfRawData_PerfProc_Process -Property IDProcess, WorkingSetPrivate | Where-Object { $ids -contains [int]$_.IDProcess }
    foreach ($p in $perf) {
        $cmd = ($all | Where-Object ProcessId -eq ([int]$p.IDProcess)).CommandLine
        $type = if ([int]$p.IDProcess -eq $root) { 'app' }
                elseif ($cmd -match '--type=([\w-]+)') { $Matches[1] + $(if ($cmd -match '--utility-sub-type=([\w.]+)') { ':' + $Matches[1].Split('.')[-1] }) }
                else { 'browser' }
        if (-not $acc.ContainsKey($type)) { $acc[$type] = @() }
        $acc[$type] += [double]$p.WorkingSetPrivate
    }
    Start-Sleep $Interval
}
$rows = foreach ($k in $acc.Keys) {
    [pscustomobject]@{ Type = $k; Count = [math]::Round($acc[$k].Count / $Samples, 1); PrivWS_MB = [math]::Round((($acc[$k] | Measure-Object -Sum).Sum / $Samples) / 1MB, 1) }
}
$rows | Sort-Object PrivWS_MB -Descending | Format-Table -AutoSize
'TOTAL private WS: {0:N1} MB' -f (($rows | Measure-Object PrivWS_MB -Sum).Sum)
