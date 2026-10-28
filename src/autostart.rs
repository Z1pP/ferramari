//! Autostart via `~/.config/autostart`. In Flatpak the XDG background portal
//! is asked as well, and the desktop file acts as a fallback.

use std::fs;

use crate::paths;

fn desktop_content(flatpak: bool, exe: &std::path::Path) -> String {
    if flatpak {
        format!(
            "[Desktop Entry]\n\
             Name=Ferramari\n\
             Exec=/usr/bin/flatpak run --command=ferramari {id} -b\n\
             Icon={id}\n\
             Terminal=false\n\
             Type=Application\n\
             Categories=GTK;Utility;\n\
             StartupNotify=true\n\
             X-Flatpak={id}\n",
            id = paths::APP_ID
        )
    } else {
        format!(
            "[Desktop Entry]\n\
             Name=Ferramari\n\
             Exec={exe} -b\n\
             Icon={id}\n\
             Terminal=false\n\
             Type=Application\n\
             Categories=GTK;Utility;\n\
             StartupNotify=true\n",
            exe = exe.display(),
            id = paths::APP_ID
        )
    }
}

pub fn is_enabled() -> bool {
    paths::autostart_path().is_file()
}

pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    if paths::is_flatpak() {
        if let Err(err) = request_portal_background(enabled) {
            log::debug!("[Autostart] Background portal request failed: {err}");
        }
    }
    let path = paths::autostart_path();
    if enabled {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("ferramari"));
        fs::write(&path, desktop_content(paths::is_flatpak(), &exe))?;
        log::info!("[Autostart] Enabled ({})", path.display());
    } else {
        match fs::remove_file(&path) {
            Ok(()) => log::info!("[Autostart] Disabled"),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

fn request_portal_background(enabled: bool) -> anyhow::Result<()> {
    use std::collections::HashMap;
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::Value;

    let connection = Connection::session()?;
    let proxy = Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Background",
    )?;

    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert(
        "handle_token",
        Value::from(format!("ferramari_{}", std::process::id())),
    );
    options.insert("commandline", Value::from(vec!["ferramari", "-b"]));
    options.insert("flags", Value::from(if enabled { 2u32 } else { 0u32 }));

    // The reply is a request handle; the result arrives asynchronously over
    // the portal's Request interface and is not needed for correctness.
    let _: zbus::zvariant::OwnedObjectPath = proxy.call(
        "RequestBackground",
        &("", "Autostart Ferramari in background", options),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_has_exec_and_icon() {
        let content = desktop_content(false, std::path::Path::new("/usr/bin/ferramari"));
        assert!(content.starts_with("[Desktop Entry]"));
        assert!(content.contains("Exec=/usr/bin/ferramari -b"));
        assert!(content.contains("Icon=io.github.ferramari.Ferramari"));
        assert!(!content.contains("X-Flatpak"));
    }

    #[test]
    fn flatpak_entry_uses_flatpak_spawn() {
        let content = desktop_content(true, std::path::Path::new("/usr/bin/ferramari"));
        assert!(content.contains("flatpak run --command=ferramari"));
        assert!(content.contains("X-Flatpak=io.github.ferramari.Ferramari"));
    }
}
