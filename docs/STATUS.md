# Status (1.0.2, 2026-11-25)

Current version: **1.0.2** (1.0.0 was the first stable release; tags v0.2.0-beta.1..5, v1.0.0-rc.1..3 before it).
v1.0.0-rc.1, rc.2 and rc.3. The workflow (read-only build job + separate publish job, actions
pinned to SHAs, Rust 1.100.0) publishes the Setup exe, the Portable zip and `SHA256SUMS.txt`, and
marks beta and rc tags as pre-releases. Release notes live in `docs/release-notes/<tag>.md`; after
the workflow finishes they are put on the release page and the release is made "Latest" by hand so
it shows on the repo main page:
`gh release edit <tag> --title "DesktopShepherd 1.0 RC N" --notes-file docs/release-notes/<tag>.md --prerelease=false --latest`.

Added since rc.1 (all in rc.2/rc.3): world clock (clock widget); network speed (system monitor,
`GetIfTable2`, graph); Now playing volume slider + mute (hover), seek on the progress bar, and
"Sound output: <device>" in the box menu (`src/audio.rs`); Bluetooth device batteries in Laptop
status (every 5 min, WinRT device enumeration); languages FR/DE/IT/PT; pause for full-screen
apps / screen off or locked; maximized windows pause drawing on their monitor; version next to the
name in Settings; silent setup (`--silent`) and silent uninstall (`--uninstall --silent`, keeps
boxes, `QuietUninstallString`); supervisor watchdog; drop shadow and raised look; adaptive text
recheck after hidden wallpaper changes; duplicate-item protection (dedupe on save, logged); the
update check is opt-in (asked once) and the updater checks the SHA-256 against `SHA256SUMS.txt` and
only accepts this repo's release assets.

## Measurements (this PC, Windows 11 Pro 26340, debug profile = opt-level 3)

**Latest (2026-11-24, after the memory-leak fix, 9 boxes, 2 monitors, music playing):** worker
private memory ~93-97 MB, flat for hours (rises and falls, no trend); working set ~126-129 MB; CPU
~0.05 % with music playing (progress bar redraw), ~0.03 % without, ~0.009 % while a full-screen
game paused the boxes; supervisor ~2 MB.

Leak story: Now playing created a new thread and a new WinRT media manager every 2 s, which grew
memory by ~4.6 MB/h (+26 MB in under 5 h). Fix: one long-lived helper thread with one manager.
The same pattern is applied through the shared `Poller` in `widgets.rs` (laptop Wi-Fi/Bluetooth
every 30 s, drive space every 60 s): it runs only while a widget asks and sleeps otherwise. Rule
for new code: never a new thread or new COM/WinRT object per periodic reading. Tools:
`tools\profile_over_time.ps1` (memory, CPU, handles, GDI/USER objects, threads every N s to
`profile\*.csv`) and `tools\leak_test.ps1` (see CLAUDE.md "Measuring").

Older table, measured 2026-11-23 with 9 boxes on 2 monitors (before the new widgets):

| Process | Working set | Private | Threads | CPU idle (10 s, all cores) |
|---|---|---|---|---|
| supervisor `desktopshepherd.exe` | ~2 MB | 2 MB | – | 0.00 % |
| worker `desktopshepherd.exe --worker`, drop shadow and raised look off | ~99 MB | ~80 MB | – | ~0.02 % |
| worker, drop shadow and raised look on for all 9 boxes | ~113 MB | ~78 MB | – | ~0.03 % |

Shadow memory only exists for boxes that have the shadow on.

History: classic GL glass path 194–240 MB → DirectX composition path ~156 MB (155 MB private,
30 threads) → after phase C (no eframe/GL) ~110 MB (100 MB private) → after the memory round
(icons loaded at real pixel size, unused icon sizes dropped, no fonts in the root egui context)
~89 MB (75 MB private) (2026-11-22). The later features (to-do redesign, weather, shadow, raised
look and more) brought it to the 2026-11-23 numbers above. A release build measured 107 MB / 99 MB
private against 110 / 100 for debug before the memory round: no real difference, so the debug
build (opt-level 3) stays fine for daily use.

