# Status (2026-11-22)

## Measurements (this PC, Windows 11 Pro 26340, debug profile = opt-level 3)

| Process | Working set | Private | Threads | CPU idle (10 s, all cores) |
|---|---|---|---|---|
| supervisor `deskboxes.exe` | ~2 MB | 2 MB | – | 0.00 % |
| worker `deskboxes.exe --worker` | ~89 MB | ~75 MB | 34 | ~0.00–0.02 % |

History: classic GL glass path 194–240 MB → DirectX composition path ~156 MB (155 MB private,
30 threads) → after phase C (no eframe/GL) ~110 MB (100 MB private) → after the memory round
(icons loaded at real pixel size, unused icon sizes dropped, no fonts in the root egui context)
~89 MB (75 MB private) (2026-11-22). A release build measured 107 MB / 99 MB private against
110 / 100 for debug before that round: no real difference, so the debug build (opt-level 3)
stays fine for daily use. (The "113 MB with settings open" figure is from before the memory
round and has not been re-measured.)

Mouse moving over a box (`tools\measure_hover_cpu.ps1`, 10 s, desktop shown so the boxes are
really under the mouse): 7.3 s CPU before phase A → 2.6–2.8 s after phase A and B
(2026-11-21) → 0.7–1.75 s after phase C (2026-11-22). (An earlier "8.0 → 1.45 s" was measured with an app window covering the boxes;
the script now shows the desktop first.) Phase A = each DirectX box draws only for its own reasons (input,
move/resize, look change, its egui timer) or when shared data changes (`redraw_all`); passes
started by box input are grouped (at most one per 16 ms, `host::MIN_PASS_GAP`). Phase C
removed eframe and its GL main window and the classic path (own message loop in
`src/runloop.rs`); moving a box no longer causes a burst of redraws (only resizing does).
`DESKBOXES_DEBUG_DRAWS=1` logs box frames drawn and time spent per 300 passes.
Dragging a box: the box follows the cursor within ~1 step; dragged icons track within 1 px.

**Original app for comparison**: only its 11–12 MB launcher was running here (the
person disabled the organizer). The organizer was *not* started for measurement on purpose:
it injects `Native-x64.dll` into Explorer with `SetWindowsHookExW`, so its cost lives inside
`explorer.exe` (estimated a few tens of MB plus its CEF/web engine processes), and running it
alongside ours would fight over the desktop icons. Our app keeps all its cost in its own
processes and touches Explorer only through ordinary window messages.

Where the memory goes (phase C removed the ~45 MB eframe/GL part): mostly the D3D11 device
plus one composition swap chain per box (big boxes on a 125 % display are ~5–7 MB each), the
egui font atlases, and the icon textures.

## Final review (done)

- D3D renderer: all `unwrap`s replaced with errors; device-removed/reset is detected, logged
  once and recovered (`HostSystem::recover`), retried every 2 s on failure.
- Icon cache cleared on recovery so textures are re-uploaded to the new device.
- Supervisor restores desktop icons after any worker exit; panic hook too.
- Layout writes are atomic (tmp + rename) with backups.
- Icon hiding/showing never blocks on Explorer (async).

## Known issues / untested

- Win+D / "Show desktop": fixed. A foreground notification (SetWinEventHook, out-of-context) re-places every box just above the desktop window when it comes forward (`desktop::watch_show_desktop`, `send_to_bottom`). Explorer lifts the desktop 30–100 ms *after* it takes focus, so a guard thread (`show-desktop-guard`, asleep otherwise) checks every 2 ms for 1 s and re-places the boxes the moment the desktop covers them. Covered time per Win+D: ~120 ms before → ~8–12 ms after the guard, ~6.5–7.6 ms after phase C (under one 60 Hz frame). When the desktop already has the focus (e.g. right after clicking it), Win+D lifts it with no focus change; a second out-of-context notice (EVENT_OBJECT_FOCUS on the desktop icon list, found with `tools\probe_show_desktop_events.ps1`) now wakes the guard too (fixed 2026-11-22; before, boxes stayed hidden in that case; covered time there ~10–12 ms). Tests: `tools\test_show_desktop.ps1` (z-order), `tools\measure_show_desktop_gap.ps1` (covered time; start with the desktop *not* shown).
- Blur slider changes the *opacity* of the system host-backdrop blur (fixed radius), so low
  values look like "less frosted" rather than a smaller radius.
- RDP / weak GPUs: the app falls back to Windows' software DirectX (WARP); slower but the same
  look. If Windows composition itself can't start (or `DESKBOXES_NO_COMPOSITION=1`), boxes are
  ordinary windows with a solid dark background and tint, no glass (phase D is done). Fallback
  chain: hardware D3D11 + glass, WARP + glass, no glass; only "no D3D device at all" exits with
  code 2 (supervisor retries). WARP and a real "no composition" machine can't be tested on this
  PC; only the forced no-glass mode was tested (looks right, same memory). The old GL classic
  path is gone.
- Fixed: the settings window now redraws live while its edge is dragged (`WM_ENTERSIZEMOVE` /
  `WM_EXITSIZEMOVE` tracking and `runloop::pass_now()` from the window procedure).
- Fixed: clean stop. `scripts\stop.ps1` sets the named event `Local\DeskBoxes.Quit`, waits up
  to 5 s and only kills if needed (log: "quit signal received", no crash entry).
