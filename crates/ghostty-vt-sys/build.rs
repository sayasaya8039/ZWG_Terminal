use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=zig/lib.zig");
    println!("cargo:rerun-if-changed=zig/build.zig");
    println!("cargo:rerun-if-changed=zig/build.zig.zon");
    println!("cargo:rerun-if-changed=zig/gpu_renderer.zig");
    println!("cargo:rerun-if-changed=zig/dx12.zig");
    println!("cargo:rerun-if-changed=zig/shaders.zig");
    println!("cargo:rerun-if-changed=zig/async_io.zig");
    println!("cargo:rerun-if-changed=zig/content_scan.zig");
    println!("cargo:rerun-if-changed=zig/vk.zig");
    println!("cargo:rerun-if-changed=zig/vulkan_renderer.zig");
    println!("cargo:rerun-if-changed=zig/shaders/terminal.vert");
    println!("cargo:rerun-if-changed=zig/shaders/terminal.frag");
    println!("cargo:rerun-if-changed=zig/shaders/terminal.vert.spv");
    println!("cargo:rerun-if-changed=zig/shaders/terminal.frag.spv");
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
    let ghostty_src_link = zig_dir.join("ghostty_src");
    ensure_ghostty_src(&ghostty_dir.join("src"), &ghostty_src_link)
        .unwrap_or_else(|e| panic!("Failed to prepare ghostty_src for zig build: {}", e));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let prefix = out_dir.join("zig-out");

    eprintln!("Building ghostty_vt with zig at {:?}", zig);
    eprintln!("  zig source dir: {:?}", zig_dir);
    eprintln!("  output prefix: {:?}", prefix);

    let prefix_str = prefix.display().to_string();
    let mut args = vec![
        "build".to_string(),
        "-Doptimize=ReleaseFast".to_string(),
        "-p".to_string(),
        prefix_str.clone(),
    ];

    // When targeting MSVC, tell Zig to emit MSVC-compatible objects
    // so that `link.exe` can resolve symbols like `__chkstk` correctly.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("msvc") {
        args.push("-Dtarget=x86_64-windows-msvc".to_string());
    }

    let mut cmd = Command::new(&zig);
    cmd.args(&args).current_dir(&zig_dir);

    // Zig 0.15.2 on Windows: global cache must be on the same drive as source
    // AND close to the source directory to avoid deep relative path resolution
    // failures (too many ../.. levels from cargo's deep OUT_DIR).
    //
    // On Linux/macOS (incl. WSL + repo on /mnt/<drv>): keep Zig caches under $HOME
    // so rename(2) into .zig-cache works (DrvFs / NTFS mounts often deny it).
    #[cfg(windows)]
    {
        let zig_global_cache = zig_dir.join(".zig-global-cache");
        std::fs::create_dir_all(&zig_global_cache).ok();
        cmd.env("ZIG_GLOBAL_CACHE_DIR", &zig_global_cache);
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| zig_dir.clone());
        let cache_root = base.join(".cache").join("zwg-terminal-zig");
        let zig_global_cache = cache_root.join("global");
        let zig_local_cache = cache_root.join("local");
        std::fs::create_dir_all(&zig_global_cache).ok();
        std::fs::create_dir_all(&zig_local_cache).ok();
        cmd.env("ZIG_GLOBAL_CACHE_DIR", &zig_global_cache);
        cmd.env("ZIG_LOCAL_CACHE_DIR", &zig_local_cache);
    }

    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("Failed to run zig build: {}", e));

    if !status.success() {
        panic!("Zig build failed with status: {}", status);
    }

    let lib_dir = prefix.join("lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=ghostty_vt");

    // On MSVC, the C runtime (msvcrt) is linked automatically.
    // Only emit -lc on non-MSVC targets (e.g. gnu/mingw).
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("msvc") {
        println!("cargo:rustc-link-lib=c");
    }

    // DX12 GPU renderer system libraries
    println!("cargo:rustc-link-lib=d3d12");
    println!("cargo:rustc-link-lib=dxgi");
    println!("cargo:rustc-link-lib=d3dcompiler");
    println!("cargo:rustc-link-lib=dwrite");
    println!("cargo:rustc-link-lib=gdi32");
}

fn ensure_ghostty_src(source: &Path, target: &Path) -> io::Result<()> {
    if target.exists() {
        return Ok(());
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }

    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(source, target).is_ok() {
            return Ok(());
        }
    }

    copy_dir_recursive(source, target)
}

fn copy_dir_recursive(source: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir_all(target)?;

    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let entry_type = entry.file_type()?;
        let from = entry.path();
        let to = target.join(entry.file_name());

        if entry_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if entry_type.is_file() {
            fs::copy(&from, &to)?;
        }
    }

    Ok(())
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
