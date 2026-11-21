# DeskBoxes — mechanics sheet (from decompiling the original app)

Decompiled with Ghidra through Craft Anything. The original's main exe is a launcher/watchdog;
the organizer is `Native-x64.dll`. Addresses: `0x14…` = exe, `0x18…` = dll. Our code is
written from these notes, not copied.

## Launcher and watchdog — exe 0x140012300, 0x140018f90
- Loads `Native-x64.dll` (from its own folder, only if the file exists) and calls its exports
  `Startup` / `Startup2` / `Shutdown`.
- Timer id 0x65 runs every **2000 ms**: if the library's message window (class `_cls_msg_`) is
  gone, it restarts the library (`-frestart`; `-fbackup` restores a backup).
- On `TaskbarCreated` (Explorer restarted) it checks the helper window and restarts if needed.
- Timer id 100 waits up to **60 ticks** (0x3c) before calling `Startup2`.
- **Rebuild:** a supervisor process starts the app with `--worker` and restarts it after a
  crash, **2000 ms** later. A clean exit ends both. It gives up after **3 crashes within 60 s**
  (our own limit, so it can never loop) and then shows the desktop icons again if the app had
  hidden them.

## Startup — dll Startup 0x18019f600, 0x1802b00a0, 0x18018c1b0
- Waits for the desktop host: EnumWindows for `WorkerW`/`Progman` that owns a
  `SHELLDLL_DefView` (0x1802b01f0), checking every **50 ms**, up to **120** tries (6 s), or
  **600** tries (30 s) when started by `boot`/`service`/`schedule` or `checkRestartExplorer`
  (0x18018c1b0). Meanwhile it sends Progman message 0x52C to make Windows create the WorkerW.
- In its default mode it then **injects into Explorer**: `SetWindowsHookExW(WH_GETMESSAGE=3)` on
  Explorer's desktop thread, waits up to **3000 ms** for event
  `{D3DFD934-029B-4464-BD48-D9455F4499F9}`, and retries without it if that fails.
- **Rebuild:** same desktop wait (120 or 600 tries × 50 ms; the Run key adds `--boot`).
  On purpose we do **not** inject into Explorer: boxes are our own windows, so a problem in
  DeskBoxes can never freeze Explorer, the taskbar or the desktop (the person's top priority).

## Always behind other windows
- The original keeps boxes in or under the desktop host (0x18025e400 checks
  Progman/SHELLDLL_DefView/WorkerW and the icon list view style).
- **Rebuild:** boxes start at the bottom of the window stack, and clicking a box doesn't
  activate it (`WM_MOUSEACTIVATE` → `MA_NOACTIVATE`), so Windows never raises it over an app.
  Widget boxes may take focus for typing; when any box loses focus it gets one asynchronous
  "go to the bottom" request. (A first version forced *every* z-order change to the bottom.
  That can clash with Explorer, which keeps its own desktop at the bottom, so it was replaced.
  The person saw 10–15 s Explorer stalls during that test session.)
- Moving and resizing are done by the app itself (mouse position from `GetCursorPos`,
  `OuterPosition` / `InnerSize` each step), not by Windows' modal move/size loop. That loop
  pauses drawing, which made the glass wallpaper slide and left black areas while growing.
- Hidden desktop icons come back whenever the app stops: on quit, after a crash (panic hook),
  and when it's force-closed (the supervisor shows them as soon as the app process ends).

## Box record — loader 0x180198d30, writer 0x18019a1a0, constructor 0x180197810
Saved fields: `name, expand, arrange_type, sort_type, sort_upper, lock, category_type`
(6 → 4 migration), `category_tag, pos_left/top/right/bottom, factor, icon_size_cx/cy,
before_icon_size_cx/cy, list_custom_icon_size_cx/cy, directory, sel_tab_id, list_view,
primary, color, boxAlpha, border_color, tabType, titleType, corner, lnkArrow, showShield,
effectType, effectDegree, onlyIcon, showCaption, bkImage`, tab/title/item font name, size,
colour and shadow, `IconSpacing, IconVerticalSpacing, title_isolated, apps`.

Constructor defaults (0x180197810):
| field | default | meaning |
|---|---|---|
| expand (+0x60) | 1 | starts unrolled |
| sort_upper (+0x6c) | 1 | ascending |
| factor (+0x120) | 100 | 100 % scale |
| sel_tab_id (+0x118) | −1 | none |
| list_view (+0x11c) | 0 | icon view |
| color, border_color, boxAlpha, tabType, titleType, corner, lnkArrow, showShield, onlyIcon, showCaption | −1 | use the app-wide theme |
| effectType / effectDegree | 0 / 30 | effect off, strength 30 |

## App-wide theme defaults — 0x1801e4470, 0x1801e43f0, 0x1801e4430, 0x1801e44f0, 0x1801e40f0
- `box_color` = **0xFF000000** (black)
- `box_alpha` = **50** (out of 255; `full_box_alpha` = 255)
- `show_border` = **1**
- `roundCorner` = **0**, or **10** if Windows' major version is above 10 (0x1802aea40 +
  0x1802ae150: GetVersionEx / RtlGetVersion). In practice that's 0 on Windows 10 and 11.
