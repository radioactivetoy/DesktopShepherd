# New base: design (step 4 of the review plan)

Goal: remove the eframe/GL main window and the classic drawing path. One Win32 loop drives
everything; every window is drawn by the existing D3D11 renderer; each window redraws only
when it has a reason to.

Expected results: worker memory ~156 MB → roughly 60–70 MB (no GL driver/context); no more GL
start failures; mouse-over CPU divided by about the number of boxes.

## Pieces

### 1. Surfaces (generalise `host::Host`)
Every window we draw is a `Surface`: HWND + swap chain + egui viewport id + schedule. There are
three kinds:

| Kind | Window | Glass | Z-order / focus |
|---|---|---|---|
| Box | `WS_POPUP`, `WS_EX_TOOLWINDOW` + `NOREDIRECTIONBITMAP` | composition backdrop + tint (today's) | bottom, no-activate (today's) |
| Menu | `WS_POPUP`, topmost | solid, rounded (DWM) | activates; closes on focus loss or Esc |
| Settings | normal captioned window, resizable, in taskbar | solid | normal app window |

The wndproc → egui event mapping (`host.rs`) is shared by all kinds. Settings needs a little more:
- caption, close button, min size (`WM_GETMINMAXINFO`), resizing (DefWindowProc does it);
- clipboard copy/paste (`platform_output` → Win32 clipboard; eframe did this via arboard);
- more keys (`key_of`).

### 2. Schedule per surface
Each surface keeps `dirty: bool` and `next: Option<Instant>`. A surface draws only when:
- it got input (wndproc sets `dirty`);
- its `next` time has come (from egui's `viewport_output[vid].repaint_delay`);
- shared data changed: a new scan, an icon loaded, a layout edit, a look change (app marks all
  affected boxes);
- it was resized or moved.

Composition properties (size, corners, tint, blur) are set only when they change.

### 3. The loop (replaces `eframe::run_native`)
- At start: `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`. eframe/winit did this
  for us before.
- One hidden message-only window (`HWND_MESSAGE`) receives wake-ups from threads (scan, icons,
  tray, ghost) via `PostMessage`. This replaces `ctx.request_repaint()` from threads.
- Loop: `MsgWaitForMultipleObjectsEx` with timeout = the earliest surface `next`. Then pump
  messages, run `DeskApp::tick()` (scan results, actions, saving), and draw the dirty or due
  surfaces. Idle = blocked in the wait = 0 % CPU.
- Quit = `PostQuitMessage` (exit code 0); the panic hook and the supervisor are unchanged.
- The tray (`tray-icon` crate) works with any Win32 message loop on the same thread.

### 4. GPU fallback chain (replaces the classic GL path)
1. Hardware D3D11 + composition glass (today's path).
2. Composition works but the hardware device fails (old GPU, some RDP setups): use the WARP
   (software) D3D11 device. Same code, slower but fine for small boxes.
3. Composition itself unavailable: a plain flip swap chain on the HWND, with a solid tint and no
   glass. Boxes still work.

After this, `wallpaper.rs`, the classic parts of `ghost.rs` (see-through copies, black/white
snapshots) and the classic viewport code in `app.rs` are deleted. The flying-icon window
(`begin_icon_drag`) stays.

### 5. Dependencies
- Replace `eframe` with `egui` (same 0.31 version that is already in `Cargo.lock`, so no
  version moves). Check the `Cargo.lock` diff: crates may only be **removed** (winit, glutin,
  glow, arboard…). See `docs/BUILDING.md`, and never run `cargo update`.
- Update hard rule 5 in CLAUDE.md / GEMINI.md ("keep the classic GL path") to the new fallback
  chain.

## Phases (each one builds, runs, gets measured and committed on its own)

| Phase | Change | eframe still there? | Check |
|---|---|---|---|
| A ✅ | Per-surface schedule for boxes (section 2) inside today's loop. **Done 2026-11-21**: hover CPU 7.3 → 2.8 s per 10 s (measured with the desktop shown). The main window re-arms its wake-up from `HostSystem::next_due()` after every pass, because egui forgets a pending wake-up when another pass runs in between (the clock stopped without this). | yes | CPU with mouse moving over a box; drag, resize, roll-up, widgets, clock |
| B ✅ | Menus and Settings become D3D surfaces (section 1). **Done 2026-11-21**: `host::Panel` (plain window + flip swap chain, same renderer and egui context as boxes), clipboard copy/paste, close button, min size, dark title bar, rounded menu. The GL viewports stay only for the classic path. Not yet: live repaint *while* the settings window is being resized (the modal sizing loop blocks eframe's loop; fixed by phase C), IME. | yes (root only) | every menu page, settings pages, typing, copy/paste, layout preview |
| C | Own loop (section 3); eframe, glow and the classic path removed | no | idle CPU, memory, tray, quit, crash → supervisor, sleep/resume, device lost, multi-monitor |
| D | WARP and no-composition fallbacks (section 4) | no | force each fallback with a debug switch |

A stays useful even if B–D are delayed: it is the biggest CPU win, and it doesn't depend on
removing eframe.

## Risks and how they're handled
- **Losing eframe conveniences** (clipboard, IME, cursor icons, DPI): each one is listed above
  and gets checked in its phase.
- **A regression on the live desktop:** each phase runs from `target\debug` with the layout backed
  up first. The previous commit stays one `git checkout` away.
- **Explorer safety:** nothing here touches Explorer differently. The same no-hook, no-blocking
  rules apply.
