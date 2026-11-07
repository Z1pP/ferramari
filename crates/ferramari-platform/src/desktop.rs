//! Desktop/session detection and the `--doctor` report.
//!
//! Ferramari deliberately supports X11 and XWayland only for the wallpaper
//! surface. This module reports that fact instead of guessing: a Wayland
//! session is labelled as XWayland when `DISPLAY` is present, and native
//! Wayland is explicitly unsupported.

use std::env;
use std::fmt::Write as _;
use std::process::Command;
use std::time::Duration;

use x11rb::connection::Connection as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    X11,
    WaylandXWayland,
    WaylandNoDisplay,
    Unknown,
}

impl SessionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X11 => "X11",
            Self::WaylandXWayland => "Wayland (XWayland)",
            Self::WaylandNoDisplay => "Wayland (no X11 display)",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    Gnome,
    Kde,
    Xfce,
    Hyprland,
    Other,
}

impl Desktop {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gnome => "GNOME",
            Self::Kde => "KDE Plasma",
            Self::Xfce => "XFCE",
            Self::Hyprland => "Hyprland",
            Self::Other => "other/unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// A desktop-type X11 toplevel can be created.
    pub desktop_window: bool,
    /// Per-monitor occlusion and smart pause are available for X11 windows.
    /// On an XWayland session this only covers X11 clients: a native Wayland
    /// window is not in `_NET_CLIENT_LIST` and cannot be detected.
    pub per_monitor_occlusion: bool,
    /// Native Wayland surfaces are implemented.
    pub native_wayland: bool,
    /// GNOME's static wallpaper backend is available.
    pub static_wallpaper: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Environment {
    pub session: SessionKind,
    pub desktop: Desktop,
    pub capabilities: Capabilities,
}

pub fn detect() -> Environment {
    let session = classify_session(
        &env::var("XDG_SESSION_TYPE").unwrap_or_default(),
        env::var("DISPLAY").ok().as_deref(),
        env::var("WAYLAND_DISPLAY").ok().as_deref(),
    );
    let desktop = classify_desktop(&env::var("XDG_CURRENT_DESKTOP").unwrap_or_default());
    let capabilities = Capabilities {
        desktop_window: matches!(session, SessionKind::X11 | SessionKind::WaylandXWayland),
        // The EWMH watcher needs an X server, and the app always runs as an
        // X11/XWayland client, so occlusion works on both kinds of session.
        // On Wayland it sees X11 clients only: a native Wayland fullscreen
        // window is not in _NET_CLIENT_LIST and cannot be detected.
        per_monitor_occlusion: matches!(session, SessionKind::X11 | SessionKind::WaylandXWayland),
        native_wayland: false,
        static_wallpaper: desktop == Desktop::Gnome,
    };
    Environment {
        session,
        desktop,
        capabilities,
    }
}

pub fn classify_session(
    session_type: &str,
    display: Option<&str>,
    wayland_display: Option<&str>,
) -> SessionKind {
    let is_wayland = session_type.eq_ignore_ascii_case("wayland")
        || wayland_display.is_some_and(|value| !value.is_empty());
    let has_x11 = display.is_some_and(|value| !value.is_empty());
    match (is_wayland, has_x11) {
        (false, true) => SessionKind::X11,
        (true, true) => SessionKind::WaylandXWayland,
        (true, false) => SessionKind::WaylandNoDisplay,
        (false, false) => SessionKind::Unknown,
    }
}

pub fn classify_desktop(current_desktop: &str) -> Desktop {
    let value = current_desktop.to_ascii_lowercase();
    if value.contains("gnome") {
        Desktop::Gnome
    } else if value.contains("kde") || value.contains("plasma") {
        Desktop::Kde
    } else if value.contains("xfce") {
        Desktop::Xfce
    } else if value.contains("hyprland") {
        Desktop::Hyprland
    } else {
        Desktop::Other
    }
}

impl Environment {
    pub fn summary(&self) -> String {
        format!(
            "{} · {} · X11 backend · native Wayland: no",
            self.session.as_str(),
            self.desktop.as_str()
        )
    }
}

/// Human-readable diagnostics. Safe to call without GTK or a D-Bus session;
/// every probe degrades to a missing line instead of failing.
pub fn doctor() -> String {
    let environment = detect();
    let mut report = String::new();
    let _ = writeln!(report, "Ferramari {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(report, "session: {}", environment.session.as_str());
    let _ = writeln!(report, "desktop: {}", environment.desktop.as_str());
    let _ = writeln!(
        report,
        "DISPLAY: {}",
        env::var("DISPLAY").unwrap_or_else(|_| "unset".to_string())
    );
    let _ = writeln!(
        report,
        "WAYLAND_DISPLAY: {}",
        env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "unset".to_string())
    );
    let _ = writeln!(
        report,
        "backend: x11 (forced; XWayland on Wayland sessions)"
    );
    let _ = writeln!(
        report,
        "desktop window: {}",
        yes_no(environment.capabilities.desktop_window)
    );
    let _ = writeln!(
        report,
        "per-monitor occlusion: {}",
        yes_no(environment.capabilities.per_monitor_occlusion)
    );
    let _ = writeln!(
        report,
        "native Wayland: {}",
        yes_no(environment.capabilities.native_wayland)
    );
    let _ = writeln!(
        report,
        "GNOME static wallpaper: {}",
        yes_no(environment.capabilities.static_wallpaper)
    );
    let _ = writeln!(
        report,
        "background: {}",
        crate::gnome_bg::background_diagnostic()
    );
    let _ = writeln!(report, "X11 screens: {}", x11_screen_count());
    let _ = writeln!(report, "mpv: {}", tool_version("mpv", &["--version"]));
    let _ = writeln!(report, "ffmpeg: {}", tool_version("ffmpeg", &["-version"]));
    let _ = writeln!(
        report,
        "ffprobe: {}",
        tool_version("ffprobe", &["-version"])
    );
    let _ = writeln!(report, "yt-dlp: {}", tool_version("yt-dlp", &["--version"]));
    let _ = writeln!(report, "vainfo: {}", tool_version("vainfo", &[]));
    let _ = writeln!(report, "render nodes: {}", render_nodes());
    let _ = writeln!(
        report,
        "idle service: {}",
        first_owned_session_service(&[
            "org.gnome.Mutter.IdleMonitor",
            "org.freedesktop.ScreenSaver",
            "org.kde.screensaver",
            "org.xfce.ScreenSaver",
        ])
    );
    let _ = writeln!(report, "lock services: {}", lock_services());
    let plugins = ferramari_core::plugin::list();
    let _ = writeln!(
        report,
        "plugin sources: {}",
        if plugins.is_empty() {
            "none".to_string()
        } else {
            plugins
                .iter()
                .map(|plugin| {
                    format!(
                        "{} ({}) -> {}",
                        plugin.manifest.id, plugin.manifest.name, plugin.manifest.api
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    report
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn tool_version(program: &str, arguments: &[&str]) -> String {
    let mut command = Command::new(ferramari_core::paths::tool(program));
    command
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    match ferramari_core::util::output_with_timeout(&mut command, Duration::from_secs(3)) {
        Some(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let line = stdout
                .lines()
                .chain(stderr.lines())
                .find(|line| !line.trim().is_empty())
                .unwrap_or("installed")
                .trim();
            line.to_string()
        }
        _ => "not found".to_string(),
    }
}

fn x11_screen_count() -> String {
    match x11rb::connect(None) {
        Ok((connection, screen)) => {
            let screens = connection.setup().roots.len();
            format!("{screens} (current screen {screen})")
        }
        Err(err) => format!("unavailable ({err})"),
    }
}

fn render_nodes() -> String {
    let Ok(entries) = std::fs::read_dir("/dev/dri") else {
        return "none".to_string();
    };
    let mut nodes = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("renderD") {
            continue;
        }
        let driver = std::fs::canonicalize(format!("/sys/class/drm/{name}/device/driver"))
            .ok()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "unknown".to_string());
        nodes.push(format!("{name} ({driver})"));
    }
    if nodes.is_empty() {
        "none".to_string()
    } else {
        nodes.join(", ")
    }
}

fn first_owned_session_service(names: &[&str]) -> String {
    let Ok(connection) = zbus::blocking::Connection::session() else {
        return "session bus unavailable".to_string();
    };
    for name in names {
        if service_owned(&connection, name) {
            return format!("{name} (available)");
        }
    }
    "not found".to_string()
}

fn lock_services() -> String {
    let Ok(session) = zbus::blocking::Connection::session() else {
        return "session bus unavailable".to_string();
    };
    let mut available = Vec::new();
    for name in [
        "org.gnome.ScreenSaver",
        "org.cinnamon.ScreenSaver",
        "org.freedesktop.ScreenSaver",
    ] {
        if service_owned(&session, name) {
            available.push(name);
        }
    }
    if let Ok(system) = zbus::blocking::Connection::system() {
        if service_owned(&system, "org.freedesktop.login1") {
            available.push("org.freedesktop.login1");
        }
    }
    if available.is_empty() {
        "not found".to_string()
    } else {
        available.join(", ")
    }
}

fn service_owned(connection: &zbus::blocking::Connection, name: &str) -> bool {
    connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &(name,),
        )
        .ok()
        .and_then(|reply| reply.body().deserialize::<bool>().ok())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_classification_is_explicit_about_xwayland() {
        assert_eq!(classify_session("x11", Some(":0"), None), SessionKind::X11);
        assert_eq!(
            classify_session("wayland", Some(":0"), Some("wayland-0")),
            SessionKind::WaylandXWayland
        );
        assert_eq!(
            classify_session("wayland", None, Some("wayland-0")),
            SessionKind::WaylandNoDisplay
        );
        assert_eq!(classify_session("tty", None, None), SessionKind::Unknown);
    }

    #[test]
    fn desktop_names_are_case_insensitive() {
        assert_eq!(classify_desktop("ubuntu:GNOME"), Desktop::Gnome);
        assert_eq!(classify_desktop("KDE"), Desktop::Kde);
        assert_eq!(classify_desktop("XFCE"), Desktop::Xfce);
        assert_eq!(classify_desktop("Hyprland"), Desktop::Hyprland);
        assert_eq!(classify_desktop("sway"), Desktop::Other);
    }

    #[test]
    fn native_wayland_is_never_claimed() {
        let environment = detect();
        assert!(!environment.capabilities.native_wayland);
    }

    #[test]
    fn occlusion_tracks_the_x11_capability() {
        // The watcher needs an X server. Both X11 and XWayland sessions have
        // one, so occlusion is offered there; a Wayland session without
        // DISPLAY has nothing to query.
        for (session_type, display, wayland, expected) in [
            ("x11", Some(":0"), None, true),
            ("wayland", Some(":0"), Some("wayland-0"), true),
            ("wayland", None, Some("wayland-0"), false),
        ] {
            let session = classify_session(session_type, display, wayland);
            let occlusion = matches!(session, SessionKind::X11 | SessionKind::WaylandXWayland);
            assert_eq!(occlusion, expected, "session {session:?}");
        }
    }
}
