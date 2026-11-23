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
3. **No web engine, no telemetry.** Online features are allowed since 2026-11-22 (owner's
   decision), on these terms: only for a feature the person turns on or adds (e.g. agenda links),
   with Windows' own WinHTTP (no extra network crates), on a background thread, never blocking the
   UI, rate-limited and only while the feature is shown, and nothing sent about the person.
4. **Never `cargo update`.** Pinned crates keep the GNU build working (see `docs/BUILDING.md`).
5. Keep the fallback chain working: hardware DirectX + glass, then software DirectX (WARP) +
   glass (`d3d::Renderer::new`), then no glass (plain windows, solid dark background; test with
   env var `DESKTOPSHEPHERD_NO_COMPOSITION=1`). Never add GL, a web engine or eframe back.
6. Saved layout (`model.rs`) must stay backward compatible: new fields use `#[serde(default)]`;
   breaking changes get a migration in `app.rs` (`version` field, see v2/v3).

## Workflow

```powershell
.\scripts\stop.ps1            # asks the worker to quit cleanly (waits up to 5 s, kills only if
                              # needed), then stops the supervisor (exe is locked while running)
cargo build
Start-Process .\target\debug\desktopshepherd.exe
```

- Quick visual check without touching the live desktop:
  `.\target\debug\desktopshepherd.exe --screenshot C:\temp\shot.png` (draws the settings view
  off-screen with the D3D renderer, no window at all, ~4 s, exits by itself; uses the default
  layout, not your saved one).
- Logs: `%APPDATA%\DesktopShepherd\log.txt`. Layout: `%APPDATA%\DesktopShepherd\layout.json` (back it up
  before experiments; backups are in `backups\`).
- Synthetic mouse tests: add waits of ≥150 ms between press/move/release, or egui won't start
  a drag. Display scale is 125 %: from a (DPI-unaware) PowerShell, `SetCursorPos` and
  `GetWindowRect` use scaled coordinates, but `CopyFromScreen` captures real pixels (×1.25).
  That mapping proved unreliable: call `SetProcessDPIAware()` first, then use real pixels
  everywhere (screenshot pixel = cursor position), and check with `GetCursorPos`. Monitors are
  2560×1440 px (2048×1152 points): capture the full 2560×1440, not 2048×1152.
- CPU while hovering: `tools\measure_hover_cpu.ps1`; Win+D: `tools\measure_show_desktop_gap.ps1`
  (start with the desktop not shown).

## Measuring

```powershell
Get-CimInstance Win32_Process -Filter "Name='desktopshepherd.exe'" | % {
  $p = Get-Process -Id $_.ProcessId; $a = $p.TotalProcessorTime.TotalMilliseconds
  Start-Sleep 10; $p.Refresh(); $b = $p.TotalProcessorTime.TotalMilliseconds
  "{0} ws={1:N0}MB cpu={2:N2}%" -f $_.CommandLine, ($p.WorkingSet64/1MB), (($b-$a)/10000/[Environment]::ProcessorCount*100) }
```

Current baseline (2026-11-23, 9 boxes, 2 monitors): worker ~99 MB working set (~80 MB private),
~0.02 % CPU idle with drop shadow and raised look off; ~113 MB working set (~78 MB private),
~0.03 % CPU idle with drop shadow and raised look on for all 9 boxes (shadow memory only exists
for boxes that have it on); supervisor ~2 MB private. (A release build is no smaller, so the debug build, which is opt-level 3, is fine
for daily use.)

## Gotchas learned the hard way

- PowerShell: `"$name"` immediately followed by letters expands to nothing; here-strings
  written to `.rs` files may carry CR characters (breaks doc comments) – strip `` `r ``.
- History: GL transparency never worked on this PC; real glass comes only from the composition
  path. The GL path and eframe are gone (phase C), so this no longer matters.
- There is no main window any more. `src/runloop.rs` is the program's own message loop (hidden
  window `DesktopShepherdMain`, never shown). Other threads wake it with `runloop::wake_after` /
  WM_APP; quit with `runloop::request_quit()` (the stop script reaches it through the named
  event `Local\DesktopShepherd.Quit`, `desktop::watch_quit_signal`).
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
- Icon boxes never take the focus (MA_NOACTIVATE); a left click in one calls
  `desktop::focus_desktop()` so the keyboard goes to the desktop (type to jump / quick search),
  not to whatever had it before (e.g. a sticky note). Keep that when changing box clicks.
- Desktop changes come from `desktop::watch_folders` (change notifications), not polling; the
  reader thread in `app.rs` re-reads only when woken (or once a minute).