- **Rebuild:** new boxes are black at opacity 50/255 with a border. The corner radius is
  **8 px**, a deliberate change: the person asked for the Windows 11 look, and Windows 11 rounds
  windows by 8 px. Boxes saved by the first build (opacity 170, 6 px) are moved to these values
  once (layout version 2).

## Icon size — box menu 0x180223170, getter 0x180199bc0 (icon_size_cx at +0x70)
- Menu choices: Large (0x12), Medium (0x11), Small (0x13), List (0x14), Auto-arrange (0x15),
  Show icons (0x18).
- Size → choice: **< 48 px → small; 48–95 → medium; ≥ 96 → large**.
- **Rebuild:** View offers Small **32**, Medium **48**, Large **96** (the standard Windows
  desktop sizes that fall in those steps). List view isn't built yet.

## Sorting — box menu 0x180223170
Name (0x27), Size (0x28), Type, Date modified (0x29), Times opened (0x2a); ascending by default.
**Rebuild:** all five, plus "as I arrange them". Times opened is counted each time an item is
opened from a box and saved in the layout.

## Item kinds — 0x1801942f0
- `__path_text_` prefix → text item (0x10)
- shell namespace path (2-char prefix) → shell item (8)
- directory → 2
- `.lnk` / `.url` → shortcut (1)
- any other file → 4
- It also reads last-write time, attributes and size with FindFirstFileW.

**Rebuild:** folders, shortcuts and files are told apart the same way. Size and date modified
are read for sorting.

## Categories (auto-tab) — 0x1801ecbc0, option `auto_tab` 0x180206bc0 / 0x1801e5fe0
User categories are a **name + list of extensions** (template strings
`STRID_DESK_CATEGORY_TMP_NAME` / `_EXT`). **Rebuild:** the auto-sort box makes one tab per
rule (editable in Settings: name + extensions), plus Folders and Other. Defaults: Apps, Files,
Archives, Pictures.

## Icon spacing — 0x1801be040 (and 0x1801e3f00)
Registry `IconSpacing` / `IconVerticalSpacing` (twips): **pixels = −twips / 15**, clamped to
**16..500** (Windows default −1125 → 75 px). Rebuild: same formula; horizontal cell pitch =
max(spacing, icon + 20).

## Other options (0x1801e5fe0 apply / 0x1801e6750 save)
`boot` → Run key `HKCU\…\CurrentVersion\Run` (rebuild: same, value "DeskBoxes" with `--boot`);
`auto_expand` → expand rolled-up boxes on hover (rebuild: on by default); `organize_hide` →
hide system icons (rebuild: off by default, always restored on exit/crash, re-applied if
Explorer restarts); `tab_vertical`/`title_vertical` → vertical title and tabs (rebuild default).

## Still estimated (looked for, not found as constants)
- Cell height = icon + 52 px (measured from the original's layout picture).
- Save delay 1.5 s, desktop re-read every 4 s, 10 backups kept (rebuild choices).
