# DesktopShepherd – instructions for AI coding agents

Read `README.md`, then `docs/ARCHITECTURE.md`, `docs/BUILDING.md` and `docs/STATUS.md` before
changing anything. The owner does not code: explain results in plain words, no jargon.

## Hard rules

1. **Stability first.** The app runs all day on the desktop. Never:
   - inject into or hook Explorer (`SetWindowsHookEx`, DLL injection, global hooks);
   - send blocking (`SendMessage` without timeout) calls to Explorer windows – use
     `PostMessage`/`SendMessageTimeout` or a helper thread;
   - `unwrap()`/`expect()` on anything that can fail at runtime (Win32, D3D, files) – return
     errors and fall back;
   - leave the desktop icons hidden: any new exit path must go through the supervisor or call
     `desktop::restore_desktop_icons()`.
2. **Performance.** Idle CPU must stay ~0 %. Don't add per-frame polling or continuous
   repaints; repaint only on input, data changes or a widget's `repaint_after`. Measure memory
   and CPU before/after big changes (see "Measuring").
3. **No network, no web engine.** No telemetry, no online features.
4. **Never `cargo update`.** Pinned crates keep the GNU build working (see `docs/BUILDING.md`).
5. Keep the classic (GL) fallback path compiling and working.
6. Saved layout (`model.rs`) must stay backward compatible: new fields use `#[serde(default)]`;
   breaking changes get a migration in `app.rs` (`version` field, see v2/v3).

## Workflow

```powershell
.\scripts\stop.ps1            # stop worker then supervisor (exe is locked while running)
cargo build
Start-Process .\target\debug\deskboxes.exe
```

- Quick visual check without touching the live desktop:
  `.\target\debug\deskboxes.exe --screenshot C:\temp\shot.png` (window opens behind others,
  no focus, exits by itself).
- Logs: `%APPDATA%\DeskBoxes\log.txt`. Layout: `%APPDATA%\DeskBoxes\layout.json` (back it up
  before experiments; backups are in `backups\`).
- Synthetic mouse tests: add waits of ≥150 ms between press/move/release, or egui won't start
  a drag. Display scale is 125 %: from a (DPI-unaware) PowerShell, `SetCursorPos` and
  `GetWindowRect` use scaled coordinates, but `CopyFromScreen` captures real pixels (×1.25).
- CPU while hovering: `tools\measure_hover_cpu.ps1`; Win+D: `tools\measure_show_desktop_gap.ps1`
  (start with the desktop not shown).

## Measuring

```powershell
Get-CimInstance Win32_Process -Filter "Name='deskboxes.exe'" | % {
  $p = Get-Process -Id $_.ProcessId; $a = $p.TotalProcessorTime.TotalMilliseconds
  Start-Sleep 10; $p.Refresh(); $b = $p.TotalProcessorTime.TotalMilliseconds
  "{0} ws={1:N0}MB cpu={2:N2}%" -f $_.CommandLine, ($p.WorkingSet64/1MB), (($b-$a)/10000/[Environment]::ProcessorCount*100) }
```

Current baseline: worker ~156 MB, ~0.01 % CPU idle; supervisor 12 MB.

## Gotchas learned the hard way

- PowerShell: `"$name"` immediately followed by letters expands to nothing; here-strings
  written to `.rs` files may carry CR characters (breaks doc comments) – strip `` `r ``.
- GL transparency does not work on this PC; real glass comes only from the composition path.
- egui child-viewport screenshots never arrive; `--screenshot` draws in the root instead.
- Moving boxes by re-rendering every frame caused jitter/flashes; boxes move themselves via
  `SetWindowPos` on a helper thread – keep it that way.
- Right-click on empty box area is handled by the selection band (`secondary_clicked`).

## Where to start for common tasks

- New widget → `src/widgets.rs` (`REGISTRY`).
- Box look (tint/blur/corners) → `host.rs` `Look` + `HostSystem::apply`; settings UI in
  `app.rs` (Personalize page).
- Icon selection/drag/drop → `app.rs` `fence_window`, `ghost::begin_icon_drag`.
- Monitor handling → `app.rs` (bind/relayout), `desktop::monitors`.
- Next suggested work: see "Improvement ideas" in `docs/STATUS.md`.
