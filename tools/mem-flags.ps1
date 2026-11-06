<#
.SYNOPSIS
  A/B test of WebView2 argument sets: launches the app once per set
  (YTM_BROWSER_ARGS), waits, then prints the private working set per process type.
#>
param([int]$Settle = 45, [int]$Samples = 5)
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root 'target\release\baseline.exe'
$base = 'msWebOOUI,msPdfOOUI,msSmartScreenProtection'
$extra = 'SpareRendererForSitePerProcess,BackForwardCache,Translate,OptimizationHints,MediaRouter,AutofillServerCommunication'
$sets = [ordered]@{
    'A default'           = "--disable-features=$base"
    'B features off'      = "--disable-features=$base,$extra"
    'C B + 1 renderer'    = "--disable-features=$base,$extra --renderer-process-limit=1"
    'D B + V8 lite mode'  = "--disable-features=$base,$extra --js-flags=--lite-mode"
    'E B + no GPU'        = "--disable-features=$base,$extra --disable-gpu --disable-gpu-compositing"
}
foreach ($name in $sets.Keys) {
    Get-Process baseline -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep 2
    $env:YTM_BROWSER_ARGS = $sets[$name]
    $p = Start-Process $exe -PassThru
    Start-Sleep $Settle
    Write-Host "== $name : $($sets[$name])"
    & (Join-Path $PSScriptRoot 'mem-breakdown.ps1') -ProcessName baseline -Samples $Samples | Out-String | Write-Host
    $cpu = (Get-Process -Id $p.Id).TotalProcessorTime.TotalSeconds
    Write-Host ("app process CPU time so far: {0:N2} s" -f $cpu)
}
Remove-Item Env:YTM_BROWSER_ARGS
Get-Process baseline -ErrorAction SilentlyContinue | Stop-Process -Force