Mouse moving over a box (`tools\measure_hover_cpu.ps1`, 10 s, desktop shown so the boxes are
really under the mouse): 7.3 s CPU before phase A → 2.6–2.8 s after phase A and B
(2026-11-21) → 0.7–1.75 s after phase C (2026-11-22). (An earlier "8.0 → 1.45 s" was measured with an app window covering the boxes;
the script now shows the desktop first.) Phase A = each DirectX box draws only for its own reasons (input,
move/resize, look change, its egui timer) or when shared data changes (`redraw_all`); passes
started by box input are grouped (at most one per 16 ms, `host::MIN_PASS_GAP`). Phase C
removed eframe and its GL main window and the classic path (own message loop in
`src/runloop.rs`); moving a box no longer causes a burst of redraws (only resizing does).
`DESKTOPSHEPHERD_DEBUG_DRAWS=1` logs box frames drawn and time spent per 300 passes.
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
  look. If Windows composition itself can't start (or `DESKTOPSHEPHERD_NO_COMPOSITION=1`), boxes are
  ordinary windows with a solid dark background and tint, no glass (phase D is done). Fallback
  chain: hardware D3D11 + glass, WARP + glass, no glass; only "no D3D device at all" exits with
  code 2 (supervisor retries). WARP and a real "no composition" machine can't be tested on this
  PC; only the forced no-glass mode was tested (looks right, same memory). The old GL classic
  path is gone.
- Fixed: the settings window now redraws live while its edge is dragged (`WM_ENTERSIZEMOVE` /
  `WM_EXITSIZEMOVE` tracking and `runloop::pass_now()` from the window procedure).
- Fixed: clean stop. `scripts\stop.ps1` sets the named event `Local\DesktopShepherd.Quit`, waits up
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
- New (2026-11-23): adaptive text (Settings > Desktop "Darker text on bright wallpapers", default on). `src/backdrop.rs` reads each monitor's wallpaper (IDesktopWallpaper; fallbacks TranscodedWallpaper, CachedFiles, plain background colour) into a 48x27 brightness grid; a box whose backdrop is brighter than 0.60 (back below 0.50) draws its text with dark ink (`src/ink.rs`). Glass is unchanged. Not yet checked live.
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
- New (2026-11-22): "More options..." in the icon right-click menu shows Windows' own menu for
  the item (`src/shellmenu.rs`; details in ARCHITECTURE). Checked live: the full menu appeared
  (7-Zip, Defender, OneDrive, PowerRename...) and was dismissed with Esc. **No menu command was
  run** in testing.
- New (2026-11-22): type to jump (Settings > Desktop "Typing on the desktop": does nothing / jumps
  to the icon that starts with the typed letter (default) / starts a quick search). Uses
  `RegisterHotKey` only while the desktop is in front, no hook. Checked live: after clicking the
  desktop, "s" jumped to the next s-item and "s" again to the one after; a text box in another
  window received "abc1" normally; typing "xyz" right after switching from the desktop to another
  window arrived intact 3 times out of 3.
- New (2026-11-23): Windows accent colour for highlights (setting follow_accent, refreshed on
  WM_DWMCOLORIZATIONCOLORCHANGED / WM_SETTINGCHANGE; dark accents lightened for contrast). OneDrive
  sync (setting sync_onedrive; `storage::write_sync` only writes when the contents differ, a newer
  shared file is loaded through RestoreBackup; checked with a stand-in OneDrive folder: a change from
  the "other PC" arrived in ~20 s). Weather: several places, dots to switch.
- New (2026-11-23): light and dark menus and settings (`src/theme.rs`). Settings > Desktop
  "Menus and settings": Like Windows (default, follows the Windows app mode and updates when
  Windows colours change) / Dark / Light. Menus and settings windows use the theme colours; boxes
  and widget faces keep their own colours.
- New (2026-11-23): weather widget with Meteocons icons (MIT licence, stored in `assets/meteocons/`).
  Shows feels-like, humidity, wind with direction, chance of rain, sunrise and sunset, UV, an hourly
  strip and 7 days with range bars. Widget settings offer Compact (current weather only), Standard
  (like the old widget), Detailed (everything) or "Adapts to the box size"; choosing one resizes the box.
- New (2026-11-23): icons: Ctrl+C / Ctrl+X / Ctrl+V on icons also work with files copied from other
  apps (Explorer), and paste goes to the box under the mouse. F2 renames the icon in place. Copy and
  cut survive the Windows menu helper closing. A click on an icon always gives the keyboard to the desktop.
- New (2026-11-23): optional drop shadow per box (Personalize: "Drop shadow", `Fence.shadow`): a separate
  click-through layered window below the box (`src/shadow.rs`), drawn once per size/corner/scale change
  (contact + ambient layers, transparent under the box), moved with the box, hidden with it. No window
  box fades below 50 %.
- New (2026-11-23): raised look per box (Personalize, `Fence.raised`): a bevel all round the inner
  edge (top brightest, sides soft, bottom dark) plus a sheen, drawn in `fence_view::paint_raised`;
  also on glass boxes.
