<#
.SYNOPSIS
  Samples RAM/CPU (and optionally GPU) of a whole process tree, including the
  msedgewebview2.exe / Electron helper children.

.EXAMPLE
  .\tools\bench.ps1 -Exe .\target\release\hybrid.exe -Seconds 120 -Label hybrid-idle -Close
  .\tools\bench.ps1 -ProcessName "YouTube Music Desktop App" -Seconds 120 -Label ytmd-play -Gpu

.NOTES
  Metrics per sample, summed over the tree:
    WS       = working set (resident, includes shared pages)
    WSPriv   = private working set (resident, not shared)
    Shared   = WS - WSPriv (resident pages shared with other processes, e.g. DLLs)
    Commit   = private bytes (committed private memory, resident or paged out)
    CPU%     = tree CPU time delta / (wall delta * logical cores) * 100
    GPU%     = sum of '\GPU Engine(pid_*)\Utilization Percentage' (-Gpu, slow)
  Sampling itself costs CPU in this PowerShell process, which is not part of the tree.
#>
[CmdletBinding(DefaultParameterSetName = 'Launch')]
param(
    [Parameter(ParameterSetName = 'Launch', Mandatory)][string]$Exe,
    [Parameter(ParameterSetName = 'Attach', Mandatory)][string]$ProcessName,
    [int]$Seconds = 60,
    [int]$Interval = 2,
    [string]$Label = 'run',
    # Seconds to wait after launch/attach before sampling (page load, warm-up).
    [int]$Warmup = 0,
    [string]$OutDir = (Join-Path $PSScriptRoot '..\bench'),
    [switch]$Gpu,
    # Launch mode: close the main window at the end and check for orphans.
    [switch]$Close
)
$ErrorActionPreference = 'Stop'
$cores = [Environment]::ProcessorCount
New-Item -ItemType Directory -Force $OutDir | Out-Null

function Get-Tree([int]$RootId) {
    $all = Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name
    $children = @{}
    foreach ($p in $all) {
        $key = [int]$p.ParentProcessId
        if (-not $children.ContainsKey($key)) { $children[$key] = [System.Collections.Generic.List[int]]::new() }
        $children[$key].Add([int]$p.ProcessId)
    }
    $ids = [System.Collections.Generic.List[int]]::new()
    $queue = [System.Collections.Generic.Queue[int]]::new()
    if ($all.ProcessId -contains $RootId) { $queue.Enqueue($RootId) }
    while ($queue.Count) {
        $id = $queue.Dequeue(); $ids.Add($id)
        if ($children.ContainsKey($id)) { foreach ($c in $children[$id]) { if ($c -ne $id) { $queue.Enqueue($c) } } }
    }
    , $ids
}

$launchMs = $null
if ($PSCmdlet.ParameterSetName -eq 'Launch') {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $root = Start-Process -FilePath (Resolve-Path $Exe) -PassThru
    # Time until the OS reports a main window (window shown, not "player ready").
    while ($sw.ElapsedMilliseconds -lt 30000) {
        $root.Refresh()
        if ($root.HasExited) { throw "process exited during launch (code $($root.ExitCode))" }
        if ($root.MainWindowHandle -ne 0) { $launchMs = $sw.ElapsedMilliseconds; break }
        Start-Sleep -Milliseconds 20
    }
    $rootId = $root.Id
} else {
    $procs = Get-CimInstance Win32_Process -Filter "Name = '$ProcessName.exe'"
    if (-not $procs) { throw "no running process named $ProcessName.exe" }
    $ids = @($procs.ProcessId)
    # Root = the one whose parent is not a process of the same name.
    $rootId = [int]($procs | Where-Object { $ids -notcontains $_.ParentProcessId } | Select-Object -First 1).ProcessId
}

