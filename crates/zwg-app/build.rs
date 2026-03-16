//! Build script for zwg-app
//! - Embeds Windows application icon
//! - Sets up Windows resources

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../resources/icons/zwg.ico");

    // Embed Windows icon
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set("ProductName", "ZWG Terminal");
        res.set(
            "FileDescription",
            "ZWG Terminal — Ghostty-powered Windows terminal",
        );
        res.set("LegalCopyright", "MIT License");
        res.set_icon("../../resources/icons/zwg.ico");

        if let Err(e) = res.compile() {
            eprintln!("Warning: Failed to compile Windows resources: {}", e);
        }
    }
}