- Fixed (2026-11-23): widget boxes stay behind other windows when clicked. `host.rs`
  WM_WINDOWPOSCHANGING refuses z-order changes without SWP_NOACTIVATE for box windows, and
  WM_ACTIVATE sends them to the bottom again.
- New (2026-11-23): to-do widget redesign: round check circles in the accent colour, a × on hover,
  "N left" count, and the add field hidden behind a "+" in the footer.
- Fixed (2026-11-23): quick search shows the right box for items inside folder boxes.
- New (2026-11-23): app icon and logo (`assets/icon`). The exe carries the icon for MSVC release
  builds (`build.rs`, rc.exe); tray and settings windows use it at run time (`src/appicon.rs`).
- New (2026-11-23): README screenshots now include quick search. Licence: GPL-3.0-or-later
  (LICENSE, also in the portable zip).
- New (2026-11-22, evening): internet allowed (CLAUDE.md rule 3). Widgets "Laptop status"
  (GetSystemPowerStatus, WLAN API, Bluetooth API; helper thread every 5 s while shown) and "Now
  playing" (Windows media session API: title, artist, cover, play/pause/next/previous, progress;
  helper thread every 2 s while shown; blank when nothing plays). Quick search also lists Start
  menu programs (`src/apps.rs`, setting `search_apps`). Undo for deleted boxes (toast panel
  UNDO_PANEL, tray, Settings > Boxes) and layout export/import (Settings > Layout). Optional fading
  (`HostSystem::set_box_opacity`, composition animation on the box's container; settings
  fade_idle/opacity/delay). Virtual desktops (`src/vdesk.rs`: registry change notification;
  IVirtualDesktopManager moves our windows to the desktop shown; setting `link_vdesks` = page per
  desktop) — checked live with a temporary second desktop. Box title Off / Left / Top / Bottom.
  Border drawn on the window edge (glass no longer shows outside it). Translations (`src/i18n.rs`,
  setting `language`, default automatic from GetUserDefaultUILanguage). GitHub Actions release
  workflow + portable mode.
