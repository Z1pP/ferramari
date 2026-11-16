//! Windows desktop detection and the `--doctor` report.
//!
//! The server draws behind Explorer's desktop icons; there is no second
//! session type to classify. The report says whether the Explorer wallpaper
//! host is reachable, what the display layout looks like, and which external
//! tools are installed, so a "no wallpaper" report can be answered without a
//! debugger.

use std::env;
use std::fmt::Write as _;
use std::process::Command;
use std::time::Duration;

use ferramari_core::platform::{PlatformCapabilities, SupportLevel};

/// Opt the server into per-monitor DPI awareness before it enumerates
/// displays or creates a window. See `win32util::init_dpi_awareness`.
pub fn init_dpi_awareness() {
    crate::win32util::init_dpi_awareness();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Windows,
    Unknown,
}

impl SessionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "Windows desktop",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    Explorer,
    Other,
}

impl Desktop {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explorer => "Explorer",
            Self::Other => "other/unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// A window can be attached behind Explorer's desktop icons.
    pub desktop_window: bool,
    /// Per-monitor occlusion and smart pause are available for top-level
    /// windows.
    pub per_monitor_occlusion: bool,
    /// Kept for API parity with the Linux report; always false here.
    pub native_wayland: bool,
    /// `SystemParametersInfoW` can replace and restore the desktop image.
    pub static_wallpaper: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Environment {
    pub session: SessionKind,
    pub desktop: Desktop,
    pub capabilities: Capabilities,
}

pub fn detect() -> Environment {
    let explorer = crate::surface::available();
    let monitors_reported = !crate::monitors::list().is_empty();
    Environment {
        session: SessionKind::Windows,
        desktop: if explorer {
            Desktop::Explorer
        } else {
            Desktop::Other
        },
        capabilities: Capabilities {
            desktop_window: explorer,
            per_monitor_occlusion: monitors_reported,
            native_wayland: false,
            static_wallpaper: true,
        },
    }
}

impl Environment {
    pub fn summary(&self) -> String {
        format!(
            "{} · {} · Win32 backend · Explorer restarts supported",
            self.session.as_str(),
            self.desktop.as_str()
        )
    }
}

pub fn platform_capabilities() -> PlatformCapabilities {
    let environment = detect();
    capabilities_for(
        environment.capabilities.desktop_window,
        environment.capabilities.per_monitor_occlusion,
    )
}

/// The level decision, separated from the probes so a CI machine without an
/// interactive desktop still covers the rules.
fn capabilities_for(explorer_available: bool, monitors_reported: bool) -> PlatformCapabilities {
    PlatformCapabilities {
        wallpaper_surface: if explorer_available {
            SupportLevel::Supported
        } else {
            SupportLevel::Unsupported
        },
        display_enumeration: if monitors_reported {
            SupportLevel::Supported
        } else {
            SupportLevel::Unsupported
        },
        // WM_DISPLAYCHANGE, WTS session notifications and the power broadcasts
        // are available on every supported Windows version.
        display_hotplug: SupportLevel::Supported,
        window_tracking: SupportLevel::Supported,
        session_lock: SupportLevel::Supported,
        power_events: SupportLevel::Supported,
        idle_detection: SupportLevel::Supported,
    }
}

pub fn doctor() -> String {
    let environment = detect();
    let capabilities = platform_capabilities();
    let monitors = crate::monitors::list();
    let mut report = String::new();
    let _ = writeln!(report, "Ferramari {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(report, "session: {}", environment.session.as_str());
    let _ = writeln!(report, "desktop: {}", environment.desktop.as_str());
    let _ = writeln!(
        report,
        "OS: {} ({})",
        env::consts::OS,
        env::var("PROCESSOR_ARCHITECTURE").unwrap_or_else(|_| "unknown".to_string())
    );
    let _ = writeln!(
        report,
        "backend: win32 (WorkerW wallpaper host behind Explorer's icons)"
    );
    let _ = writeln!(
        report,
        "Explorer wallpaper host: {}",
        yes_no(crate::surface::available())
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
        "static wallpaper: {}",
        yes_no(environment.capabilities.static_wallpaper)
    );
    let _ = writeln!(
        report,
        "background: {}",
        crate::wallpaper::background_diagnostic()
    );
    let _ = writeln!(
        report,
        "capabilities: wallpaper={}, displays={}, hotplug={}, window tracking={}, lock={}, power={}, idle={}",
        support_name(capabilities.wallpaper_surface),
        support_name(capabilities.display_enumeration),
        support_name(capabilities.display_hotplug),
        support_name(capabilities.window_tracking),
        support_name(capabilities.session_lock),
        support_name(capabilities.power_events),
        support_name(capabilities.idle_detection),
    );
    if capabilities.wallpaper_surface != SupportLevel::Supported {
        let _ = writeln!(
            report,
            "  - wallpaper surface: unsupported (Progman was not found; is Explorer running?)"
        );
    }
    if capabilities.display_enumeration != SupportLevel::Supported {
        let _ = writeln!(
            report,
            "  - display enumeration: unsupported (no display reported a usable rectangle)"
        );
    }
    let _ = writeln!(report, "monitors: {}", monitors.len());
    let _ = writeln!(report, "display identities: {}", display_report(&monitors));
    let _ = writeln!(report, "mpv: {}", tool_version("mpv", &["--version"]));
    let _ = writeln!(report, "ffmpeg: {}", tool_version("ffmpeg", &["-version"]));
    let _ = writeln!(
        report,
        "ffprobe: {}",
        tool_version("ffprobe", &["-version"])
    );
    let _ = writeln!(report, "yt-dlp: {}", tool_version("yt-dlp", &["--version"]));
    let _ = writeln!(report, "hardware decode: {}", hwdec_report());
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

fn support_name(level: SupportLevel) -> &'static str {
    match level {
        SupportLevel::Supported => "supported",
        SupportLevel::Partial => "partial",
        SupportLevel::Unsupported => "unsupported",
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

/// Per-display identity summary: the config key each monitor owns and whether
/// it is already remembered. The device interface string survives a cable
/// swap; the `DISPLAY1` connector name does not.
fn display_report(monitors: &[ferramari_core::MonitorInfo]) -> String {
    let mut registry = ferramari_core::display_ids::DisplayIdRegistry::load();
    let mut monitors = monitors.to_vec();
    if monitors.is_empty() {
        return if registry.is_empty() {
            "none".to_string()
        } else {
            format!("{} remembered, none connected", registry.records.len())
        };
    }
    let known: Vec<String> = registry
        .records
        .iter()
        .map(|record| record.stable_id.clone())
        .collect();
    registry.bind(&mut monitors);
    let mut lines = Vec::new();
    for monitor in &monitors {
        let identity = match (
            monitor.identity.native_id.as_ref(),
            monitor.identity.connector.as_ref(),
        ) {
            (Some(id), _) => format!("device {id}"),
            (_, Some(connector)) => format!("connector {connector}"),
            _ => "no identity".to_string(),
        };
        let remembered = monitor.stable_id().is_some_and(|id| known.contains(&id));
        lines.push(format!(
            "{} [{identity}] ({})",
            monitor.name,
            if remembered {
                "key remembered"
            } else {
                "new screen"
            }
        ));
    }
    lines.join("; ")
}

/// Hardware decoding as configured and as the running server reports it.
/// `mpv --hwdec=help` lists the APIs mpv was built with, which says nothing
/// about the attached GPU; only the live player reports what it decodes with.
fn hwdec_report() -> String {
    let requested = requested_hwdec();
    let mut parts = vec![format!(
        "requested {requested} -> --hwdec={}",
        ferramari_core::hwdec::hwdec_arg(&requested)
    )];
    match mpv_hwdec_apis() {
        Some(apis) => parts.push(format!("mpv built with: {apis}")),
        None => parts.push("mpv API list unavailable".to_string()),
    }
    parts.push(match crate::dbus::hwdec_running() {
        Some(running) if !running.is_empty() => {
            let current = ferramari_core::hwdec::HwDec::from_report(Some(running.as_str()));
            let verdict = ferramari_core::hwdec::HwDecCheck::evaluate(&requested, &current);
            if verdict.is_silent() {
                format!("decoding as {running}")
            } else {
                format!("decoding as {running}: {}", verdict.message())
            }
        }
        _ => "not decoding".to_string(),
    });
    parts.join("; ")
}

fn requested_hwdec() -> String {
    let Ok(text) = std::fs::read_to_string(ferramari_core::paths::config_path()) else {
        return ferramari_core::hwdec::HWDEC_AUTO.to_string();
    };
    let names: Vec<String> = crate::monitors::list()
        .into_iter()
        .map(|monitor| monitor.name)
        .collect();
    ferramari_core::config::Config::parse(&text, &names).0.hwdec
}

fn mpv_hwdec_apis() -> Option<String> {
    let mut command = Command::new(ferramari_core::paths::tool("mpv"));
    command
        .args(["--no-config", "--hwdec=help"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = ferramari_core::util::output_with_timeout(&mut command, Duration::from_secs(3))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_hwdec_apis(&stderr).or_else(|| parse_hwdec_apis(&stdout))
}

fn parse_hwdec_apis(text: &str) -> Option<String> {
    let mut apis: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((api, rest)) = line.split_once(' ') else {
            continue;
        };
        if !rest.starts_with('(') || line.contains(':') || api.contains('(') {
            continue;
        }
        if api == "auto" || api.starts_with("auto-") {
            continue;
        }
        if !apis.iter().any(|seen| seen == api) {
            apis.push(api.to_string());
        }
    }
    (!apis.is_empty()).then(|| apis.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explorer_session_reports_every_capability() {
        let levels = capabilities_for(true, true);
        for level in [
            levels.wallpaper_surface,
            levels.display_enumeration,
            levels.display_hotplug,
            levels.window_tracking,
            levels.session_lock,
            levels.power_events,
            levels.idle_detection,
        ] {
            assert_eq!(level, SupportLevel::Supported);
        }
    }

    #[test]
    fn without_explorer_the_surface_is_explicitly_unsupported() {
        let levels = capabilities_for(false, true);
        assert_eq!(levels.wallpaper_surface, SupportLevel::Unsupported);
        // Display and window APIs still work; only the host is missing.
        assert_eq!(levels.display_enumeration, SupportLevel::Supported);
        assert_eq!(levels.window_tracking, SupportLevel::Supported);
    }

    #[test]
    fn an_empty_layout_is_not_enumerable() {
        let levels = capabilities_for(true, false);
        assert_eq!(levels.display_enumeration, SupportLevel::Unsupported);
        assert_eq!(levels.wallpaper_surface, SupportLevel::Supported);
    }

    #[test]
    fn the_hwdec_help_list_keeps_only_real_apis() {
        let text = "Valid values (with alternative full names):\n  \
                    d3d11va (h264-d3d11va)\n  d3d11va (hevc-d3d11va)\n  \
                    dxva2 (h264-dxva2)\n  auto (yes '')\n  no\n  auto-safe\n";
        assert_eq!(parse_hwdec_apis(text).unwrap(), "d3d11va,dxva2");
        assert_eq!(parse_hwdec_apis(""), None);
    }

    #[test]
    fn the_live_report_does_not_panic() {
        let environment = detect();
        assert_eq!(environment.session, SessionKind::Windows);
        let _ = environment.summary();
        let _ = platform_capabilities();
    }

    #[test]
    fn tools_that_are_missing_report_not_found() {
        assert_eq!(
            tool_version("ferramari-definitely-missing-tool", &[]),
            "not found"
        );
    }
}