- Fixed: starting an icon drag no longer loads the icon on the UI thread (the flying picture
  loads it on its own thread), so a slow shortcut can't freeze the boxes.
- Done (2026-11-22), review S7: icon loader watchdog. If one icon load takes over 5 s (e.g. an
  offline network shortcut) it is marked failed (placeholder) and a fresh loader thread takes
  the other pending icons (at most one restart per 5 s). Not tested with a real hang.
- Done (2026-11-22), review S8: box positions are always in app screen points (pixels / main
  display scale), box sizes in the box's own points. Both monitors here are 125 %, so nothing
  changed on this PC; **untested on mixed displays**.
- Done (2026-11-22): international typing. Emoji and rare CJK characters that arrive in two
  halves are combined (they were dropped), and the input method's composition and candidate
  windows are placed at the text cursor. Typing an emoji and a CJK character into a settings
  field works. Real input-method use (e.g. Japanese IME) is **not tested**: none is installed.
- Done (2026-11-22): copy/paste also works in widgets inside boxes (the note).
- Done (2026-11-22): a box keeps the exact spot where it was grabbed while moving.
- Done (2026-11-22): when a move or resize ends, the box's monitor assignment is updated at
  once, so a later monitor change no longer sends it back to its old monitor. Checked: a box
  moved to DISPLAY2 was recorded as monitor 1 / `\\.\DISPLAY2`, and back as monitor 0 / DISPLAY1.
- New (2026-11-22): snapping (`src/snap.rs`, `cargo test snap` passes). Checked live: a box
  dropped about 5 px off snaps into line with the box below; with Alt it stays; moving back
  snaps again. Settings > Desktop: "Snap boxes to each other and to the screen edges" and "gap".
- New (2026-11-22): quick search (`src/search.rs`, window code `show_search` in `app.rs`). A global
  shortcut (`RegisterHotKey` on the hidden window, no hook) opens a search box near the top of the
  main monitor; typing filters all desktop items (12 results with icon and box name), arrows move,
  Enter opens, Ctrl+Enter opens the folder, Esc or a click elsewhere closes, the shortcut toggles.
  Also in the tray menu and box menu ("Search desktop"). Settings > Desktop: Off / Ctrl+Shift+Space
  (default) / Ctrl+Alt+Space / Win+Shift+Space / Ctrl+Alt+F; a warning shows if another program
  already uses the choice. Checked live: Ctrl+Shift+Space opened it with focus, "ste" listed Steam
  and SteamVR with box names, Down moved, Esc closed.
- New (2026-11-22): more look options per box (box menu > Personalize, now scrollable): any
  colour with opacity, border thickness/colour, text colour, text shadow, title size, text size,
  font, names under icons on/off, icon size 16-128, icon spacing (0 = Windows spacing), "Apply this
  look to all boxes". Old layouts still load (new fields have defaults). Checked live: colour
  picker works; switching a box to Bahnschrift redraws at once, saves, and switching back works.
- New (2026-11-22): optional grid snapping (Settings > Desktop "Also snap to a grid", default off,
  20 pt). Unit test added (`snap.rs` now 7 tests).
- Fixed (2026-11-22, owner's report): turning off the primary monitor moved boxes to the new
  primary but left some above app windows. The hidden window now handles `WM_DISPLAYCHANGE`; the app
  re-reads monitors, re-places boxes and re-stacks them just above the desktop at once, after 1.5 s
  and after 4 s. Checked with a simulated display-change message (a box pushed above a test window
  went back under it). A **real monitor switch-off was not tested** here.
- Tests: all 11 unit tests pass (`cargo test`). Worker memory after start ~88 MB.
- Names: exe/data folder still `DeskBoxes`.
- Synthetic drags (SetCursorPos + mouse_event) don't start a drag on a *rolled-up* box's title
  (same before and after phase C); untested by hand.
- Still untested: mixed display scaling and real input-method (IME) use.
- Fixed in phase C: app-driven box moves (monitor relayout, bring back on screen, restore
  backup/import, roll-up from settings) now really move the DirectX windows; the Settings page
  scrolls (its bottom – backups list and Quit button – used to be cut off).

## Improvement ideas (ordered by value)

1. ~~Replace the eframe/glow root with a plain Win32 message loop~~ – done in phase C
   (2026-11-22): 155 → 110 MB. Remaining memory is mostly the D3D device + one composition
   swap chain per box (big boxes on a 125 % display are ~5–7 MB each) + egui font atlases +
   icon textures, so ideas 2 (rest) and 3 are the next memory wins.
2. ~~Memory: load icon textures at `icon_size × pixels_per_point` instead of 2× size~~ – done
   (2026-11-22, `IconCache::get(path, px)`; sizes no longer used are dropped by
   `IconCache::keep_only_sizes`; 110 → 89 MB together with the font change). Still open: free
   textures of items not shown in any box (LRU).
3. Memory: share one swap chain sized per box but drop swap chains of rolled-up boxes.
4. CPU: desktop rescan via `ReadDirectoryChangesW` instead of polling every 4 s.
5. Rename to DesktopShepherd (exe, mutex name, Run key, data folder with migration of
   `%APPDATA%\DeskBoxes`).
6. Features: more widgets (calendar, system monitor, image frame), per-box hotkeys, quick
   (Quick search, snapping to boxes/screen edges and an optional grid are done, 2026-11-22.)
