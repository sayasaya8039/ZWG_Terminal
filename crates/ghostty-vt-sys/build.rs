//! Build script for ghostty-vt-sys
//!
//! Phase 1: Will build Ghostty's Zig library via `zig build`
//! Currently a no-op placeholder

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=zig/lib.zig");
    println!("cargo:rerun-if-changed=include/ghostty_vt.h");

    // Phase 1: Uncomment when Ghostty submodule is integrated
    // build_ghostty_vt();
}

#[allow(dead_code)]
fn build_ghostty_vt() {
    // Find zig binary
    let zig = std::env::var("ZIG").unwrap_or_else(|_| "zig".to_string());

    // Build the Zig library
    let status = std::process::Command::new(&zig)
        .args(["build", "-Doptimize=ReleaseFast"])
        .current_dir("zig")
        .status()
        .expect("Failed to run zig build");

    if !status.success() {
        panic!("Zig build failed with status: {}", status);
    }

    // Link the static library
    println!("cargo:rustc-link-search=native=zig/zig-out/lib");
    println!("cargo:rustc-link-lib=static=ghostty_vt");
}
