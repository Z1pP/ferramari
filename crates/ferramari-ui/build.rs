//! Embed the application icon and version information in `ferramari-ui.exe`
//! and compile the translation catalogs next to the build's binaries.
//!
//! The control surface draws its own title bar, so the taskbar and Alt-Tab
//! icon comes from the Iced window (`window_icon` in `src/main.rs`); this
//! resource is what Explorer, the shortcut, the uninstaller and the tray read
//! from the executable. Both binaries share the same committed ICO, so the
//! tray icon loaded from resource ID 1 matches the window icon exactly.
//!
//! The dependency and the resource are Windows-only; on Linux this part of the
//! script does nothing and the desktop entry installs the SVG instead.
//!
//! `po/*.po` is also compiled into `<target>/<profile>/locale`, the exact
//! layout `paths::locale_dir` looks for next to an executable. Without it a
//! `cargo build` tree had the UI but no catalogs, so an in-app language choice
//! was written to `config.json` and still rendered English. Packaging still
//! compiles its own copy (`installer/build.ps1`, `make install`); `msgfmt` is
//! optional here and only a warning when gettext is not installed.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let icon = manifest.join("../../data/icons/windows/ferramari.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    // The build script is compiled for the host, so `cfg(windows)` alone would
    // still run `rc.exe` when cross-compiling to Linux. Check the target too.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon(icon.to_str().expect("icon path is UTF-8"))
            .set("ProductName", "Ferramari")
            .set("FileDescription", "Ferramari control surface")
            .set("LegalCopyright", "GPL-3.0-or-later")
            .set("OriginalFilename", "ferramari-ui.exe");
        resource.compile().expect("compile the Windows resource");
    }

    build_catalogs(&manifest);
}

/// Compile every `po/*.po` into the locale directory next to the binaries.
///
/// The destination is derived from `OUT_DIR` rather than `CARGO_TARGET_DIR`
/// so a custom target directory and a cross-compilation triple both land in
/// the directory Cargo actually put the executables in.
fn build_catalogs(manifest: &Path) {
    let po_dir = manifest.join("../../po");
    println!("cargo:rerun-if-changed={}", po_dir.display());
    println!("cargo:rerun-if-env-changed=MSGFMT");

    let Some(msgfmt) = find_msgfmt() else {
        println!(
            "cargo:warning=msgfmt was not found; the build will only show English strings. \
             Install gettext or set MSGFMT to compile po/*.po."
        );
        return;
    };
    let Some(locale_dir) = target_profile_dir().map(|dir| dir.join("locale")) else {
        println!("cargo:warning=could not derive the target profile directory from OUT_DIR");
        return;
    };

    let mut catalogs = Vec::new();
    let entries = match std::fs::read_dir(&po_dir) {
        Ok(entries) => entries,
        Err(error) => {
            println!("cargo:warning=cannot read {}: {error}", po_dir.display());
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("po") {
            continue;
        }
        let Some(language) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        println!("cargo:rerun-if-changed={}", path.display());
        catalogs.push((language.to_string(), path));
    }
    catalogs.sort_by(|left, right| left.0.cmp(&right.0));

    for (language, source) in catalogs {
        let destination = locale_dir.join(&language).join("LC_MESSAGES");
        if let Err(error) = std::fs::create_dir_all(&destination) {
            panic!(
                "cannot create translation directory {}: {error}",
                destination.display()
            );
        }
        let output = destination.join("ferramari.mo");
        let result = Command::new(&msgfmt)
            .arg("--check")
            .arg("-c")
            .arg("-o")
            .arg(&output)
            .arg(&source)
            .output()
            .unwrap_or_else(|error| {
                panic!(
                    "cannot run {} for {}: {error}",
                    msgfmt.display(),
                    source.display()
                )
            });
        if !result.status.success() {
            panic!(
                "msgfmt failed for {}:\n{}",
                source.display(),
                String::from_utf8_lossy(&result.stderr).trim()
            );
        }
    }
}

/// `OUT_DIR` ends in `.../<profile>/build/<package>-<hash>/out`; the profile
/// directory is three ancestors up.
fn target_profile_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR")?);
    out_dir.ancestors().nth(3).map(Path::to_path_buf)
}

fn find_msgfmt() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("MSGFMT") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(path) = find_on_path("msgfmt") {
        return Some(path);
    }
    #[cfg(windows)]
    {
        if let Some(path) = find_on_path("msgfmt.exe") {
            return Some(path);
        }
        let mut candidates = Vec::new();
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            // The mlocati gettext build installs here when winget is not
            // elevated; its bin directory is not always on the session PATH.
            candidates.push(
                PathBuf::from(local_app_data)
                    .join("Programs")
                    .join("gettext-iconv")
                    .join("bin")
                    .join("msgfmt.exe"),
            );
        }
        for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(program_files) = std::env::var_os(variable) {
                candidates.push(
                    PathBuf::from(program_files)
                        .join("gettext-iconv")
                        .join("bin")
                        .join("msgfmt.exe"),
                );
            }
        }
        candidates.into_iter().find(|path| path.is_file())
    }
    #[cfg(not(windows))]
    {
        ["/usr/bin/msgfmt", "/usr/local/bin/msgfmt"]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.is_file())
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}
