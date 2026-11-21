# Status (2026-11-21)

## Measurements (this PC, Windows 11 Pro 26340, debug profile = opt-level 3)

| Process | Working set | Private | CPU idle (10 s, all cores) |
|---|---|---|---|
| supervisor `deskboxes.exe` | 12 MB | 3 MB | 0.00 % |
| worker `deskboxes.exe --worker` | ~156 MB | ~157 MB | ~0.01 % |

History: classic GL glass path 194–240 MB → DirectX composition path ~156–158 MB.

Mouse moving over a box (`tools\measure_hover_cpu.ps1`, 10 s, desktop shown so the boxes are
really under the mouse): 7.3 s CPU before phase A → 2.6–2.8 s after phase A and B
(2026-11-21). (An earlier "8.0 → 1.45 s" was measured with an app window covering the boxes;
the script now shows the desktop first.) Phase A = each DirectX box draws only for its own reasons (input,
move/resize, look change, its egui timer) or when shared data changes (`redraw_all`); passes
started by box input are grouped (at most one per 16 ms, `host::MIN_PASS_GAP`); the invisible
GL main window is no longer see-through (`--transparent` only for the classic path) and has
vsync off. What is left per pass is mostly eframe's GL main window (phase C removes it).
`DESKBOXES_DEBUG_DRAWS=1` logs box frames drawn and time spent per 300 passes.
Dragging a box: the box follows the cursor within ~1 step; dragged icons track within 1 px.

**Original app for comparison**: only its 11–12 MB launcher was running here (the
person disabled the organizer). The organizer was *not* started for measurement on purpose:
it injects `Native-x64.dll` into Explorer with `SetWindowsHookExW`, so its cost lives inside
`explorer.exe` (estimated a few tens of MB plus its CEF/web engine processes), and running it
alongside ours would fight over the desktop icons. Our app keeps all its cost in its own
processes and touches Explorer only through ordinary window messages.

Where the memory goes: most of the worker's ~156 MB is the eframe/glow root context + GL
driver (~90 MB baseline even for a 1×1 window), then D3D11 device + composition swap chains
(one per box), egui font atlas, and icon textures.

## Final review (done)

- D3D renderer: all `unwrap`s replaced with errors; device-removed/reset is detected, logged
  once and recovered (`HostSystem::recover`), retried every 2 s on failure.
- Icon cache cleared on recovery so textures are re-uploaded to the new device.
- Supervisor restores desktop icons after any worker exit; panic hook too.
- Layout writes are atomic (tmp + rename) with backups.
- Icon hiding/showing never blocks on Explorer (async).

## Known issues / untested

- Win+D / "Show desktop": fixed. A foreground notification (SetWinEventHook, out-of-context) re-places every box just above the desktop window when it comes forward (`desktop::watch_show_desktop`, `send_to_bottom`). Explorer lifts the desktop 30–100 ms *after* it takes focus, so a guard thread (`show-desktop-guard`, asleep otherwise) checks every 2 ms for 1 s and re-places the boxes the moment the desktop covers them. Covered time per Win+D: ~120 ms before → ~8–12 ms now (under one 60 Hz frame). Tests: `tools\test_show_desktop.ps1` (z-order), `tools\measure_show_desktop_gap.ps1` (covered time; start with the desktop *not* shown).
- Blur slider changes the *opacity* of the system host-backdrop blur (fixed radius), so low
  values look like "less frosted" rather than a smaller radius.
- RDP / very old GPUs fall back to the classic path, which is heavier (~200 MB) and uses
  fake glass from a wallpaper capture.
- Names: exe/data folder still `DeskBoxes`.
- Synthetic drags (SetCursorPos + mouse_event) don't start a drag on a *rolled-up* box's title
  (same before and after phase A); untested by hand.

## Improvement ideas (ordered by value)

1. Memory: replace the eframe/glow root with a plain Win32 message loop driving only the
   D3D path (would drop the ~90 MB GL baseline; biggest single win). Keep glow only for the
   classic fallback, created lazily.
2. Memory: load icon textures at `icon_size × pixels_per_point` instead of 2× size; free
   textures of items not shown in any box (LRU).
3. Memory: share one swap chain sized per box but drop swap chains of rolled-up boxes.
4. CPU: desktop rescan via `ReadDirectoryChangesW` instead of polling every 4 s.
5. Rename to DesktopShepherd (exe, mutex name, Run key, data folder with migration of
   `%APPDATA%\DeskBoxes`).
6. Features: more widgets (calendar, system monitor, image frame), per-box hotkeys, quick
   search, snapping boxes to a grid.
