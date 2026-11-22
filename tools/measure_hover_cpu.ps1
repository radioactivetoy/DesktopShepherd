# Measures the worker's CPU while the mouse moves in circles over a box for 10 s, then while
# idle for 10 s. Moves the real mouse cursor (puts it back afterwards).
# Usage: .\tools\measure_hover_cpu.ps1 [-X 400] [-Y 300] [-R 120]   (centre and radius in pixels)
param([int]$X = 400, [int]$Y = 300, [int]$R = 120)
Add-Type 'using System; using System.Runtime.InteropServices; public static class M { [StructLayout(LayoutKind.Sequential)] public struct P { public int x, y; } [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y); [DllImport("user32.dll")] public static extern bool GetCursorPos(out P p); }'
$w = Get-CimInstance Win32_Process -Filter "Name='desktopshepherd.exe'" | Where-Object CommandLine -match worker
if (-not $w) { "DesktopShepherd worker is not running"; exit 1 }
$p = Get-Process -Id $w.ProcessId
$cores = [Environment]::ProcessorCount
$orig = New-Object M+P; [void][M]::GetCursorPos([ref]$orig)
# The boxes must be visible: show the desktop first (Win+D) if an app is in front, and put
# things back at the end.
Add-Type 'using System; using System.Runtime.InteropServices; public static class MD { [DllImport("user32.dll")] public static extern void keybd_event(byte k, byte s, uint f, UIntPtr e); [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow(); [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, System.Text.StringBuilder s, int n); public static string Fg(){ var s=new System.Text.StringBuilder(64); GetClassName(GetForegroundWindow(),s,64); return s.ToString(); } public static void WinD(){ keybd_event(0x5B,0,0,UIntPtr.Zero); keybd_event(0x44,0,0,UIntPtr.Zero); keybd_event(0x44,0,2,UIntPtr.Zero); keybd_event(0x5B,0,2,UIntPtr.Zero); } }'
$pressed = $false
if ([MD]::Fg() -ne 'Progman' -and [MD]::Fg() -ne 'WorkerW') { [MD]::WinD(); $pressed = $true; Start-Sleep 1.5 }

$a = $p.TotalProcessorTime.TotalMilliseconds
$sw = [Diagnostics.Stopwatch]::StartNew()
while ($sw.ElapsedMilliseconds -lt 10000) {
  $t = $sw.ElapsedMilliseconds / 1000.0 * 2 * [Math]::PI / 2   # one circle every 2 s
  [void][M]::SetCursorPos([int]($X + $R * [Math]::Cos($t)), [int]($Y + $R * [Math]::Sin($t)))
  Start-Sleep -Milliseconds 8
}
$p.Refresh(); $b = $p.TotalProcessorTime.TotalMilliseconds
[void][M]::SetCursorPos($orig.x, $orig.y)
"moving mouse: cpu={0:N2}% of all cores ({1:N0} ms CPU in 10 s)" -f (($b-$a)/10000/$cores*100), ($b-$a)

Start-Sleep 2
$p.Refresh(); $a = $p.TotalProcessorTime.TotalMilliseconds
Start-Sleep 10
$p.Refresh(); $b = $p.TotalProcessorTime.TotalMilliseconds
"idle:         cpu={0:N2}% of all cores ({1:N0} ms CPU in 10 s), ws={2:N0} MB" -f (($b-$a)/10000/$cores*100), ($b-$a), ($p.WorkingSet64/1MB)
if ($pressed) { [MD]::WinD() }
