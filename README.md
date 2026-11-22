# DesktopShepherd

A lightweight Windows 11 desktop icon organizer: translucent, frosted "boxes" (fences) that sit
on the desktop, behind every window, and hold your desktop files, folders and shortcuts. It is
an original Rust program; its behaviour was modelled on an existing desktop organizer the owner uses
(defaults and formulas are listed in `MECHANICS.md` / `mechanics.json`).

> The executable and data folder are still called **DeskBoxes** (`deskboxes.exe`,
> `%APPDATA%\DeskBoxes`). Renaming is a listed todo in `docs/STATUS.md`.

## Priorities (do not break these)

1. **Stability** – never freeze Explorer, never lose the desktop icons, never lose the layout.
2. **Performance** – ~0 % CPU when idle, low memory, nothing injected into other processes.
3. **Extensibility** – widgets (clock, note, sketch) live in `src/widgets.rs`; new kinds need no
   file-format change.

## Features

- Boxes with title, tabs, roll-up (expand on hover), lock, rename, delete, auto-sort box with
  user-editable category rules (name + extensions).
- Real frosted glass via Windows composition (DirectX): per-box tint colour, opacity,
  blur amount, rounded corners; boxes move smoothly with no flicker. On weak GPUs or Remote
  Desktop it uses Windows' software DirectX (WARP) instead.
- Select one/many icons (click, Ctrl/Shift, rubber band), drag them between boxes with a
  flying icon under the cursor; open, show in folder, sort (manual, name, size, type, date,
  times opened).
- Hides Windows' own desktop icons while running; a supervisor process gives them back after
  any exit or crash and restarts the app after a crash or failed start, waiting longer after
  each failure in a row (2 s up to 5 min).
- Snapping: while you move or resize a box, its edges jump onto other boxes (side by side, or
  lined up) and onto the screen edges, with an adjustable gap (default 8 pt). Hold Alt to place a
  box freely; both options are in Settings > Desktop.
- Multi-monitor: boxes follow a monitor **role** (main/second…) or stay on a **fixed** monitor.
- Tray icon, Windows 11 style menus and settings page, start with Windows, low CPU priority,
  layout backups.
- No network, no web engine, no Explorer hooks.

## Build and run

See `docs/BUILDING.md` for the toolchain quirks (read it first – **never run `cargo update`**).

```powershell
cargo build                      # debug profile is already optimised (opt-level 3)
.\target\debug\deskboxes.exe      # starts the supervisor, which starts the --worker
cargo build --release            # smaller/faster binary in target\release
```

Stop it safely (worker first, then supervisor, so icons are restored):

```powershell
.\scripts\stop.ps1
```

Command-line options: `--screenshot out.png` (draws the settings view off-screen, no
window, saves a PNG, exits; ~4 s), `--worker` (internal), `--boot` (started with Windows: waits longer
for Explorer), `--composition-trial` (diagnostic).

## Documentation

| File | What |
|---|---|
| `docs/ARCHITECTURE.md` | Processes, threads, the render path, every source file |
| `docs/BUILDING.md` | Toolchain, pinned crates, linker stand-ins |
| `docs/STATUS.md` | Measurements, final review results, known issues, ideas/todo |
| `MECHANICS.md`, `mechanics.json` | Values recovered from the original (with addresses) |
| `CLAUDE.md` | Instructions for Claude Code |
| `GEMINI.md` | Instructions for Gemini CLI |

`shots/` holds development screenshots used while tuning the look (safe to delete).
