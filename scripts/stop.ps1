# Stops DeskBoxes safely: the worker first (the supervisor then restores the desktop icons),
# then the supervisor. Use this before rebuilding (the .exe is locked while it runs).
$w = Get-CimInstance Win32_Process -Filter "Name='deskboxes.exe'" | Where-Object { $_.CommandLine -match '--worker' }
if ($w) { Stop-Process -Id $w.ProcessId -Force }
Start-Sleep -Milliseconds 700
Stop-Process -Name deskboxes -Force -ErrorAction SilentlyContinue
