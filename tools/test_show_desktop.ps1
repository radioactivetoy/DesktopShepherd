# Presses Win+D twice and reports, before / after "Show desktop" / after restoring:
#  - z-order position of every box window and of the desktop windows (Progman, WorkerW with icons)
#  - for each: visible, minimised, DWM-cloaked, always-on-top
# Boxes should be listed ABOVE the desktop window (smaller index) and not cloaked after Win+D.
$src = @'
using System; using System.Runtime.InteropServices; using System.Text; using System.Collections.Generic;
public static class ZW {
 public delegate bool EP(IntPtr h, IntPtr l);
 [DllImport("user32.dll")] public static extern bool EnumWindows(EP f, IntPtr l);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
 [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int i);
 [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern IntPtr FindWindowEx(IntPtr p, IntPtr a, string c, string w);
 [DllImport("user32.dll")] public static extern void keybd_event(byte k, byte s, uint f, UIntPtr e);
 [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);
 public static string Cls(IntPtr h){ var s=new StringBuilder(256); GetClassName(h,s,256); return s.ToString(); }
 static string Flags(IntPtr h){
  int cl=0; DwmGetWindowAttribute(h,14,out cl,4);
  bool top=(GetWindowLong(h,-20)&0x8)!=0;
  return (IsWindowVisible(h)?"vis":"HIDDEN")+(IsIconic(h)?",MIN":"")+(cl!=0?",CLOAKED":"")+(top?",TOPMOST":"");
 }
 public static List<string> Z(uint pid){ var r=new List<string>(); int i=0; int firstBox=-1, desk=-1; EnumWindows((h,l)=>{ uint p; GetWindowThreadProcessId(h,out p); string c=Cls(h);
  bool isBox = p==pid && c=="DesktopShepherdBox";
  bool isDesk = (c=="Progman" || c=="WorkerW") && FindWindowEx(h,IntPtr.Zero,"SHELLDLL_DefView",null)!=IntPtr.Zero;
  bool isProg = c=="Progman";
  if(isBox || isDesk || isProg){ r.Add("  #"+i+" "+c+(isDesk?"(icons)":"")+" ["+Flags(h)+"]"); }
  if(isBox && firstBox<0) firstBox=i; if(isDesk && desk<0) desk=i;
  i++; return true; }, IntPtr.Zero);
  r.Add("  => first box #"+firstBox+", desktop #"+desk+(firstBox>=0 && desk>=0 ? (firstBox<desk?"  (boxes ABOVE desktop: OK)":"  (boxes BELOW desktop: hidden)") : ""));
  return r; }
 public static void WinD(){ keybd_event(0x5B,0,0,UIntPtr.Zero); keybd_event(0x44,0,0,UIntPtr.Zero); keybd_event(0x44,0,2,UIntPtr.Zero); keybd_event(0x5B,0,2,UIntPtr.Zero); }
}
'@
Add-Type $src
$w = (Get-CimInstance Win32_Process -Filter "Name='desktopshepherd.exe'" | Where-Object CommandLine -match worker).ProcessId
if (-not $w) { "DesktopShepherd worker is not running"; exit 1 }
"BEFORE (fg=" + [ZW]::Cls([ZW]::GetForegroundWindow()) + ")"; [ZW]::Z($w)
[ZW]::WinD(); Start-Sleep -Milliseconds 300
"300 ms AFTER WIN+D (fg=" + [ZW]::Cls([ZW]::GetForegroundWindow()) + ")"; [ZW]::Z($w)
Start-Sleep -Milliseconds 1700
"2 s AFTER WIN+D (fg=" + [ZW]::Cls([ZW]::GetForegroundWindow()) + ")"; [ZW]::Z($w)
[ZW]::WinD(); Start-Sleep 2
"AFTER RESTORE (fg=" + [ZW]::Cls([ZW]::GetForegroundWindow()) + ")"; [ZW]::Z($w)
