# Stops DeskBoxes cleanly. First it asks the worker to quit by itself (named event
# Local\DeskBoxes.Quit), so the layout is saved and the supervisor ends normally and restores
# the desktop icons. It waits up to 5 seconds for that. Only if something is still running
# (an old build without the signal, or a hang) does it fall back to killing the worker and
# then the supervisor. Use this before rebuilding (the .exe is locked while it runs).
$w = Get-CimInstance Win32_Process -Filter "Name='deskboxes.exe'" | Where-Object { $_.CommandLine -match '--worker' }
if (-not (Get-Process -Name deskboxes -ErrorAction SilentlyContinue)) {
    Write-Host "DeskBoxes was not running."
    return
}
if ($w) {
    try {
        $ev = [System.Threading.EventWaitHandle]::OpenExisting('Local\DeskBoxes.Quit')
        [void]$ev.Set()
        $ev.Dispose()
    } catch { }
}
$deadline = (Get-Date).AddSeconds(5)
while ((Get-Process -Name deskboxes -ErrorAction SilentlyContinue) -and ((Get-Date) -lt $deadline)) {
    Start-Sleep -Milliseconds 100
}
if (Get-Process -Name deskboxes -ErrorAction SilentlyContinue) {
    $w = Get-CimInstance Win32_Process -Filter "Name='deskboxes.exe'" | Where-Object { $_.CommandLine -match '--worker' }
    if ($w) { Stop-Process -Id $w.ProcessId -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Milliseconds 700
    Stop-Process -Name deskboxes -Force -ErrorAction SilentlyContinue
    Write-Host "DeskBoxes did not quit on its own - had to force it."
} else {
    Write-Host "DeskBoxes stopped cleanly."
}