if ($Warmup -gt 0) { Start-Sleep -Seconds $Warmup }
$csv = Join-Path $OutDir ("{0}-{1:yyyyMMdd-HHmmss}.csv" -f $Label, (Get-Date))
$rows = [System.Collections.Generic.List[object]]::new()
$prevCpu = $null; $prevT = $null
$end = (Get-Date).AddSeconds($Seconds)

while ((Get-Date) -lt $end) {
    $ids = Get-Tree $rootId
    if (-not $ids.Count) { Write-Warning 'root process exited'; break }
    $perf = Get-CimInstance Win32_PerfRawData_PerfProc_Process -Property IDProcess, WorkingSet, WorkingSetPrivate, PrivateBytes |
        Where-Object { $ids -contains [int]$_.IDProcess }
    $cpu = 0.0
    foreach ($id in $ids) {
        try { $cpu += (Get-Process -Id $id).TotalProcessorTime.TotalMilliseconds } catch { }
    }
    $now = [Diagnostics.Stopwatch]::GetTimestamp()
    $cpuPct = $null
    if ($null -ne $prevCpu) {
        $wallMs = ($now - $prevT) * 1000.0 / [Diagnostics.Stopwatch]::Frequency
        $cpuPct = [math]::Round([math]::Max(0, $cpu - $prevCpu) / ($wallMs * $cores) * 100, 2)
    }
    $prevCpu = $cpu; $prevT = $now

    $gpuPct = $null
    if ($Gpu) {
        try {
            $samples = (Get-Counter '\GPU Engine(*)\Utilization Percentage' -ErrorAction Stop).CounterSamples
            $gpuPct = [math]::Round((($samples | Where-Object {
                $_.InstanceName -match '^pid_(\d+)_' -and $ids -contains [int]$Matches[1]
            } | Measure-Object CookedValue -Sum).Sum), 2)
        } catch { $gpuPct = $null }
    }

    $ws = ($perf | Measure-Object WorkingSet -Sum).Sum
    $wsp = ($perf | Measure-Object WorkingSetPrivate -Sum).Sum
    $row = [pscustomobject]@{
        Time      = (Get-Date).ToString('HH:mm:ss')
        Processes = $ids.Count
        WS_MB     = [math]::Round($ws / 1MB, 1)
        WSPriv_MB = [math]::Round($wsp / 1MB, 1)
        Shared_MB = [math]::Round(($ws - $wsp) / 1MB, 1)
        Commit_MB = [math]::Round(($perf | Measure-Object PrivateBytes -Sum).Sum / 1MB, 1)
        CPU_Pct   = $cpuPct
        GPU_Pct   = $gpuPct
    }
    $rows.Add($row)
    $row | Format-Table -HideTableHeaders | Out-String | ForEach-Object { $_.Trim() } | Write-Host
    Start-Sleep -Seconds $Interval
}

$rows | Export-Csv -NoTypeInformation -Path $csv
$stat = { param($n) $m = $rows | Where-Object { $null -ne $_.$n } | Measure-Object $n -Average -Maximum -Minimum
    if ($m.Count) { '{0,-10} avg {1,8:N1}  min {2,8:N1}  max {3,8:N1}' -f $n, $m.Average, $m.Minimum, $m.Maximum } }

Write-Host "`n== $Label ($($rows.Count) samples, ${Interval}s interval, $cores logical cores) -> $csv"
if ($null -ne $launchMs) { Write-Host "Launch to main window: $launchMs ms" }
foreach ($n in 'Processes', 'WS_MB', 'WSPriv_MB', 'Shared_MB', 'Commit_MB', 'CPU_Pct', 'GPU_Pct') { & $stat $n | Write-Host }

if ($Close -and $PSCmdlet.ParameterSetName -eq 'Launch') {
    $tree = Get-Tree $rootId
    $root.CloseMainWindow() | Out-Null
    Start-Sleep -Seconds 5
    $alive = @($tree | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
    if ($alive.Count) { Write-Warning "still alive 5s after close: $($alive -join ', ')" }
    else { Write-Host "Clean shutdown: all $($tree.Count) processes exited." }
}
