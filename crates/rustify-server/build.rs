//! Ensure `web/dist` exists before compiling so a fresh clone builds even
//! when the web bundle has not been produced yet (rust-embed needs the folder
//! to exist at compile time).

use std::path::Path;

fn main() {
    // Workspace root is two levels up from this crate directory.
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo");
    let dist = Path::new(&manifest_dir)
        .join("..")
        .join("..")
        .join("web")
        .join("dist");
    if let Err(e) = std::fs::create_dir_all(&dist) {
        println!("cargo:warning=failed to create {}: {e}", dist.display());
    }
    println!("cargo:rerun-if-changed=build.rs");
}
