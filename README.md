# DesktopShepherd

A lightweight Windows 11 desktop icon organizer: translucent, frosted "boxes" (fences) that sit
on the desktop, behind every window, and hold your desktop files, folders and shortcuts. It is
an original Rust program; its behaviour was modelled on an existing desktop organizer the owner uses
(defaults and formulas are listed in `MECHANICS.md` / `mechanics.json`).

> The executable and data folder are still called **DesktopShepherd** (`desktopshepherd.exe`,
> `%APPDATA%\DesktopShepherd`). Renaming is a listed todo in `docs/STATUS.md`.

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
  Optionally also snap to a grid (Settings > Desktop, off by default, 20 pt).
- Quick search: a global shortcut (default Ctrl+Shift+Space, changeable or off in Settings >
  Desktop) opens a search box at the top of the main monitor. It finds any desktop item by name
  (also by file type, or by letters in order, e.g. "vsc" finds Visual Studio Code) and shows the
  box each item is in. Enter opens it, Ctrl+Enter opens its folder, Esc closes. Also in the tray
  menu and the box menu ("Search desktop").
- "More options..." in the icon right-click menu opens Windows' own context menu for the item
  (7-Zip, Defender, OneDrive, Send to, Open with...). With several icons selected, the others in
  the same folder are included. It runs inside DesktopShepherd, never inside Explorer.
- Type to jump: when the desktop itself is in front, typing a letter or digit jumps to the next
  icon that starts with it (same letter again = next one), Enter opens the selected icon. Settings >
  Desktop "Typing on the desktop": does nothing / jumps to the icon (default) / starts a quick search.
- Look options per box (box menu > Personalize…, its own window; font from a list): any colour with opacity, border thickness and
  colour, text colour, text shadow, title and text size, font, names under icons on/off, icon
  size and spacing, and "Apply this look to all boxes".
- Windows' own right-click menu for icons (Win 11 style, all shell extensions), shown by a
  short-lived helper process (`--shell-menu`), so no shell extension ever loads into the app.
- Keyboard inside boxes: arrows move the selection, Enter opens, Delete sends to the Recycle Bin,
  F2 renames, Ctrl+A selects all, Ctrl+C / Ctrl+X / Ctrl+V copy, cut and paste files (pasting into
  a box copies the files to the desktop and puts them in that box).
- Folder boxes (box menu > New > Folder box…): show a folder's contents live, e.g. Downloads.
- Category rules for the auto-sort box: by file type, by name pattern (`*invoice*`) and by age
  ("changed in the last N days").
- Hide / show all boxes with a shortcut (default Ctrl+Alt+D, changeable) or the tray menu.
- Pages of boxes: each box belongs to a page; Ctrl+Alt+PageDown / PageUp or the tray switch
  pages; box menu > "Move to page".
- When icons don't fit a box: scroll (default) or "pages with dots" (box menu > View); the wheel
  or the dots flip pages.
- Remember box positions per monitor setup (laptop alone / docked), Settings > Monitors.
- Create a box or widget anywhere: **Ctrl+Alt+B** (or tray / box menu "Draw a new box…") dims the
  screen, drag a rectangle, then pick Box, Folder box or a widget; it fills that rectangle. "New ▸"
  in a box menu creates it where you right-clicked. The desktop's own right-click menu gets
  "DesktopShepherd ▸" (draw, new box / folder box / widget here; on Windows 11 under "Show more
  options"; Settings > Desktop to turn off). These are plain registry menu entries that start the
  program: nothing runs inside Explorer.
- Settings > Layout: all monitors together or one at a time (dots), labelled by role or display.
- Keyboard shortcut per box (box menu > Keyboard shortcut: Ctrl+Alt+F2..F10) brings the box into
  view (switching page or showing hidden boxes), pressed again rolls it up or down.
- New, renamed and removed desktop files show up within about a second (Windows change
  notifications, no polling).
- Widgets: clock, sticky note, drawing pad, picture frame (PNG/JPEG, fit or fill), system monitor
  (CPU, RAM, drives), calendar, to-do
  list, search box, agenda (online calendars: iCal / .ics links from Google, Outlook…; repeating
  events supported). Three views (month, week, agenda list) switched with the dots at the
  bottom; a click on an event or a day shows its details (time, calendar, place, description). Widget options: box menu > "Widget settings…". Each redraws only when needed (clock: each minute, monitor: every 2 s,
  calendar: at midnight, the others only on input).
- Multi-monitor: boxes follow a monitor **role** (main/second…) or stay on a **fixed** monitor.
- Tray icon, Windows 11 style menus and settings page, start with Windows, low CPU priority,
  layout backups.
- No network (except the agenda widget, which downloads only the calendar links you add to it),
  no web engine, no Explorer hooks.

## Build and run

See `docs/BUILDING.md` for the toolchain quirks (read it first – **never run `cargo update`**).

```powershell
cargo build                      # debug profile is already optimised (opt-level 3)
.\target\debug\desktopshepherd.exe      # starts the supervisor, which starts the --worker
cargo build --release            # smaller/faster binary in target\release
```

Installer (one file, per user, no admin rights): `.\scripts\make-setup.ps1` builds a slim copy
(profile `dist`, no debugging information, ~11 MB) as `dist\DesktopShepherd-Setup.exe`. Any copy
of the program whose file name contains "setup" acts as the installer: it asks, copies itself to
`%LOCALAPPDATA%\Programs\DesktopShepherd\DesktopShepherd.exe`, adds a Start menu shortcut and an
entry in Settings > Apps > Installed apps, turns on start with Windows and starts it. Running it
again updates. Uninstalling (from Installed apps, i.e. `DesktopShepherd.exe --uninstall`) stops
the program, removes everything and asks whether to keep the saved boxes (default: keep).

Stop it safely (worker first, then supervisor, so icons are restored):

```powershell
.\scripts\stop.ps1
```

Command-line options: `--screenshot out.png` (draws the settings view off-screen, no
window, saves a PNG, exits; ~4 s), `--worker` (internal), `--boot` (started with Windows: waits longer
for Explorer), `--composition-trial` (diagnostic), `--uninstall` (remove the installed copy).

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
