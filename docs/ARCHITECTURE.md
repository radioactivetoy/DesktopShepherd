# Architecture

## Processes

```
deskboxes.exe            supervisor (main.rs: supervise)
  └─ deskboxes.exe --worker   the real app (eframe/egui + DirectX boxes)
```

- The supervisor claims a single-instance mutex, starts the worker and waits. Whenever the
  worker exits (quit, crash, kill) it restores Windows' desktop icons. Non-zero exit → restart
  after 2 s; 3 crashes within 60 s → give up (icons restored, logged).
- The worker also has a panic hook that logs and restores icons.
- Log: `%APPDATA%\DeskBoxes\log.txt`. Layout: `%APPDATA%\DeskBoxes\layout.json` (written via
  `.tmp` + rename; rolling copies in `backups\`).

## Windows inside the worker

- **Root window**: eframe/glow, 1×1, invisible, click-through, bottom-most. It owns the single
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
    The wndproc turns Win32 mouse/keyboard messages into egui events and wakes the root
    context; every root update runs every box frame (only boxes with changes repaint).
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
