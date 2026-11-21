// This computer's GNU Rust toolchain lacks a few Windows import libraries (e.g. shlwapi);
// app/lib holds stand-ins (the Windows API import library from the `windows` crate family).
fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("lib");
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rerun-if-changed=build.rs");
}
