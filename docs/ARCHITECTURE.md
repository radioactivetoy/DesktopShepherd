# Architecture

## Processes

```
desktopshepherd.exe            supervisor (main.rs: supervise)
  └─ desktopshepherd.exe --worker   the real app (own Win32 loop + egui + DirectX windows)
```

- The supervisor claims a single-instance mutex, starts the worker and waits. Whenever the
  worker exits (quit, crash, kill) it restores Windows' desktop icons. Non-zero exit → restart
  with growing waits for failures in a row (2, 5, 15, 30, 60, 120, then every 300 s; a run of
  ≥ 60 s resets them). Only exit code 0 (a real quit) stops the supervisor. A failed start
  (no DirectX device at all) exits with code 2 and is retried the same way.
- Clean stop: the worker waits on a named event `Local\DesktopShepherd.Quit`
  (`desktop::watch_quit_signal`, hooked to `runloop::request_quit`). `scripts\stop.ps1` sets
  it, waits up to 5 s and only force-kills if the worker does not leave.
- The worker also has a panic hook that logs and restores icons.
- Log: `%APPDATA%\DesktopShepherd\log.txt`. Layout: `%APPDATA%\DesktopShepherd\layout.json` (written via
  `.tmp` + rename; rolling copies in `backups\`).

## The program's own loop (`runloop.rs`, since phase C)

There is no eframe, no GL and no main window to draw on any more.

- At start the process is set to per-monitor DPI aware v2 (eframe used to do that).
- A hidden top-level window, class/name `DesktopShepherdMain`, is created and **never shown**. It
  receives wake-ups (`WM_APP`) posted by other threads (desktop scan, icon loader, tray...) and
  Windows' shutdown notice (`WM_QUERYENDSESSION` / `WM_ENDSESSION`): on shutdown the app saves
  and puts the desktop icons back.
- The hidden window also handles two more things, both without any hook:
  - `WM_HOTKEY` for the quick-search shortcut (`RegisterHotKey`, choices in
    `runloop::SEARCH_HOTKEYS`, `Settings.search_hotkey`): it only sets a flag, which `app.rs`
    reads with `take_search_request`.
  - Type to jump (`Settings.desktop_typing`: 0 nothing, 1 jump (default), 2 quick search). While
    the desktop itself is the foreground window (`desktop::foreground_is_desktop`: Progman/WorkerW)
    and Windows' icons are hidden, the window borrows the digit, letter and Enter keys with
    `RegisterHotKey` (`runloop::set_typing_keys`, ids 100+). `WM_HOTKEY` pushes the key to a queue
    that `app.rs` reads with `take_typed`. A foreground WinEvent callback wakes the app on every
    foreground change, so it re-checks each pass and gives the keys back as soon as another window
    (including our own settings, menu or search windows) comes forward. Safety: if a borrowed key
    arrives while the desktop is not in front (a race), the keys are released at once and that key
    is re-sent with `keybd_event`, so typing is never lost. Still no hook.
  - The same window is the owner of Windows' own item menu (see `shellmenu.rs`) and forwards
    `WM_INITMENUPOPUP` / `WM_DRAWITEM` / `WM_MEASUREITEM` / `WM_MENUCHAR` to it
    (`main_wndproc` -> `shellmenu::handle_menu_msg`) so the "Send to" and "Open with" submenus fill in.
  - `WM_DISPLAYCHANGE` (monitor switched off/on, resolution change): it sets a flag read with
    `take_display_changed`. The app then re-reads the monitors, re-places the boxes and puts
    every box back just above the desktop at once, after 1.5 s and after 4 s
    (`DeskApp.restack_at`; log line "display change: re-stacking N boxes").
- The loop sleeps in `MsgWaitForMultipleObjectsEx` until a message arrives or the next due
  pass. Idle = blocked in the wait = 0 % CPU.
- A "root" `egui::Context` is kept only for scheduling: its repaint callback calls
  `runloop::wake_after`. It draws nothing (the one exception is `--screenshot`), so it has no
  fonts (`FontDefinitions::empty()`); screenshot mode still gets fonts.
- The settings window can be resized by its edge with live redraw: `host.rs` tracks
  `WM_ENTERSIZEMOVE` / `WM_EXITSIZEMOVE` per panel, and on `WM_SIZE` during that time calls
  `runloop::pass_now()`, which runs one pass right inside the window procedure (a `RefCell`
  `try_borrow` stops it from re-entering).
- Quit = `runloop::request_quit()` (exit code 0, so the supervisor stops).

## Windows inside the worker

- **Menus and settings**: `host::Panel` windows (class `DesktopShepherdPanel`): plain Win32 windows
  with an ordinary flip swap chain, drawn by the boxes' renderer and egui context
  (`HostSystem::panel`, ids `MENU_PANEL` / `SETTINGS_PANEL` / `SEARCH_PANEL` in `app.rs`). The quick
  search box (`show_search`) is such a topmost panel near the top of the main monitor; ranking
  is in `search.rs`. Same input mapping
  as boxes plus clipboard (Ctrl+C/X/V), close button (`WM_CLOSE` → egui close request) and a
  minimum size. The settings page scrolls.
- **Boxes (`host.rs` + `d3d.rs`)**: one plain Win32 window per box
  (`WS_EX_NOREDIRECTIONBITMAP`, no-activate). Content is built with Windows.UI.Composition:
  - `DesktopWindowTarget` → container visual with a rounded geometric clip
  - `HostBackdropBrush` layer (frosted desktop; opacity = blur/40, i.e. a strength
    approximation – the system blur radius is fixed)
  - `CompositionColorBrush` tint layer (box colour × alpha)
  - `SpriteVisual` whose brush is a composition **swap chain**; egui draws the box UI into it
    with our own small D3D11 renderer (`d3d.rs`, HLSL compiled at start-up with D3DCompile,
    premultiplied alpha).
  - All box windows share one egui `Context` (`HostSystem::ctx`), one viewport id per box.
    The wndproc turns Win32 mouse/keyboard messages into egui events, marks the box dirty
    and wakes the loop (grouped: at most one pass per `host::MIN_PASS_GAP`). On each pass
    `HostSystem::frame` draws a box only if it is dirty (input, resize), its look changed, its
    egui timer is due, or the app forces it (`DeskApp::redraw_all`: desktop files, icons,
    layout or shared selection changed; every pass during an icon drag). Skipped boxes run no
    egui pass and present nothing. After each pass the loop re-arms its wake-up with
    `HostSystem::next_due()`. Moving a box does not trigger redraws (only resizing does).
  - Moving a box by mouse: `host::move_with_mouse` runs a helper thread that follows the
    cursor with `SetWindowPos(…SWP_ASYNCWINDOWPOS)`; DWM moves the composed content, so the
    frosted backdrop never flickers.
  - Snapping (`snap.rs`, plain arithmetic with unit tests): within 10 points an edge jumps onto
    (a) another box's edge with a gap between (side by side / stacked), (b) another box's same
    edge (lined up), (c) the monitor's usable area, with the gap inside its edges. Other boxes
    only count when near in the other direction. Alt = no snapping. Settings: `Settings.snap`
    (default on) and `Settings.snap_gap` (default 8 pt, 0-40), both with serde defaults.
    Optional grid: `Settings.snap_grid` (default off) and `grid_size` (default 20 pt); when no
    box or screen edge is within reach, the moved box's top-left / resized edge lands on a
    multiple of the grid (needs snapping on). Moving:
    `host::move_with_mouse(hwnd, grab, Option<Snap>)` snaps in pixels on its helper thread, and
    keeps the exact spot where the box was grabbed (egui's press origin). Resizing: the grip code
    in `app.rs` snaps the right/bottom edge in points. `app.rs` `snap_targets()` gathers the other
    boxes' rectangles (`desktop::window_rect_px`) and the monitors' work areas
    (`desktop::work_areas_px`).
  - Monitor memory: when a move or resize ends, the box's monitor assignment (role index,
    monitor name, offset) is updated at once (`UiState.rebind` on drag stop), so a later monitor
    change cannot send it back to an old monitor.
  - Scales: box positions are always in app screen points (pixels / main display scale, like the
    monitor list and the saved layout); box sizes are in the box's own points (its monitor's
    scale) (`host.rs` `frame()` / `apply_to`, and box creation). Icon drops use the main scale.
    Mixed-scale setups are not tested.
  - Typing: characters outside the basic range (emoji, rare CJK) that arrive as two `WM_CHAR`
    halves are combined; the input method's composition and candidate windows are placed at the
    text cursor (`ImmSetCompositionWindow` / `ImmSetCandidateWindow`, from egui's
    `platform_output.ime`). Copy/paste also works in widgets inside boxes (the note).
  - Moving a box from the app (monitor relayout, "Bring every box back on screen", restoring a
    backup / importing, rolling up from the settings page):
    `DeskApp::place_box` / `place_all_boxes` push `OuterPosition` / `InnerSize` commands to the
    DirectX windows.
  - **GPU fallback**: `d3d::Renderer::new` tries the hardware device first, then
    `D3D_DRIVER_TYPE_WARP` (Windows' software DirectX; logged). WARP is slower but fine for
    small boxes and covers weak GPUs and Remote Desktop.
  - **No-glass fallback**: if Windows' composition cannot start (or env var
    `DESKTOPSHEPHERD_NO_COMPOSITION=1`, for testing), boxes are ordinary windows with their own flip
    swap chain; egui draws a solid dark background and the tint (`HostSystem::has_glass`,
    `Host.glass: Option<Glass>`, `UiState.glass`). Chain: hardware D3D11 + glass, WARP + glass,
    no glass. Only "no D3D device at all" exits with code 2 (the supervisor retries with growing
    waits). WARP and a real "no composition" machine are not testable on this PC; forced
    no-glass mode was tested.
  - Device lost (`DXGI_ERROR_DEVICE_REMOVED/RESET`, e.g. driver update or GPU reset):
    `HostSystem::lost` is set, `app.rs` calls `HostSystem::recover()` (new renderer, fresh
    egui context, box windows recreated); retries every 2 s if it fails.
- Boxes stay behind all windows: `desktop::prepare_box` subclasses them with
  `MA_NOACTIVATE`, and on focus loss they are sent to the bottom. **No hooks into Explorer**
  (the original injects with `SetWindowsHookExW`; we deliberately don't – it can freeze
  Explorer).

## Threads

| Thread | Job |
|---|---|
| main/UI | the loop in `runloop.rs`, egui passes, box frames, D3D rendering |
| folder watch | `desktop::watch_folders`: Windows change notifications on the desktop folders and folder-box folders (asleep otherwise); wakes the desktop reader; every 4 s re-hides Windows' desktop icons if Explorer showed them |
| desktop reader | re-reads the folders when the folder watch or the app wakes it (and once a minute as a safety net); monitors come from WM_DISPLAYCHANGE, not polling |
| short-lived helpers | agenda downloads, drive space (system monitor), picture decoding (picture frame), file pickers, "open file location", paste / Recycle Bin, autostart entry: each on its own thread so the boxes never wait |
| icon loader | SHGetFileInfo/IShellItemImageFactory → textures (cached in `icons.rs`, with a 5 s watchdog that replaces a stuck loader thread); icons are loaded at the size shown in real pixels (`IconCache::get(path, px)`, icon size × display scale), and textures of sizes no longer used are dropped (`IconCache::keep_only_sizes`, called from `app.rs` when the set of sizes changes) |
| icon hide/show | toggles the desktop ListView asynchronously (never blocks on Explorer) |
| move helper | `host::move_with_mouse`, alive only during a box drag |
| flying icon | topmost layered window during icon drag (`ghost::begin_icon_drag(path, px) -> DragImage`); the picture loads its icon on its own thread and can be cancelled with `DragImage::destroy`, so a slow shortcut (network drive) cannot freeze the boxes |
| quit watcher | waits on the named event `Local\DesktopShepherd.Quit` (`desktop::watch_quit_signal`) |
| show-desktop guard | asleep except for ~0.4–1 s after Win+D (stops once the boxes stayed up for 300 ms; see STATUS) |

Idle the UI thread sleeps: it only wakes on input, a scan change, a widget timer (clock: once
a second) or a wake-up posted by another thread.

## Source files

| File | Contents |
|---|---|
| `main.rs` | arg parsing, supervisor, panic hook, start of the worker |
| `runloop.rs` | the Win32 message loop, hidden `DesktopShepherdMain` window, wake-ups, shutdown notice, quit |
| `app.rs` | `DeskApp` state, update pass, boxes UI (`fence_window`), menus, settings page, selection/drag/drop, multi-monitor binding, `place_box` / `place_all_boxes`, layout migrations (v2/v3), `Act` command queue; `type_ahead` (type to jump: open boxes in reading order, all tabs; same letter again = next match; letters within 1 s narrow the prefix; selects, switches tab, scrolls into view via `UiState.scroll_to`; Enter opens the selection within 30 s; mode 2 opens quick search with the typed letter) |
| `shellmenu.rs` | Windows' own context menu for desktop items, shown by a short-lived helper process (`desktopshepherd.exe --shell-menu`): `IShellFolder::GetUIObjectOf(IContextMenu)` -> `QueryContextMenu` -> `TrackPopupMenuEx` -> `InvokeCommand` with `CMINVOKECOMMANDINFOEX`. Our own entries are added on top; the chosen one comes back as the exit code. Shell extensions load only into the helper, never into the app or Explorer. |
| `fileops.rs` | Recycle Bin delete, clipboard (copy/cut as files) and paste into a box (`SHFileOperationW`, undo-able, renames on collision) |
| `install.rs` | installer (any copy named *setup*.exe) and `--uninstall`: per-user install to `%LOCALAPPDATA%ProgramsDesktopShepherd`, Start menu shortcut, Installed apps entry, Run value; stops a running copy through the quit event |
| `host.rs` | DirectX/composition box and panel windows, input mapping, look (`Look`), device-lost recovery, `move_with_mouse` |
| `d3d.rs` | D3D11 egui renderer (hardware or WARP device, textures, swap chains, draw, `render_offscreen` for `--screenshot`) |
| `comp_trial.rs` | stand-alone composition diagnostic (`--composition-trial`) |
| `desktop.rs` | Win32 helpers: desktop scan, open item, hide/show icons, autostart Run key, single instance, monitors, work areas, z-order, rounding |
| `fence_view.rs` | sorting/grid layout of items inside a box; the font list (`FONTS`, `family`, `set_loaded_fonts`): only installed fonts are offered, only fonts in use are loaded (`setup_style(ctx, extra_fonts)` in `app.rs`), each falling back to the default fonts |
| `ghost.rs` | only the flying icon picture while dragging icons between boxes (`begin_icon_drag` returns a `DragImage`; `DragImage::destroy`) and `primary_button_down()` |
| `icons.rs` | icon cache (`get(path, px)`, `keep_only_sizes`); loader watchdog: a load over 5 s is marked failed (placeholder) and a fresh loader thread takes the other pending icons (at most one restart per 5 s) |
| `shadow.rs` | optional drop shadow: one click-through layered window (`DesktopShepherdShadow`) directly below each box with `Fence.shadow` on, picture via UpdateLayeredWindow (freed right after), `sync` (UI pass), `follow` (move thread), `restack`, `set_visible`, `set_faded`, `destroy` (Host drop) |
| `snap.rs` | box snapping arithmetic (pure, 7 unit tests: `cargo test snap`) |
| `search.rs` | quick-search ranking (pure, 4 unit tests): name starts with text, then a word starts with it, then contains it (or file type), then letters in order; 12 results |
| `model.rs` | saved data types: `Layout`, `Fence`, `Tab`, `Settings`, `CategoryRule`, widgets config. Look fields on `Fence` (all with defaults; `sanitize()` repairs odd values): `border_width`, `text_color`, `text_shadow`, `font`, `title_size`, `caption_size`, `show_captions`, `icon_spacing`; `Fence::copy_look_from` backs "Apply this look to all boxes" (`Act::ApplyLookToAll`) |
| `agenda.rs` | the agenda widget's data: WinHTTP download of iCal links (the only network use, background thread, every 30 min while shown), iCal reading incl. repeating events (4 unit tests) |
| `storage.rs` | load/save/backups/log; `data_dir()` moves the old `%APPDATA%DeskBoxes` folder to `%APPDATA%DesktopShepherd` once |
| `theme.rs` | light/dark theme for menus and settings (`Settings.theme`; reads the Windows app mode from `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme`; palette functions bg, bar, menu, card, text, soft, faint, line, hover, warn; `panel_visuals` for the panel pass in `host.rs`) |
| `backdrop.rs` | wallpaper brightness for adaptive text: helper thread (COM `IDesktopWallpaper`, checks file dates every 30 s and on `poke()`), per-monitor 48x27 luminance grid, `brightness(rect_px)`, `generation()` |
| `ink.rs` | text palette for boxes (`set(dark)` per box while it draws; `text`, `strong`, `soft`, `faint`, `line`, `halo`, `adapt`, `gray`, `white_alpha`, `halo_label`) |
| `weather.rs` | weather widget: looks (Compact / Standard / Detailed / adaptive), Meteocons icons from `assets/meteocons/` (MIT, licence file there) embedded as 128 px PNGs |
| `widgets.rs` | widget registry (clock, note, drawing pad, system monitor, calendar, to-do list, agenda, picture frame; the search box is drawn by `search_widget` in `app.rs`) – add new widgets here. The system monitor shares one reading for all monitors (CPU/RAM every 2 s, drives every 60 s) and reads only while shown |

(`wallpaper.rs` and the classic GL drawing path were deleted in phase C.)

## Adding a widget

1. In `widgets.rs`, write a struct implementing `DeskWidget`
   (`fn ui(&self, ui: &mut Ui, data: &mut Value) -> WidgetOutput`).
2. Add one `WidgetKind { id, label, icon (Segoe Fluent glyph), default_size, make }` line to
   `REGISTRY`. It appears in the "New widget" menu automatically. State persists in
   `Fence::widget.data` (free-form JSON); no format change needed.
3. Keep it cheap: set `WidgetOutput::repaint_after` only as often as it really changes
   (`None` = redraw on input only); set `changed` when `data` must be saved.
