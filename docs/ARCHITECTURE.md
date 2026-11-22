# Architecture

## Processes

```
deskboxes.exe            supervisor (main.rs: supervise)
  └─ deskboxes.exe --worker   the real app (own Win32 loop + egui + DirectX windows)
```

- The supervisor claims a single-instance mutex, starts the worker and waits. Whenever the
  worker exits (quit, crash, kill) it restores Windows' desktop icons. Non-zero exit → restart
  with growing waits for failures in a row (2, 5, 15, 30, 60, 120, then every 300 s; a run of
  ≥ 60 s resets them). Only exit code 0 (a real quit) stops the supervisor. A failed start
  (for example composition cannot start at all) exits with code 2 and is retried the same way.
- The worker also has a panic hook that logs and restores icons.
- Log: `%APPDATA%\DeskBoxes\log.txt`. Layout: `%APPDATA%\DeskBoxes\layout.json` (written via
  `.tmp` + rename; rolling copies in `backups\`).

## The program's own loop (`runloop.rs`, since phase C)

There is no eframe, no GL and no main window to draw on any more.

- At start the process is set to per-monitor DPI aware v2 (eframe used to do that).
- A hidden top-level window, class/name `DeskBoxesMain`, is created and **never shown**. It
  receives wake-ups (`WM_APP`) posted by other threads (desktop scan, icon loader, tray...) and
  Windows' shutdown notice (`WM_QUERYENDSESSION` / `WM_ENDSESSION`): on shutdown the app saves
  and puts the desktop icons back.
- The loop sleeps in `MsgWaitForMultipleObjectsEx` until a message arrives or the next due
  pass. Idle = blocked in the wait = 0 % CPU.
- A "root" `egui::Context` is kept only for scheduling: its repaint callback calls
  `runloop::wake_after`. It draws nothing (the one exception is `--screenshot`).
- Quit = `runloop::request_quit()` (exit code 0, so the supervisor stops).

## Windows inside the worker

- **Menus and settings**: `host::Panel` windows (class `DeskBoxesPanel`): plain Win32 windows
  with an ordinary flip swap chain, drawn by the boxes' renderer and egui context
  (`HostSystem::panel`, ids `MENU_PANEL` / `SETTINGS_PANEL` in `app.rs`). Same input mapping
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
  - Moving a box from the app (monitor relayout, "Bring every box back on screen", restoring a
    backup / importing, rolling up from the settings page):
    `DeskApp::place_box` / `place_all_boxes` push `OuterPosition` / `InnerSize` commands to the
    DirectX windows.
  - **GPU fallback**: `d3d::Renderer::new` tries the hardware device first, then
    `D3D_DRIVER_TYPE_WARP` (Windows' software DirectX; logged). WARP is slower but fine for
    small boxes and covers weak GPUs and Remote Desktop. If composition itself cannot start,
    the worker exits with code 2 and the supervisor retries with growing waits. Not done yet
    (rest of phase D): a fallback for "no composition at all" (plain swap chain, no glass).
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
| desktop scan | re-reads desktop folders every `rescan_secs` (default 4) |
| icon loader | SHGetFileInfo/IShellItemImageFactory → textures (cached in `icons.rs`) |
| icon hide/show | toggles the desktop ListView asynchronously (never blocks on Explorer) |
| move helper | `host::move_with_mouse`, alive only during a box drag |
| flying icon | topmost layered window during icon drag (`ghost::begin_icon_drag`) |
| show-desktop guard | asleep except for ~1 s after Win+D (see STATUS) |

Idle the UI thread sleeps: it only wakes on input, a scan change, a widget timer (clock: once
a second) or a wake-up posted by another thread.

## Source files

| File | Contents |
|---|---|
| `main.rs` | arg parsing, supervisor, panic hook, start of the worker |
| `runloop.rs` | the Win32 message loop, hidden `DeskBoxesMain` window, wake-ups, shutdown notice, quit |
| `app.rs` | `DeskApp` state, update pass, boxes UI (`fence_window`), menus, settings page, selection/drag/drop, multi-monitor binding, `place_box` / `place_all_boxes`, layout migrations (v2/v3), `Act` command queue |
| `host.rs` | DirectX/composition box and panel windows, input mapping, look (`Look`), device-lost recovery, `move_with_mouse` |
| `d3d.rs` | D3D11 egui renderer (hardware or WARP device, textures, swap chains, draw, `render_offscreen` for `--screenshot`) |
| `comp_trial.rs` | stand-alone composition diagnostic (`--composition-trial`) |
| `desktop.rs` | Win32 helpers: desktop scan, open item, hide/show icons, autostart Run key, single instance, monitors, work areas, z-order, rounding |
| `fence_view.rs` | sorting/grid layout of items inside a box |
| `ghost.rs` | only the flying icon picture while dragging icons between boxes (`begin_icon_drag` / `destroy`) and `primary_button_down()` |
| `icons.rs` | icon cache |
| `model.rs` | saved data types: `Layout`, `Fence`, `Tab`, `Settings`, `CategoryRule`, widgets config |
| `storage.rs` | load/save/backups/log |
| `widgets.rs` | widget registry (clock, note, sketch) – add new widgets here |

(`wallpaper.rs` and the classic GL drawing path were deleted in phase C.)

## Adding a widget

1. In `widgets.rs`, write a struct implementing `DeskWidget`
   (`fn ui(&self, ui: &mut Ui, data: &mut Value) -> WidgetOutput`).
2. Add one `WidgetKind { id, label, icon (Segoe Fluent glyph), default_size, make }` line to
   `REGISTRY`. It appears in the "New widget" menu automatically. State persists in
   `Fence::widget.data` (free-form JSON); no format change needed.
3. Keep it cheap: set `WidgetOutput::repaint_after` only as often as it really changes
   (`None` = redraw on input only); set `changed` when `data` must be saved.
