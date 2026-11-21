# Architecture

## Processes

```
deskboxes.exe            supervisor (main.rs: supervise)
  └─ deskboxes.exe --worker   the real app (eframe/egui + DirectX boxes)
```

- The supervisor claims a single-instance mutex, starts the worker and waits. Whenever the
  worker exits (quit, crash, kill) it restores Windows' desktop icons. Non-zero exit → restart
  with growing waits for failures in a row (2, 5, 15, 30, 60, 120, then every 300 s; a run of
  ≥ 60 s resets them). Only exit code 0 (a real quit) stops the supervisor. A failed start
  exits with code 2; code 3 means "GL had no see-through setup" and the supervisor restarts at
  once with `--opaque` (main window not see-through).
- The worker also has a panic hook that logs and restores icons.
- Log: `%APPDATA%\DeskBoxes\log.txt`. Layout: `%APPDATA%\DeskBoxes\layout.json` (written via
  `.tmp` + rename; rolling copies in `backups\`).

## Windows inside the worker

- **Menus and settings (DirectX path)**: `host::Panel` windows (class `DeskBoxesPanel`):
  plain Win32 windows with an ordinary flip swap chain, drawn by the boxes' renderer and egui
  context (`HostSystem::panel`, ids `MENU_PANEL` / `SETTINGS_PANEL` in `app.rs`). Same input
  mapping as boxes plus clipboard (Ctrl+C/X/V), close button (`WM_CLOSE` → egui close
  request) and a minimum size. With the classic path they are still eframe GL viewports.
- **Root window**: eframe/glow, 1×1, invisible, click-through, bottom-most, vsync off, not
  see-through by default. If DirectX boxes can't start, the worker exits with code 4 and the
  supervisor restarts it with `--transparent` (classic boxes need a see-through GL setup); if
  that GL setup is missing it falls back to `--opaque`. It owns the single
  update loop (`DeskApp::update` in `app.rs`). The settings/menu page is an egui immediate
  viewport.
- **Boxes – DirectX path (default, `host.rs` + `d3d.rs`)**: one plain Win32 window per box
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
    and wakes the root context (grouped: at most one pass per `host::MIN_PASS_GAP`). On each
    root update `HostSystem::frame` draws a box only if it is dirty (input, move, resize),
    its look changed, its egui timer is due, or the app forces it (`DeskApp::redraw_all`:
    desktop files, icons, layout or shared selection changed; every pass during an icon
    drag). Skipped boxes run no egui pass and present nothing. After each pass the root
    re-arms its wake-up with `HostSystem::next_due()`.
  - Moving a box: `host::move_with_mouse` runs a helper thread that follows the cursor with
    `SetWindowPos(…SWP_ASYNCWINDOWPOS)`; DWM moves the composed content, so the frosted
    backdrop never flickers.
  - Device lost (`DXGI_ERROR_DEVICE_REMOVED/RESET`, e.g. driver update or GPU reset):
    `HostSystem::lost` is set, `app.rs` calls `HostSystem::recover()` (new renderer, fresh
    egui context, box windows recreated); retries every 2 s if it fails.
- **Boxes – classic fallback (GL viewports)**: used when composition cannot start. Fake glass
  from a captured wallpaper (`wallpaper.rs`, PrintWindow of Progman, half-size blur, per-box
  crops) and layered "ghost" copies while dragging (`ghost.rs`). Keep it working; it is the
  safety net for old GPUs / RDP.
- Boxes stay behind all windows: `desktop::prepare_box` subclasses them with
  `MA_NOACTIVATE`, and on focus loss they are sent to the bottom. **No hooks into Explorer**
  (the original injects with `SetWindowsHookExW`; we deliberately don't – it can freeze
  Explorer).

## Threads

| Thread | Job |
|---|---|
| main/UI | egui update loop, box frames, D3D rendering |
| desktop scan | re-reads desktop folders every `rescan_secs` (default 4) |
| icon loader | SHGetFileInfo/IShellItemImageFactory → textures (cached in `icons.rs`) |
| icon hide/show | toggles the desktop ListView asynchronously (never blocks on Explorer) |
| move helper | `move_with_mouse`, alive only during a drag |
| ghost/flying icon | topmost layered window during icon drag (`ghost::begin_icon_drag`) |

Idle the UI thread sleeps: egui only repaints on input, a scan change, a widget timer
(clock: once a second) or a wake from a box wndproc.

## Source files

| File | Contents |
|---|---|
| `main.rs` | arg parsing, supervisor, panic hook, eframe options |
| `app.rs` | `DeskApp` state, update loop, boxes UI (`fence_window`), menus, settings page, selection/drag/drop, multi-monitor binding, layout migrations (v2/v3), `Act` command queue |
| `host.rs` | DirectX/composition box windows, input mapping, look (`Look`), device-lost recovery, `move_with_mouse` |
| `d3d.rs` | D3D11 egui renderer (textures, swap chains, draw) |
| `comp_trial.rs` | stand-alone composition diagnostic (`--composition-trial`) |
| `desktop.rs` | Win32 helpers: desktop scan, open item, hide/show icons, autostart Run key, single instance, monitors, work areas, z-order, rounding |
| `fence_view.rs` | sorting/grid layout of items inside a box |
| `ghost.rs` | classic drag copies and the flying icon window |
| `icons.rs` | icon cache |
| `model.rs` | saved data types: `Layout`, `Fence`, `Tab`, `Settings`, `CategoryRule`, widgets config |
| `storage.rs` | load/save/backups/log |
| `wallpaper.rs` | classic-path wallpaper capture and blur |
| `widgets.rs` | widget registry (clock, note, sketch) – add new widgets here |

## Adding a widget

1. In `widgets.rs`, write a struct implementing `DeskWidget`
   (`fn ui(&self, ui: &mut Ui, data: &mut Value) -> WidgetOutput`).
2. Add one `WidgetKind { id, label, icon (Segoe Fluent glyph), default_size, make }` line to
   `REGISTRY`. It appears in the "New widget" menu automatically. State persists in
   `Fence::widget.data` (free-form JSON); no format change needed.
3. Keep it cheap: set `WidgetOutput::repaint_after` only as often as it really changes
   (`None` = redraw on input only); set `changed` when `data` must be saved.
