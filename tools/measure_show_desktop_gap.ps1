# Presses Win+D, then samples the window stack every ~1 ms for 800 ms and reports when the
# desktop (Progman) covered the boxes and when the boxes came back above it. Presses Win+D
# again at the end to restore. Run while DeskBoxes is running.
$src = @'
using System; using System.Runtime.InteropServices; using System.Text; using System.Diagnostics; using System.Collections.Generic;
public static class G {
 public delegate bool EP(IntPtr h, IntPtr l);
 [DllImport("user32.dll")] static extern bool EnumWindows(EP f, IntPtr l);
 [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
 [DllImport("user32.dll")] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
 [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] static extern void keybd_event(byte k, byte s, uint f, UIntPtr e);
 static string Cls(IntPtr h){ var s=new StringBuilder(64); GetClassName(h,s,64); return s.ToString(); }
 // 1 = every visible box above Progman, 0 = at least one box below it
 static int State(uint pid){ bool seenProg=false; bool bad=false; EnumWindows((h,l)=>{ string c=Cls(h);
   if(c=="Progman"){ seenProg=true; return true; }
   if(seenProg && IsWindowVisible(h)){ uint p; GetWindowThreadProcessId(h,out p); if(p==pid && c=="DeskBoxesBox"){ bad=true; return false; } }
   return true; }, IntPtr.Zero); return bad?0:1; }
 public static void WinD(){ keybd_event(0x5B,0,0,UIntPtr.Zero); keybd_event(0x44,0,0,UIntPtr.Zero); keybd_event(0x44,0,2,UIntPtr.Zero); keybd_event(0x5B,0,2,UIntPtr.Zero); }
 public static string Run(uint pid){
  var sw=Stopwatch.StartNew(); WinD(); int last=State(pid); var log=new List<string>(); log.Add("0.0 ms: "+(last==1?"boxes visible":"boxes COVERED"));
  double covered=0, since=-1;
  while(sw.Elapsed.TotalMilliseconds<800){ int s=State(pid); double t=sw.Elapsed.TotalMilliseconds;
   if(s!=last){ log.Add(t.ToString("F1")+" ms: "+(s==1?"boxes visible":"boxes COVERED")); if(s==0) since=t; else if(since>=0){ covered+=t-since; since=-1; } last=s; } }
  if(since>=0) covered+=800-since;
  System.Threading.Thread.Sleep(1200); WinD();
  return string.Join("\n",log)+"\nTotal time covered: "+covered.ToString("F1")+" ms";
 }
}
'@
Add-Type $src
$w = (Get-CimInstance Win32_Process -Filter "Name='deskboxes.exe'" | Where-Object CommandLine -match worker).ProcessId
if (-not $w) { "DeskBoxes worker is not running"; exit 1 }
for ($i = 1; $i -le 3; $i++) { "--- run $i"; [G]::Run($w); Start-Sleep 2 }
