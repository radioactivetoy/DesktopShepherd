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
5. Keep the fallback chain working: hardware DirectX + glass, then software DirectX (WARP) +
   glass (`d3d::Renderer::new`), then no glass (plain windows, solid dark background; test with
   env var `DESKBOXES_NO_COMPOSITION=1`). Never add GL, a web engine or eframe back.
6. Saved layout (`model.rs`) must stay backward compatible: new fields use `#[serde(default)]`;
   breaking changes get a migration in `app.rs` (`version` field, see v2/v3).

## Workflow

```powershell
.\scripts\stop.ps1            # asks the worker to quit cleanly (waits up to 5 s, kills only if
                              # needed), then stops the supervisor (exe is locked while running)
cargo build
Start-Process .\target\debug\deskboxes.exe
```

- Quick visual check without touching the live desktop:
  `.\target\debug\deskboxes.exe --screenshot C:\temp\shot.png` (draws the settings view
  off-screen with the D3D renderer, no window at all, ~4 s, exits by itself; uses the default
  layout, not your saved one).
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

Current baseline: worker ~89 MB working set (~75 MB private), ~0.01 % CPU idle; supervisor
~2 MB private. (A release build is no smaller, so the debug build, which is opt-level 3, is fine
for daily use.)

## Gotchas learned the hard way

- PowerShell: `"$name"` immediately followed by letters expands to nothing; here-strings
  written to `.rs` files may carry CR characters (breaks doc comments) – strip `` `r ``.
- History: GL transparency never worked on this PC; real glass comes only from the composition
  path. The GL path and eframe are gone (phase C), so this no longer matters.
- There is no main window any more. `src/runloop.rs` is the program's own message loop (hidden
  window `DeskBoxesMain`, never shown). Other threads wake it with `runloop::wake_after` /
  WM_APP; quit with `runloop::request_quit()` (the stop script reaches it through the named
  event `Local\DeskBoxes.Quit`, `desktop::watch_quit_signal`).
- App-driven box moves (relayout, bring back on screen, restore backup) must go through
  `DeskApp::place_box` / `place_all_boxes`, otherwise the DirectX windows don't move.
- Moving boxes by re-rendering every frame caused jitter/flashes; boxes move themselves via
  `SetWindowPos` on a helper thread – keep it that way.
- Right-click on empty box area is handled by the selection band (`secondary_clicked`).
- `TrackPopupMenu` (Windows' own menu, `shellmenu.rs`) needs its owner window in front. Our own
  menu panel is closed first, which would hand focus to another app, so `runloop::bring_main_to_front()`
  is called while our menu still has focus (`show_menu` in `app.rs`). Also: any window of ours
  that activates ends "Show desktop" mode, and destroying our foreground panel hands focus to
  another app.
- Type to jump borrows keys with `RegisterHotKey` only while the desktop is in front; give them
  back the moment another window (even our own) comes forward, and re-send a key that arrives
  late (`keybd_event`) so typing is never lost. Still no hooks: `RegisterHotKey` and a foreground
  WinEvent callback only.
- PowerShell: `FindWindow(class, $null)` passes an empty title, not NULL, so it finds nothing –
  use `[NullString]::Value` or enumerate the windows instead.

## Where to start for common tasks

- New widget → `src/widgets.rs` (`REGISTRY`).
- Box look (tint/blur/corners) → `host.rs` `Look` + `HostSystem::apply`; settings UI in
  `app.rs` (Personalize page).
- Box look options (colour, border, fonts, text, icon size/spacing) → `Fence` fields in
  `model.rs`, drawing in `fence_view.rs`, Personalize page in `app.rs` (`fence_menu`,
  `MenuPage::Look`).
- Quick search → `src/search.rs` (ranking, unit tests) + `show_search` in `app.rs`; the shortcut
  is in `runloop.rs` (`SEARCH_HOTKEYS`, `take_search_request`).
- Windows' right-click menu → `src/shellmenu.rs`.
- Typing on the desktop → `runloop.rs` `set_typing_keys` + `app.rs` `type_ahead`.
- Icon selection/drag/drop → `app.rs` `fence_window`, `ghost::begin_icon_drag`.
- Monitor handling → `app.rs` (bind/relayout), `desktop::monitors`.
- Snapping (incl. optional grid) → `src/snap.rs` (+ `snap_targets` in `app.rs`).
- Next suggested work: see "Improvement ideas" in `docs/STATUS.md`.
