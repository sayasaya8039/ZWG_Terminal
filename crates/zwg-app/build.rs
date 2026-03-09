//! Build script for zwg-app
//! - Embeds Windows application icon
//! - Sets up Windows manifest

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Embed Windows icon and manifest
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set("ProductName", "ZWG Terminal");
        res.set("FileDescription", "ZWG Terminal — Ghostty-powered Windows terminal");
        res.set("LegalCopyright", "MIT License");

        // Icon will be added in Phase 2 when we create the icon
        // res.set_icon("../../resources/icons/zwg.ico");

        if let Err(e) = res.compile() {
            eprintln!("Warning: Failed to compile Windows resources: {}", e);
        }
    }
}
