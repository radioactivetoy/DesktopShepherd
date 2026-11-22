# Building

## Toolchain

- Rust stable, **GNU** target (`stable-x86_64-pc-windows-gnu`), tested with rustc 1.100.
- This machine's GNU toolchain has no binutils `as` and no full set of Windows import
  libraries. Three workarounds are in the repo:
  1. `.cargo/config.toml` points rustc at the toolchain's bundled `dlltool.exe`
     (absolute path – adjust it on another computer, or delete the file when using the
     MSVC toolchain).
  2. `lib/libshlwapi.a` is a stand-in import library (a copy of the `windows_x86_64_gnu`
     0.52.6 `libwindows.0.52.0.a`); `build.rs` adds `lib/` to the link search path.
  3. Crates that use `raw-dylib` (which needs dlltool/`as` combos that fail here) are
     **pinned** in `Cargo.lock`: chrono 0.4.39, parking_lot 0.12.3, parking_lot_core 0.9.10.
     (The old pins of libloading 0.8.6, arboard and webbrowser no longer apply: since phase C
     the app uses plain `egui` 0.31 instead of eframe, and those crates are no longer in the
     dependency tree at all. libloading 0.7.4 still comes in through another crate.)

**Never run `cargo update`** (or add crates that pull newer versions of the above) – the build
will break with dlltool / raw-dylib errors. When adding a dependency, check `Cargo.lock` diff
afterwards; if a pinned crate moved, re-pin with `cargo update -p <crate> --precise <ver>`.

On the MSVC toolchain none of this should be needed (untested).

## Profiles

- `dev` uses opt-level 3 (deps 2) so everyday builds already run at full speed.
- `release`: thin LTO, 1 codegen unit.

## The exe is locked while running

`cargo build` fails with "failed to remove file …deskboxes.exe" when the app is running.
Run `scripts\stop.ps1` first. Do **not** kill only the supervisor: kill the worker first, or
desktop icons stay hidden until the next start (fix: start and quit the app, or toggle
"Show desktop icons" in Explorer's desktop right-click → View).

## Windows crate features

`windows` 0.58 features used: Foundation, UI_Shell(+Common), WindowsAndMessaging, HiDpi,
Graphics_Gdi, Graphics_Dwm, UI_Controls, Storage_Xps, UI_Input_KeyboardAndMouse,
System_LibraryLoader, System_WinRT(+Composition), UI, UI_Composition(+Desktop),
Foundation_Numerics, System, Graphics_Direct3D(+Fxc), Direct3D11, Dxgi(+Common), System_Com,
System_Threading, Security. Add new ones to `Cargo.toml` when a Win32 call doesn't resolve.
