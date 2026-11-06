# Launch / WM_CLOSE cycles: reports whether the whole process tree exits.
param([string]$App = 'hybrid', [int]$Cycles = 6, [int]$RunSeconds = 25, [int]$WaitSeconds = 10)
$exe = Join-Path (Split-Path $PSScriptRoot -Parent) "target\release\$App.exe"
$fails = 0
for ($i = 1; $i -le $Cycles; $i++) {
    $p = Start-Process $exe -PassThru
    Start-Sleep $RunSeconds
    $p.Refresh()
    $closed = $p.CloseMainWindow()
    $exited = $p.WaitForExit($WaitSeconds * 1000)
    $kids = @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $($p.Id)").Count
    if (-not $exited) { $fails++; Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
    "{0} cycle {1}: WM_CLOSE posted={2} exited={3} children_left={4}" -f $App, $i, $closed, $exited, $kids
    Start-Sleep 3
}
"$App`: $fails/$Cycles cycles did not exit within ${WaitSeconds}s"
