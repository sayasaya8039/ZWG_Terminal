use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=zig/lib.zig");
    println!("cargo:rerun-if-changed=zig/build.zig");
    println!("cargo:rerun-if-changed=zig/build.zig.zon");
    println!("cargo:rerun-if-changed=include/ghostty_vt.h");

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");

    // Check vendor/ghostty submodule
    let ghostty_dir = workspace_root.join("vendor").join("ghostty");
    if !ghostty_dir.join("src").join("terminal").exists() {
        panic!(
            "Ghostty submodule not found at {:?}. Run: git submodule update --init --recursive",
            ghostty_dir
        );
    }

    let zig = find_zig();
    let zig_dir = manifest_dir.join("zig");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let prefix = out_dir.join("zig-out");

    eprintln!("Building ghostty_vt with zig at {:?}", zig);
    eprintln!("  zig source dir: {:?}", zig_dir);
    eprintln!("  output prefix: {:?}", prefix);

    // Zig 0.15.2 on Windows: global cache must be on the same drive as source
    // AND close to the source directory to avoid deep relative path resolution
    // failures (too many ../.. levels from cargo's deep OUT_DIR).
    let zig_global_cache = zig_dir.join(".zig-global-cache");
    std::fs::create_dir_all(&zig_global_cache).ok();

    let prefix_str = prefix.display().to_string();
    let status = Command::new(&zig)
        .args(["build", "-Doptimize=ReleaseFast", "-p", &prefix_str])
        .env("ZIG_GLOBAL_CACHE_DIR", &zig_global_cache)
        .current_dir(&zig_dir)
        .status()
        .unwrap_or_else(|e| panic!("Failed to run zig build: {}", e));

    if !status.success() {
        panic!("Zig build failed with status: {}", status);
    }

    let lib_dir = prefix.join("lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=ghostty_vt");
    println!("cargo:rustc-link-lib=c");
}

fn find_zig() -> PathBuf {
    // 1. Check ZIG environment variable
    if let Ok(zig) = std::env::var("ZIG") {
        let p = Path::new(&zig);
        if p.exists() {
            return p.to_path_buf();
        }
    }

    // 2. Check system PATH
    if let Ok(output) = Command::new("zig").arg("version").output() {
        if output.status.success() {
            return PathBuf::from("zig");
        }
    }

    // 3. Check common Windows locations
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let candidates = [
        format!("{}\\.zig\\zig.exe", home),
        format!("{}\\AppData\\Local\\Programs\\zig\\zig.exe", home),
    ];
    for c in &candidates {
        if Path::new(c).exists() {
            return PathBuf::from(c);
        }
    }

    panic!("Zig compiler not found. Install Zig 0.15.2+ and ensure it's in PATH.");
}
