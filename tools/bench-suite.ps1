<#
.SYNOPSIS
  Runs the prototype benchmark suite sequentially:
    1. idle (home page, nothing playing) for youtube-music (baseline), hybrid and YTMDesktop;
    2. playing, then playing while minimized, for youtube-music and hybrid.
  Playback starts on its own from the same watch URL in both prototypes
  (YTM_START_URL), so both play the same content. YTMDesktop is measured idle
  only: start its playback by hand, then run bench.ps1 -ProcessName.
#>
param(
    [string]$YtmdExe = "$env:LOCALAPPDATA\youtube_music_desktop_app\app-2.1.0\youtube-music-desktop-app.exe",
    [string]$PlayUrl = 'https://music.youtube.com/watch?v=dQw4w9WgXcQ',
    [int]$IdleSeconds = 90,
    [int]$PlaySeconds = 60,
    [int]$Interval = 3,
    [switch]$SkipIdle
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$bench = Join-Path $PSScriptRoot 'bench.ps1'

Add-Type @"
using System; using System.Runtime.InteropServices;
public static class BenchWin { [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c); }
"@

function Last-State([string]$App) {
    $id = if ($App -eq 'youtube-music') { 'baseline' } else { $App }   # identifier kept from the prototype
    $f = Get-ChildItem "$env:LOCALAPPDATA\dev.ytm-desktop.$id\logs" | Sort-Object LastWriteTime | Select-Object -Last 1
    Get-Content $f.FullName -Encoding UTF8 | Where-Object { $_ -match 'player: |first playback' } | Select-Object -Last 2
}

if (-not $SkipIdle) {
    Remove-Item Env:YTM_START_URL -ErrorAction SilentlyContinue
    foreach ($app in 'youtube-music', 'hybrid') {
        & $bench -Exe (Join-Path $root "target\release\$app.exe") -Seconds $IdleSeconds -Interval $Interval -Warmup 20 -Label "$app-idle" -Gpu -Close
        Start-Sleep 5
    }
    if (Test-Path $YtmdExe) {
        & $bench -Exe $YtmdExe -Seconds $IdleSeconds -Interval $Interval -Warmup 20 -Label 'ytmdesktop-idle' -Gpu -Close
        # YTMDesktop hides to the tray on close; end the instance this script started.
        Get-Process youtube-music-desktop-app -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep 5
    }
}

$env:YTM_START_URL = $PlayUrl
foreach ($app in 'youtube-music', 'hybrid') {
    $p = Start-Process (Join-Path $root "target\release\$app.exe") -PassThru
    Start-Sleep 60   # page load + pre-roll ads (not signed in) + track start
    Write-Host "$app-playcheck: $(Last-State $app)"
    & $bench -ProcessName $app -Seconds $PlaySeconds -Interval $Interval -Label "$app-playing" -Gpu
    [BenchWin]::ShowWindow($p.MainWindowHandle, 6) | Out-Null   # SW_MINIMIZE
    & $bench -ProcessName $app -Seconds $PlaySeconds -Interval $Interval -Warmup 5 -Label "$app-minimized" -Gpu
    Write-Host "$app-minimized-playcheck: $(Last-State $app)"
    [BenchWin]::ShowWindow($p.MainWindowHandle, 9) | Out-Null   # SW_RESTORE
    Start-Sleep 2
    $p.CloseMainWindow() | Out-Null
    Start-Sleep 5
    Write-Host "$app closed: $(-not (Get-Process -Id $p.Id -ErrorAction SilentlyContinue))"
}
Remove-Item Env:YTM_START_URL
