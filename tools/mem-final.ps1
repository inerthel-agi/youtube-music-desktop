<#
.SYNOPSIS
  Idle memory per process type, then tree CPU while playing, for the default
  configuration and the Options "memory saver" (+ hardware acceleration off).
#>
param([int]$Settle = 45, [string]$PlayUrl = 'https://music.youtube.com/watch?v=dQw4w9WgXcQ')
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root 'target\release\youtube-music.exe'
$def = '--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection'
$saver = "$def --renderer-process-limit=1"
$sets = [ordered]@{
    'default'             = $def
    'memory saver'        = $saver
    'saver + no GPU'      = "$saver --disable-gpu --disable-gpu-compositing"
}
foreach ($name in $sets.Keys) {
    $env:YTM_BROWSER_ARGS = $sets[$name]
    foreach ($phase in 'idle', 'playing') {
        Get-Process youtube-music -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep 2
        if ($phase -eq 'playing') { $env:YTM_START_URL = $PlayUrl } else { Remove-Item Env:YTM_START_URL -ErrorAction SilentlyContinue }
        Start-Process $exe | Out-Null
        Start-Sleep $Settle
        Write-Host "== $name / $phase"
        if ($phase -eq 'idle') {
            & (Join-Path $PSScriptRoot 'mem-breakdown.ps1') -ProcessName youtube-music -Samples 5 | Out-String | Write-Host
        } else {
            & (Join-Path $PSScriptRoot 'bench.ps1') -ProcessName youtube-music -Seconds 30 -Interval 3 -Label "final-$($name -replace '\W','')" -Gpu 6>&1 |
                Where-Object { "$_" -match 'WSPriv|CPU_Pct|GPU_Pct' } | Write-Host
        }
    }
}
Remove-Item Env:YTM_BROWSER_ARGS, Env:YTM_START_URL -ErrorAction SilentlyContinue
Get-Process youtube-music -ErrorAction SilentlyContinue | Stop-Process -Force
