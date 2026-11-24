# Records DesktopShepherd's resource use over hours, to spot slow leaks.
#
# Every -IntervalSeconds (default 300) it appends one line per process (worker and supervisor) to
# a CSV: memory (working set, private), processor time used since the previous sample, handles,
# GDI and USER objects, threads, and the process id (a new id = the program was restarted).
# It only reads public process information; it never touches the program itself.
#
#   pwsh -File tools\profile_over_time.ps1                       # until stopped
#   pwsh -File tools\profile_over_time.ps1 -Hours 12 -IntervalSeconds 120
#
# Output: profile\desktopshepherd-<start time>.csv next to this repository's tools folder
# (the profile folder is not committed).
param(
    [double]$Hours = 0,           # 0 = run until stopped
    [int]$IntervalSeconds = 300,
    [string]$OutDir = (Join-Path (Split-Path $PSScriptRoot -Parent) 'profile')
)

Add-Type @'
using System; using System.Runtime.InteropServices;
public static class GuiRes {
    [DllImport("user32.dll")] public static extern uint GetGuiResources(IntPtr hProcess, uint uiFlags);
}
'@

# Dots for decimals whatever the Windows language (a decimal comma would break the CSV).
[System.Threading.Thread]::CurrentThread.CurrentCulture = [cultureinfo]::InvariantCulture
New-Item -ItemType Directory -Force $OutDir | Out-Null
$file = Join-Path $OutDir ("desktopshepherd-{0:yyyyMMdd-HHmm}.csv" -f (Get-Date))
'time,role,pid,working_set_mb,private_mb,cpu_percent,handles,gdi_objects,user_objects,threads' | Set-Content $file
$cores = [Environment]::ProcessorCount
$last = @{}   # pid -> (cpu ms, time)
$end = if ($Hours -gt 0) { (Get-Date).AddHours($Hours) } else { [datetime]::MaxValue }

while ((Get-Date) -lt $end) {
    $now = Get-Date
    foreach ($w in Get-CimInstance Win32_Process -Filter "Name='desktopshepherd.exe'") {
        $p = Get-Process -Id $w.ProcessId -ErrorAction SilentlyContinue
        if (-not $p) { continue }
        $role = if ($w.CommandLine -match '--worker') { 'worker' } elseif ($w.CommandLine -match '--shell-menu') { 'menu-helper' } else { 'supervisor' }
        if ($role -eq 'menu-helper') { continue }
        $cpuMs = $p.TotalProcessorTime.TotalMilliseconds
        $cpu = ''
        if ($last.ContainsKey($p.Id)) {
            $prev = $last[$p.Id]
            $span = ($now - $prev.Time).TotalMilliseconds
            if ($span -gt 0) { $cpu = [string]::Format([cultureinfo]::InvariantCulture, '{0:F4}', (($cpuMs - $prev.Cpu) / $span / $cores * 100)) }
        }
        $last[$p.Id] = @{ Cpu = $cpuMs; Time = $now }
        $gdi = [GuiRes]::GetGuiResources($p.Handle, 0)
        $usr = [GuiRes]::GetGuiResources($p.Handle, 1)
        [string]::Format([cultureinfo]::InvariantCulture, '{0:yyyy-MM-dd HH:mm:ss},{1},{2},{3:F1},{4:F1},{5},{6},{7},{8},{9}', $now, $role, $p.Id,
            ($p.WorkingSet64 / 1MB), ($p.PrivateMemorySize64 / 1MB), $cpu, $p.HandleCount, $gdi, $usr, $p.Threads.Count) | Add-Content $file
    }
    Start-Sleep -Seconds $IntervalSeconds
}
