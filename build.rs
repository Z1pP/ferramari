//! Embed the application icon and version information in `ferramari.exe`.
//!
//! Explorer, the taskbar, the notification area and the installer read the
//! icon from the executable resource, not from the Iced window. The resource
//! is built from the same SVG-derived ICO the control surface shows; the file
//! is committed so a release build needs no image tools.
//!
//! `winresource` locates `rc.exe` through the Windows SDK registry key. The
//! dependency is Windows-only, and this script does nothing on Linux, where
//! the desktop entry installs the SVG instead.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let icon = manifest.join("data/icons/windows/ferramari.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon(icon.to_str().expect("icon path is UTF-8"))
            .set("ProductName", "Ferramari")
            .set("FileDescription", "Ferramari video wallpaper server")
            .set("LegalCopyright", "GPL-3.0-or-later")
            .set("OriginalFilename", "ferramari.exe");
        resource.compile().expect("compile the Windows resource");
    }
}
