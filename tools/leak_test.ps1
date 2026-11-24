# Finds which periodic job makes DesktopShepherd's memory grow: runs the program once per mode
# with DESKTOPSHEPHERD_LEAKTEST=<mode> (that job repeats every minute instead of every 30), waits
# for it to settle, then samples private memory every 15 s. A job that leaks climbs each minute.
#
#   pwsh -File tools\leak_test.ps1 [-Modes none,agenda,weather,backdrop] [-Minutes 5]
param([string[]]$Modes = @('none', 'agenda', 'weather', 'backdrop'), [int]$Minutes = 5, [int]$WarmupSeconds = 60)
[System.Threading.Thread]::CurrentThread.CurrentCulture = [cultureinfo]::InvariantCulture
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root 'target\debug\desktopshepherd.exe'
$out = Join-Path $root ("profile\leaktest-{0:yyyyMMdd-HHmm}.csv" -f (Get-Date))
New-Item -ItemType Directory -Force (Split-Path $out) | Out-Null
'mode,seconds,private_mb,working_set_mb' | Set-Content $out
foreach ($mode in $Modes) {
    & pwsh -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root 'scripts\stop.ps1') | Out-Null
    $env:DESKTOPSHEPHERD_LEAKTEST = if ($mode -eq 'none') { '' } else { $mode }
    Start-Process $exe | Out-Null
    Start-Sleep $WarmupSeconds
    $t0 = Get-Date
    while (((Get-Date) - $t0).TotalMinutes -lt $Minutes) {
        $w = Get-CimInstance Win32_Process -Filter "Name='desktopshepherd.exe'" | Where-Object CommandLine -match '--worker' | Select-Object -First 1
        if ($w) {
            $p = Get-Process -Id $w.ProcessId -ErrorAction SilentlyContinue
            if ($p) { '{0},{1:F0},{2:F2},{3:F2}' -f $mode, ((Get-Date) - $t0).TotalSeconds, ($p.PrivateMemorySize64 / 1MB), ($p.WorkingSet64 / 1MB) | Add-Content $out }
        }
        Start-Sleep 15
    }
}
# Back to normal.
& pwsh -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root 'scripts\stop.ps1') | Out-Null
Remove-Item Env:DESKTOPSHEPHERD_LEAKTEST -ErrorAction SilentlyContinue
Start-Process $exe | Out-Null
"done: $out"
