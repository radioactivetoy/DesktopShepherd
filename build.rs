// This computer's GNU Rust toolchain lacks a few Windows import libraries (e.g. shlwapi);
// app/lib holds stand-ins (the Windows API import library from the `windows` crate family).
//
// It also adds the program's file icon (a Windows resource). Never fails the build: without a
// resource compiler the program just gets the default icon (window icons are set at run time).
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("lib");
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon/desktopshepherd.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if let Err(e) = add_icon() {
        println!("cargo:warning=program icon resource skipped: {e}");
    }
}

fn add_icon() -> Result<(), String> {
    let out = PathBuf::from(std::env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let ico = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("icon").join("desktopshepherd.ico");
    if !ico.exists() {
        return Err("assets/icon/desktopshepherd.ico not found".into());
    }
    let ico_s = ico.display().to_string().replace('\\', "\\\\");
    let v = |n: &str| std::env::var(n).unwrap_or_else(|_| "0".into());
    let (maj, min, pat) = (v("CARGO_PKG_VERSION_MAJOR"), v("CARGO_PKG_VERSION_MINOR"), v("CARGO_PKG_VERSION_PATCH"));
    let rc_text = format!(
        "1 ICON \"{ico_s}\"\n\
         1 VERSIONINFO\n\
         FILEVERSION {maj},{min},{pat},0\n\
         PRODUCTVERSION {maj},{min},{pat},0\n\
         BEGIN\n\
         BLOCK \"StringFileInfo\"\n\
         BEGIN\n\
         BLOCK \"040904b0\"\n\
         BEGIN\n\
         VALUE \"ProductName\", \"DesktopShepherd\"\n\
         VALUE \"FileDescription\", \"DesktopShepherd\"\n\
         VALUE \"FileVersion\", \"{maj}.{min}.{pat}\"\n\
         VALUE \"ProductVersion\", \"{maj}.{min}.{pat}\"\n\
         END\n\
         END\n\
         BLOCK \"VarFileInfo\"\n\
         BEGIN\n\
         VALUE \"Translation\", 0x409, 1200\n\
         END\n\
         END\n"
    );
    let rc_file = out.join("app.rc");
    std::fs::write(&rc_file, rc_text).map_err(|e| e.to_string())?;

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let res = out.join("app.res");
        let rc = find_rc().ok_or("rc.exe not found")?;
        let status = Command::new(&rc).arg("/nologo").arg("/fo").arg(&res).arg(&rc_file).status().map_err(|e| format!("running {}: {e}", rc.display()))?;
        if !status.success() {
            return Err(format!("{} failed ({status})", rc.display()));
        }
        println!("cargo:rustc-link-arg-bins={}", res.display());
        Ok(())
    } else {
        // GNU toolchain: windres makes a COFF object.
        let obj = out.join("app_res.o");
        let mut last = String::from("windres not found");
        for tool in ["windres", "x86_64-w64-mingw32-windres"] {
            match Command::new(tool).arg("-i").arg(&rc_file).arg("-o").arg(&obj).arg("-O").arg("coff").status() {
                Ok(s) if s.success() => {
                    println!("cargo:rustc-link-arg-bins={}", obj.display());
                    return Ok(());
                }
                Ok(s) => last = format!("{tool} failed ({s})"),
                Err(e) => last = format!("{tool}: {e}"),
            }
        }
        Err(last)
    }
}

fn find_rc() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("RC") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    if Command::new("rc").arg("/?").output().is_ok() {
        return Some(PathBuf::from("rc"));
    }
    let bin = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(bin).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs.reverse();
    dirs.into_iter().map(|d| d.join("x64").join("rc.exe")).find(|p| p.exists())
}