- New (2026-11-22): creating boxes anywhere. (1) Draw: `src/drawbox.rs`, a layered GDI window over
  the whole desktop (Ctrl+Alt+B = hotkey id 5, tray, box menu > New > Draw a new box…); the result
  opens a "Create" menu (MenuTarget::Create) and the new box fills the rectangle. (2) Box menu
  "New ▸" places the box at the right-click point (`UiState.new_place`, `new_fence_rect`, clamped to
  that monitor's work area). (3) Desktop menu: registry static verbs under
  HKCU\Software\Classes\DesktopBackground\Shell\DesktopShepherd (cascading, `desktop::set_desktop_menu`,
  setting `desktop_menu`, default on, removed on uninstall); the started copy forwards
  "--new <what>" + cursor position to the running one with WM_COPYDATA (`forward_new_request`,
  SendMessageTimeout to our own hidden window) or leaves `pending-new.txt` if none runs. Windows 11
  shows these entries only under "Show more options" (the modern menu would need a DLL loaded into
  Explorer: rule 1). Checked live: drawn box exactly in the rectangle; "--new widget:clock" put a
  clock at the mouse; the classic menu shows DesktopShepherd ▸ with the submenu; New ▸ Box landed at
  the click point.
- New (2026-11-22): Settings > Layout shows all monitors (labelled main / second… or by display in
  fixed mode) or one at a time via dots; Settings > Monitors wording "Bind each box to: the monitor's
  role / the monitor itself" (both modes already existed).
- Note for tests: this PC's monitors are 2560×1440 pixels at 125 % (2048×1152 points); capture the
  full 2560×1440 when checking anything near the right or bottom edge.
- Fixed (2026-11-22, owner's report "typing writes into the note"): icon boxes don't take the
  focus, so whatever had it kept it (e.g. a sticky note) and desktop typing landed there. A left
  click in an icon box now gives the keyboard to the desktop (`desktop::focus_desktop`), the app
  re-checks the focus for a second after any change (Windows settles it in steps), "typing does
  nothing" no longer swallows letters, letters typed before the search window has the keyboard
  join the query, type-to-jump skips boxes on other pages, and switching pages / hiding clears the
  selection (Delete could otherwise remove an invisible item). Ranking: initials ("vsc") above
  loose matches, several words each starting a word. Checked live: click note, click icon, type
  "ste" -> quick search with Steam; the note unchanged.
- Done (2026-11-22): review-2 items R4 (folders opened with SHOpenFolderAndSelectItems /
  ShellExecute, no explorer.exe by name), R5 (windows by handle, not title), R8 (font list cached),
  R9 + idea 4 (folder change notifications instead of 4 s re-reads; no monitor polling; checked: a
  new file shows in ~1 s, a removed one disappears), R10 (Win+D guard stops early; covered time
  still 8–9 ms), R11 (drawing pad thins points, at most 300 strokes), R12 (extra icon sizes dropped
  when search/settings close), R13 (interval setting removed), R14 (re-stack after Explorer restart).
- Done (2026-11-22): review 3 (Sonnet subagent, 20 findings) fixed: setup no longer writes an empty
  layout on a new PC (it made first start show no boxes with icons hidden); only a main-thread crash
  restores the icons; agenda download can't hang on "Loading…" (panics caught, overflow-safe);
  weekday-only and yearly "2nd Tuesday" rules; data-folder move via a temporary copy; uninstall
  doesn't recreate the deleted folder; cmd/ping by full path; rename case-only / trailing dots /
  device names; paste finds the real new files; clipboard memory freed; off-screen check in points;
  backup restore refreshes; widget data always an object; shell-menu helper responsive; drive space
  read off the UI thread; supervisor stops on the quit signal while waiting.
- New (2026-11-22): memory: boxes on other pages / hidden give back their drawing memory (graphics
  memory 39 -> 26 MB with all hidden here); icons unused for 5 min are dropped.
- New (2026-11-22): agenda converts named time zones with the calendar's VTIMEZONE rules (test:
  09:00 New York -> 14:00 UTC in winter, 13:00 in summer).
- New (2026-11-22): picture frame widget (PNG/JPEG, fit or fill; decoded and shrunk to 1600 px on a
  helper thread) and box shortcuts Ctrl+Alt+F2..F10 (box menu > Keyboard shortcut: brings the box
  into view, switching page / un-hiding, then rolls it up or down). Ctrl+Alt+F1 is not offered: on
  this PC another tool swallows it (registration succeeds but the key never arrives).
- New (2026-11-22): agenda widget (`src/agenda.rs`, 4 unit tests). The owner approved this one
  network use. Downloads with WinHTTP on a short-lived thread every 30 min (5 after a failure),
  only while shown; reads single, all-day and repeating events (daily/weekly/monthly incl. "last
  Friday"/yearly, COUNT/UNTIL, EXDATE, moved instances, cancelled). Named time zones (TZID) are
  taken as local time (wrong for calendars in another zone). Checked live with Google's public US
  holidays calendar (Thanksgiving and Black Friday shown); CPU with it shown 0.016 %.
- New (2026-11-22): Personalize is its own window (two columns, font from a list); widget options
  (agenda links, clock 24-hour, calendar week start) are its "Widget settings" tab, opened from the
  box menu ("Widget settings…").
- New (2026-11-22): renamed DeskBoxes -> **DesktopShepherd** (exe `desktopshepherd.exe`, window
  classes, mutex `Local\DesktopShepherd.SingleInstance`, quit event `Local\DesktopShepherd.Quit`,
  Run value, env vars `DESKTOPSHEPHERD_*`). The data folder `%APPDATA%\DeskBoxes` is moved to
  `%APPDATA%\DesktopShepherd` on the first start (copied if it can't be moved); the old Run value
  is removed. Checked: the folder moved, all 8 boxes loaded.
- New (2026-11-22): installer built into the program (`src/install.rs`, `scripts\make-setup.ps1`,
  see README). Checked live on this PC: install, update over a running copy, uninstall keeping the
  boxes (program folder, Start menu shortcut, Installed apps entry and Run value all removed).
  Two bugs found and fixed while testing: the folder was deleted before the final message closed
  (so it stayed), and its path was garbled for cmd (now `raw_arg`).
- New (2026-11-22): widgets system monitor, calendar, to-do list and search box. Measured with the
  four on screen: worker CPU 0.02 % -> 0.05 % (the monitor's 2 s update; the others cost nothing
  when idle). A widget on another page or while boxes are hidden isn't drawn and reads nothing.
- New (2026-11-22): pages of boxes and "pages with dots" for full boxes (see README). Checked:
  a box moved to page 2 disappears from page 1; a box with 47 icons shows 2 dots and the wheel
  flips to page 2.
- New (2026-11-22): review 2 R1-R3 (Windows menu in a helper process, Properties etc. now work),
  keyboard in boxes incl. copy/cut/paste, rename (F2), folder boxes, rules by name/age, hide/show
  all, per-monitor-setup positions. **Per-setup positions are untested** (needs a laptop).
- Tests: all 20 unit tests pass (`cargo test`). Worker memory after start ~87 MB.
- Synthetic drags (SetCursorPos + mouse_event) don't start a drag on a *rolled-up* box's title
  (same before and after phase C); untested by hand.
- Still untested: mixed display scaling and real input-method (IME) use.
- Fixed in phase C: app-driven box moves (monitor relayout, bring back on screen, restore
  backup/import, roll-up from settings) now really move the DirectX windows; the Settings page
  scrolls (its bottom – backups list and Quit button – used to be cut off).

## Open items (2026-11-25)

- winget: rc.3 submitted as microsoft/winget-pkgs#488743 (manifests in `packaging/winget/`); waiting
  for the owner to accept Microsoft's CLA and for review. Once merged:
  `winget install radioactivetoy.DesktopShepherd`.
- Microsoft Store: not started (needs the owner's free Partner Center account; plan in
  `docs/plans/next-features.md` section 9).
- Bluetooth batteries: not yet tested with a real device (devices that don't report a battery don't show).
- Silent install (`--silent`) not tested end to end.
- Native-speaker review of the FR/DE/IT/PT texts (maybe Brazilian Portuguese; German "Strg" instead of "Ctrl").
- Laptop: per-monitor-setup positions still need a test on a real laptop.
- Code signing: the installer and program are not signed yet (SmartScreen warning).
- Library updates are blocked on this PC's GNU toolchain (`docs/BUILDING.md`); MSVC Build Tools would lift it.
- Dependabot glib alert: glib is not used on Windows.
- First run with several monitors: `desktop::desktop_icon_positions` (official shell API, own thread, 3 s timeout) reads where Windows' icons sit; icons on a non-main monitor get their own "Desktop N" box there (`plan_monitor_boxes` in `app.rs`, unit-tested). Tested only with the dump test (`cargo test icon_positions_dump -- --ignored --nocapture`) on a PC whose icons were all on the main monitor: icons on a second monitor are untested.
- `.cargo/config.toml` is no longer in the repo (personal path): copy `.cargo/config.example.toml` (see `docs/BUILDING.md`).

## Improvement ideas (ordered by value)

1. ~~Replace the eframe/glow root with a plain Win32 message loop~~ – done in phase C
   (2026-11-22): 155 → 110 MB. Remaining memory is mostly the D3D device + one composition
   swap chain per box (big boxes on a 125 % display are ~5–7 MB each) + egui font atlases +
   icon textures, so ideas 2 (rest) and 3 are the next memory wins.
2. ~~Memory: load icon textures at `icon_size × pixels_per_point` instead of 2× size~~ – done
   (2026-11-22, `IconCache::get(path, px)`; sizes no longer used are dropped by
   `IconCache::keep_only_sizes`; 110 → 89 MB together with the font change). Still open: free
   textures of items not shown in any box (LRU) – done 2026-11-22 (5-minute sweep).
3. ~~Memory: drop drawing memory of boxes not shown~~ – done 2026-11-22 (hidden / other page; rolled-up
   boxes already shrink with their window).
4. ~~CPU: desktop rescan via change notifications instead of polling every 4 s~~ – done 2026-11-22.
5. ~~Rename to DesktopShepherd~~ – done, with an installer (2026-11-22).
6. Features: more widgets (calendar, system monitor, to-do, search box, agenda and picture frame are done), per-box hotkeys (done), quick
   (Quick search, snapping to boxes/screen edges and an optional grid are done, 2026-11-22.)
- New (2026-11-24): pause when nobody can see the boxes (Settings > Desktop "Pause while a full-screen
  app or game is running", default on; `Settings.pause_fullscreen`). `desktop::poll_fullscreen` looks (no polling:
  on foreground change, minimize/restore, display change, plus again after 0.5 s and 1.5 s) at the foreground
  window of another program: SHQueryUserNotificationState (busy / exclusive D3D / presentation) or a window covering
  its whole monitor = full screen; a maximized window (always, no setting) or one filling the work area only covers
  its monitor. Boxes whose centre is on a covered monitor are not drawn and have no timer (`HostSystem::set_occluded`,
  redrawn forced when visible again); other monitors keep running. A real full-screen app also gives back all global
  shortcuts (`runloop::set_hotkeys_paused`) and holds the toasts. Screen off (power-setting notice) or session locked
  (WTS notice) pauses all boxes. Log lines "paused: ..." / "resumed". Not yet checked live with a game.
