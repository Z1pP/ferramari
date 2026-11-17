//! Embed the application icon and version information in `ferramari-ui.exe`.
//!
//! The control surface draws its own title bar, so the taskbar and Alt-Tab
//! icon comes from the Iced window (`window_icon` in `src/main.rs`); this
//! resource is what Explorer, the shortcut, the uninstaller and the tray read
//! from the executable. Both binaries share the same committed ICO, so the
//! tray icon loaded from resource ID 1 matches the window icon exactly.
//!
//! The dependency and the resource are Windows-only; on Linux this script
//! does nothing and the desktop entry installs the SVG instead.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let icon = manifest.join("../../data/icons/windows/ferramari.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon(icon.to_str().expect("icon path is UTF-8"))
            .set("ProductName", "Ferramari")
            .set("FileDescription", "Ferramari control surface")
            .set("LegalCopyright", "GPL-3.0-or-later")
            .set("OriginalFilename", "ferramari-ui.exe");
        resource.compile().expect("compile the Windows resource");
    }
}
