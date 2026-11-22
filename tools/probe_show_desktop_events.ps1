# Diagnostic: lists which accessibility events (out-of-context WinEvents) Explorer sends while
# "Show desktop" (Win+D) runs, including the case where the desktop already has the focus.
# Presses Win+D twice. Nothing is injected into Explorer.
$src = @'
using System; using System.Runtime.InteropServices; using System.Collections.Generic; using System.Text;
public static class P {
  public delegate void WinEventProc(IntPtr h, uint ev, IntPtr hwnd, int obj, int child, uint thread, uint time);
  [DllImport("user32.dll")] static extern IntPtr SetWinEventHook(uint min, uint max, IntPtr mod, WinEventProc cb, uint pid, uint tid, uint flags);
  [DllImport("user32.dll")] static extern bool UnhookWinEvent(IntPtr h);
  [DllImport("user32.dll")] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] static extern bool PeekMessage(out MSG m, IntPtr h, uint a, uint b, uint r);
  [DllImport("user32.dll")] static extern bool TranslateMessage(ref MSG m);
  [DllImport("user32.dll")] static extern IntPtr DispatchMessage(ref MSG m);
  [DllImport("user32.dll")] static extern void keybd_event(byte k, byte s, uint f, UIntPtr e);
  [StructLayout(LayoutKind.Sequential)] public struct MSG { public IntPtr h; public uint m; public IntPtr w, l; public uint t; public int x, y; }
  static Dictionary<string,int> seen = new Dictionary<string,int>();
  static WinEventProc keep;
  static void Cb(IntPtr h, uint ev, IntPtr hwnd, int obj, int child, uint thread, uint time) {
    var s = new StringBuilder(64); if (hwnd != IntPtr.Zero) GetClassName(hwnd, s, 64);
    string k = "0x" + ev.ToString("X4") + " " + s + " obj=" + obj;
    int n; seen.TryGetValue(k, out n); seen[k] = n + 1;
  }
  static void Pump(int ms) { var end = DateTime.Now.AddMilliseconds(ms); MSG m; while (DateTime.Now < end) { while (PeekMessage(out m, IntPtr.Zero, 0, 0, 1)) { TranslateMessage(ref m); DispatchMessage(ref m); } System.Threading.Thread.Sleep(2); } }
  static void WinD(){ keybd_event(0x5B,0,0,UIntPtr.Zero); keybd_event(0x44,0,0,UIntPtr.Zero); keybd_event(0x44,0,2,UIntPtr.Zero); keybd_event(0x5B,0,2,UIntPtr.Zero); }
  public static string Run(uint pid) {
    keep = Cb; var hk = SetWinEventHook(1, 0x7FFFFFFF, IntPtr.Zero, keep, pid, 0, 0x0000 | 0x0002);
    var sb = new StringBuilder();
    foreach (var label in new[]{"WIN+D #1", "WIN+D #2"}) {
      seen.Clear(); WinD(); Pump(1500);
      sb.AppendLine("== " + label);
      foreach (var kv in seen) sb.AppendLine("  " + kv.Key + "  x" + kv.Value);
    }
    UnhookWinEvent(hk); return sb.ToString();
  }
}
'@
Add-Type $src
$pid_ = (Get-Process explorer | Sort-Object StartTime | Select-Object -First 1).Id
[P]::Run([uint32]$pid_)
