# Plan: next features (5, 6, 7, 8, 9, 10)

## Status (2026-11-25)

- Done in 1.0.0-rc.3: 5 (world clock), 6 (network speed), 7 (volume / sound output), 8 (Bluetooth
  batteries, not yet tested with a real device), 10 (languages FR/DE/IT/PT).
- 9: winget submitted (microsoft/winget-pkgs#488743, waiting for the owner's CLA and review);
  Microsoft Store not started (needs the owner's Partner Center account).

Agreed 2026-11-24. All within the rules in CLAUDE.md: no new crates (only extra Windows API
*features* on the existing `windows` dependency, Cargo.lock unchanged), no polling beyond what a
shown widget needs, slow readings on the shared long-lived `Poller` helper (widgets.rs), every
text through `t()`, layout back-compat (`#[serde(default)]`, widget data in JSON).

Order (each its own commit, tested on screen and measured):
1. 10 Languages (no code risk, big text change — do first so new strings of 5-8 are translated once)
2. 5 World clock, 6 Network speed (small)
3. 7 Volume / output, 8 Bluetooth batteries (medium, Windows audio/device APIs)
4. 9 winget, then Microsoft Store (needs the owner's accounts)
Then: profile CPU/memory, release as 1.0 RC 3.

## 10. More languages: French, German, Italian, Portuguese
- `i18n.rs`: `Lang` gets Fr, De, It, Pt; one table per language (today ES and CA, ~374 entries
  each); `lang()` detection from the Windows UI language (fr-*, de-*, it-*, pt-*); language list in
  Settings shows each in its own name (Français, Deutsch, Italiano, Português).
- Date helpers (`long_date`, `month_year`, `weekday_short`, month names) per language.
- The table-consistency test checks every table against the English keys.
- Translation by cheaper agents (one per language), natural UI wording, same placeholders `{}`.
- Risk: machine-quality wording. Mitigation: README asks native speakers to suggest fixes.
- Effort: small (mostly text). Cost: a few hundred KB of text in the program.

## 5. World clock (clock widget)
- Widget settings: "Other time zones" list — add from Windows' own list of time zones
  (`EnumDynamicTimeZoneInformation`, localised names), optional short label ("NYC").
- Time conversion: `SystemTimeToTzSpecificLocalTimeEx` with the zone's
  `DYNAMIC_TIME_ZONE_INFORMATION` (daylight saving handled by Windows; no time-zone crate).
  Windows feature: `Win32_System_Time`.
- Drawing: small rows under the main time: label, time, difference ("−6 h"), a moon icon at night.
  Box grows to fit, or rows shrink; the clock already redraws once a minute — no extra cost.
- Stored in the widget's data: `zones: [{ id, label }]`.
- Effort: small.

## 6. Network speed (system monitor)
- `GetIfTable2` (Windows feature `Win32_NetworkManagement_IpHelper`): sum received/sent bytes of
  connected, non-loopback, non-virtual interfaces; speed = difference between two readings.
- Read with the system monitor's other readings (every 2 s while shown), cheap (well under 1 ms).
- Drawing: "↓ 12.3 MB/s  ↑ 0.8 MB/s" line plus a small two-colour history graph like the CPU one;
  units adapt (KB/s, MB/s; option bits/s in widget settings).
- Effort: small.

## 7. Volume and output device ("Now playing")
- Volume and mute through Windows' official audio API (Core Audio `IAudioEndpointVolume` on the
  default output device), on the media helper thread (COM already initialised there). Features:
  `Win32_Media_Audio`, `Win32_Media_Audio_Endpoints`.
- Shows: speaker icon (click = mute), thin volume slider, name of the current output device
  ("Headphones (WH-1000XM5)").
- Switching the output device: Windows has **no official API** for changing the default device
  (only an undocumented one that can break with Windows updates). Plan: clicking the device name
  opens Windows' own output picker (the same as Win+Ctrl+V / the quick settings sound list). Safe
  and always correct.
- Widget-rule note: like play/pause, these are the media widget's core controls (an accepted
  exception to "no buttons on widgets"); they appear only while the mouse is over the widget, so
  the widget stays clean.
- Effort: medium.

## 8. Bluetooth device batteries (Laptop status)
- Windows exposes the battery level of many Bluetooth devices (headphones, mice, keyboards) as a
  device property (the same one Settings > Bluetooth shows). Read with Windows' device
  enumeration (`Windows.Devices.Enumeration`, feature `Devices_Enumeration`) asking for the
  battery property, on the laptop `Poller` every 5 minutes while the widget shows.
- Shows: one line per connected device with a battery: icon, name, percentage, low = orange.
- Limitation: devices that don't report a battery to Windows (some older or proprietary ones)
  won't show — the same as in Windows' own Settings.
- Effort: medium (device APIs vary; needs testing with the owner's devices).

## 9. winget and Microsoft Store
### winget (first)
- winget needs a silent install: add `--silent` (and `/S`) to the setup (`install.rs`): no
  dialogs, keeps settings, exit code 0/1. Also silent uninstall for winget's uninstall.
- Manifest (3 YAML files: version, installer with URL + SHA-256 from the release's
  SHA256SUMS.txt, locale with description/licence GPL-3.0-or-later) submitted as a pull request
  to `microsoft/winget-pkgs` — **from the owner's GitHub account, with the owner's OK** (it's
  publishing). Microsoft's bots validate and a moderator merges (days).
- Later releases: a step in `release.yml` (`wingetcreate update`) can open the update PR
  automatically; it needs a GitHub token the owner creates and stores as a repository secret.
- Result: `winget install DesktopShepherd`.
### Microsoft Store (second)
- The owner registers a free individual developer account in Partner Center (their identity;
  only they can do it).
- Store route without buying a certificate: an **MSIX package** (the Store signs it, so no
  "unknown publisher"). `makeappx.exe` is on this PC (Windows SDK).
- Changes the Store build needs (a build flag, same code otherwise):
  - no own installer/uninstaller/update check (the Store updates it);
  - "Start with Windows" through the package's StartupTask instead of the registry;
  - the desktop right-click menu entries can't be written from a Store package — hidden in the
    Store build (or done later with a packaged shell extension, which we avoid);
  - settings stay in `%APPDATA%\DesktopShepherd` (allowed; may need the package's
    "unvirtualized resources" setting so users switching between versions keep their boxes).
- Store listing: description, screenshots (README ones), privacy policy URL (a short page in the
  repo: "no data collected"), age rating questionnaire.
- Effort: winget small; Store medium, plus waiting for certification.

## Testing for each
- Build, tests, on-screen check with temporary boxes on the second monitor.
- Fast leak test where a new periodic reading is added (network, volume, Bluetooth).
- Memory/CPU before and after; overnight recording before the RC 3 release.