- Hotkey ids (`runloop.rs`): 1 search, 2 hide/show, 3/4 pages, 5 draw a box (Ctrl+Alt+B), 21..29 box shortcuts
  (Ctrl+Alt+F2..F10; not Ctrl+Alt+digits = AltGr on many keyboards; F1 is often taken), 100+
  typing keys.
- PowerShell: `FindWindow(class, $null)` passes an empty title, not NULL, so it finds nothing –
  use `[NullString]::Value` or enumerate the windows instead.
- Boxes must never come in front of other programs' windows: box windows refuse z-order changes
  that don't carry SWP_NOACTIVATE (host.rs WM_WINDOWPOSCHANGING); our own restacking
  (desktop::send_to_bottom, shadow::restack) always passes SWP_NOACTIVATE.

## Where to start for common tasks

- Adaptive text (dark ink on bright wallpapers) → `src/backdrop.rs` (wallpaper brightness grid per
  monitor, helper thread) + `src/ink.rs` (palette for text drawn on boxes; `ink::set` per box in
  the box loop of `app.rs`). New text/line colours on boxes must use `ink::` (or `ink::adapt`).
- Every visible text goes through `crate::i18n::t("English")` / `tf("… {} …", &[&x])` with
  entries in the ES and CA tables of `src/i18n.rs` (a test checks both tables match). Dates:
  `i18n::long_date`, `month_year`, `weekday_short`…
- New widget → `src/widgets.rs` (`REGISTRY`). Widget options go in `DeskWidget::settings` (shown in the
  Personalize window's "Widget settings" tab, opened from the box menu): **no buttons or edit
  views on the widgets themselves**, and no right-click menus inside them (the box menu takes the
  right-click). Widgets that need the desktop's items (search box) are
  drawn in `app.rs` (`search_widget`).
- Box look (tint/blur/corners) → `host.rs` `Look` + `HostSystem::apply`; settings UI in
  `app.rs` (Personalize window, `show_personalize`).
- Box look options (colour, border, fonts, text, icon size/spacing) → `Fence` fields in
  `model.rs`, drawing in `fence_view.rs`, Personalize window in `app.rs` (`show_personalize`, columns `look_left` / `look_right`).
- Quick search → `src/search.rs` (ranking, unit tests) + `show_search` in `app.rs`; the shortcut
  is in `runloop.rs` (`SEARCH_HOTKEYS`, `take_search_request`).
- Drop shadow -> `src/shadow.rs`: separate click-through layered window under each box, only exists when `Fence.shadow` is on (synced from the box pass in `app.rs`, restacked in `desktop::send_to_bottom`).
- Raised look (bevel round the inner edge plus sheen, `Fence.raised`) → `fence_view.rs` `paint_raised`, switch in the Personalize window (`show_personalize`); also drawn on glass boxes.
- Windows' right-click menu → `src/shellmenu.rs`.
- Typing on the desktop → `runloop.rs` `set_typing_keys` + `app.rs` `type_ahead`.
- Icon selection/drag/drop → `app.rs` `fence_window`, `ghost::begin_icon_drag`.
- Monitor handling → `app.rs` (bind/relayout), `desktop::monitors`.
- Snapping (incl. optional grid) → `src/snap.rs` (+ `snap_targets` in `app.rs`).
- Releases: `.github/workflows/release.yml` (tag vX.Y.Z = GitHub Release with installer and
  portable zip; built with MSVC on GitHub, so this PC's GNU workarounds don't apply there).
- Installer / uninstaller → `src/install.rs`; build the setup file with `scripts\make-setup.ps1`.
- Pages of boxes → `Fence.page`, `Layout.current_page`, page block in `update_inner` (`app.rs`).
- Light or dark menus and settings → `src/theme.rs` (`Settings.theme`: 0 like Windows, 1 dark,
  2 light; use its palette functions in panel UI, not fixed colours; the panel pass in `host.rs`
  swaps in `theme::panel_visuals`).
- Weather looks and icons → `src/weather.rs` (looks via the widget settings, which may set
  `resize_to` in the widget data) and `assets/meteocons/` (PNG icons, embedded with `include_bytes!`).
- App icon → `assets/icon` (ico built from the icon-1024 cut-out + `icon-small.svg` for ≤24 px), embedded by
  `build.rs` on MSVC (rc.exe); runtime HICONs from the PNGs (`src/appicon.rs`), tray icon from icon-32.png.
- Next suggested work: see "Improvement ideas" in `docs/STATUS.md`.
