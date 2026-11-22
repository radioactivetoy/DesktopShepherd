# Stops DesktopShepherd cleanly. First it asks the worker to quit by itself (named event
# Local\DesktopShepherd.Quit), so the layout is saved and the supervisor ends normally and restores
# the desktop icons. It waits up to 5 seconds for that. Only if something is still running
# (an old build without the signal, or a hang) does it fall back to killing the worker and
# then the supervisor. Use this before rebuilding (the .exe is locked while it runs).
$w = Get-CimInstance Win32_Process -Filter "Name='desktopshepherd.exe'" | Where-Object { $_.CommandLine -match '--worker' }
if (-not (Get-Process -Name desktopshepherd -ErrorAction SilentlyContinue)) {
    Write-Host "DesktopShepherd was not running."
    return
}
if ($w) {
    try {
        $ev = [System.Threading.EventWaitHandle]::OpenExisting('Local\DesktopShepherd.Quit')
        [void]$ev.Set()
        $ev.Dispose()
    } catch { }
}
$deadline = (Get-Date).AddSeconds(5)
while ((Get-Process -Name desktopshepherd -ErrorAction SilentlyContinue) -and ((Get-Date) -lt $deadline)) {
    Start-Sleep -Milliseconds 100
}
if (Get-Process -Name desktopshepherd -ErrorAction SilentlyContinue) {
    $w = Get-CimInstance Win32_Process -Filter "Name='desktopshepherd.exe'" | Where-Object { $_.CommandLine -match '--worker' }
    if ($w) { Stop-Process -Id $w.ProcessId -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Milliseconds 700
    Stop-Process -Name desktopshepherd -Force -ErrorAction SilentlyContinue
    Write-Host "DesktopShepherd did not quit on its own - had to force it."
} else {
    Write-Host "DesktopShepherd stopped cleanly."
}
