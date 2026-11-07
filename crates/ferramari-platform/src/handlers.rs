//! Session integrations that only produce events: screen lock, logout, sleep
//! and desktop idle time.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use zbus::blocking::{Connection, Proxy};

use ferramari_core::paths;
use ferramari_core::{Command, CommandSender};

/// Portable idle detection. There is no freedesktop signal for "the user has
/// been idle for N seconds", so this polls the desktop's idle counter. The
/// poll runs on its own thread and only turns into a command when the
/// threshold is crossed, keeping the GTK loop untouched.
pub fn start_idle_watcher(tx: CommandSender, threshold_secs: Arc<AtomicU64>) {
    std::thread::Builder::new()
        .name("ferramari-idle".to_string())
        .spawn(move || {
            let mut idle = false;
            let mut misses = 0u8;
            loop {
                std::thread::sleep(Duration::from_secs(15));
                let threshold_ms = threshold_secs.load(Ordering::Relaxed).saturating_mul(1000);
                match session_idle_ms() {
                    Some(milliseconds) => {
                        misses = 0;
                        let now = milliseconds >= threshold_ms;
                        if now != idle {
                            idle = now;
                            if tx.send(Command::Idle(now)).is_err() {
                                break;
                            }
                        }
                    }
                    None => {
                        // The screensaver service may vanish on logout or
                        // restart. Do not stay latched in Eco idle forever.
                        misses = misses.saturating_add(1);
                        if idle && misses >= 4 {
                            idle = false;
                            if tx.send(Command::Idle(false)).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        })
        .ok();
}

fn session_idle_ms() -> Option<u64> {
    let connection = Connection::session().ok()?;
    for (service, path, interface, method) in [
        (
            "org.gnome.Mutter.IdleMonitor",
            "/org/gnome/Mutter/IdleMonitor/Core",
            "org.gnome.Mutter.IdleMonitor",
            // Mutter spells it with a lower-case "t".
            "GetIdletime",
        ),
        (
            "org.freedesktop.ScreenSaver",
            "/org/freedesktop/ScreenSaver",
            "org.freedesktop.ScreenSaver",
            "GetSessionIdleTime",
        ),
        (
            "org.kde.screensaver",
            "/ScreenSaver",
            "org.freedesktop.ScreenSaver",
            "GetSessionIdleTime",
        ),
        (
            "org.xfce.ScreenSaver",
            "/org/xfce/ScreenSaver",
            "org.xfce.ScreenSaver",
            "GetSessionIdleTime",
        ),
    ] {
        let Ok(proxy) = Proxy::new(&connection, service, path, interface) else {
            continue;
        };
        // Mutter reports u64 milliseconds; the screensaver interfaces use
        // u32 seconds. Try the signed/unsigned shapes without guessing.
        if let Ok(milliseconds) = proxy.call::<_, _, u64>(method, &()) {
            return Some(milliseconds);
        }
        if let Ok(milliseconds) = proxy.call::<_, _, u32>(method, &()) {
            // ScreenSaver's value is already seconds, not milliseconds.
            return Some(u64::from(milliseconds) * 1000);
        }
    }
    None
}

/// Watch every screen-saver implementation we know about. Each service gets
/// its own thread because a missing service must not stop the others.
pub fn start_screen_lock_watcher(tx: CommandSender) {
    for service in [
        "org.gnome.ScreenSaver",
        "org.cinnamon.ScreenSaver",
        "org.freedesktop.ScreenSaver",
    ] {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name(format!(
                "ferramari-lock-{}",
                service.rsplit('.').next().unwrap_or("saver")
            ))
            .spawn(move || {
                let path = format!("/{}", service.replace('.', "/"));
                let Ok(connection) = Connection::session() else {
                    return;
                };
                let Ok(proxy) = Proxy::new(&connection, service, path.as_str(), service) else {
                    return;
                };
                let Ok(signals) = proxy.receive_signal("ActiveChanged") else {
                    return;
                };
                log::debug!("[ScreenLock] Watching {service}");
                for message in signals {
                    let Ok(active) = message.body().deserialize::<bool>() else {
                        continue;
                    };
                    if tx.send(Command::ScreenLockActive(active)).is_err() {
                        break;
                    }
                }
            })
            .ok();
    }
}

/// Watch the login session's lock/unlock signals as a fallback for
/// compositors that do not expose one of the screensaver services above.
/// Both sources are aggregated by the app, so a logind unlock cannot override
/// a still-active screensaver lock.
pub fn start_logind_lock_watcher(tx: CommandSender) {
    std::thread::Builder::new()
        .name("ferramari-logind-lock".to_string())
        .spawn(move || {
            let Ok(connection) = Connection::system() else {
                return;
            };
            let Ok(proxy) = Proxy::new(
                &connection,
                "org.freedesktop.login1",
                "/org/freedesktop/login1/session/auto",
                "org.freedesktop.login1.Session",
            ) else {
                return;
            };
            let unlock_proxy = proxy.clone();
            let unlock_tx = tx.clone();
            if let Ok(signals) = proxy.receive_signal("Lock") {
                std::thread::Builder::new()
                    .name("ferramari-logind-lock-on".to_string())
                    .spawn(move || {
                        for _ in signals {
                            if tx.send(Command::SessionLock(true)).is_err() {
                                break;
                            }
                        }
                    })
                    .ok();
            }
            if let Ok(signals) = unlock_proxy.receive_signal("Unlock") {
                std::thread::Builder::new()
                    .name("ferramari-logind-lock-off".to_string())
                    .spawn(move || {
                        for _ in signals {
                            if unlock_tx.send(Command::SessionLock(false)).is_err() {
                                break;
                            }
                        }
                    })
                    .ok();
            }
        })
        .ok();
}

/// Pause playback while the machine is suspended. logind emits `true` right
/// before sleep and `false` after resume.
///
/// The system-bus connection is created on the worker: D-Bus can block for
/// seconds when the bus is slow or absent, and startup runs on the GTK thread.
pub fn start_sleep_watcher(tx: CommandSender) {
    std::thread::Builder::new()
        .name("ferramari-sleep".to_string())
        .spawn(move || {
            let Ok(connection) = Connection::system() else {
                log::debug!("[Power] System bus unavailable; suspend watch disabled");
                return;
            };
            let Ok(proxy) = Proxy::new(
                &connection,
                "org.freedesktop.login1",
                "/org/freedesktop/login1",
                "org.freedesktop.login1.Manager",
            ) else {
                return;
            };
            let Ok(signals) = proxy.receive_signal("PrepareForSleep") else {
                return;
            };
            for message in signals {
                let Ok(sleeping) = message.body().deserialize::<bool>() else {
                    continue;
                };
                if tx.send(Command::Suspend(sleeping)).is_err() {
                    break;
                }
            }
        })
        .ok();
}

pub fn start_end_session_watcher(tx: CommandSender) {
    let gnome = paths::is_gnome();
    // Both venders register clients and query the session bus; do that off the
    // GTK thread so a missing/slow bus cannot delay the first frame.
    std::thread::Builder::new()
        .name("ferramari-session".to_string())
        .spawn(move || {
            if gnome {
                start_gnome_session_watcher(tx);
            } else {
                start_logind_watcher(tx);
            }
        })
        .ok();
}

fn start_gnome_session_watcher(tx: CommandSender) {
    let Ok(connection) = Connection::session() else {
        return;
    };
    let Ok(manager) = Proxy::new(
        &connection,
        "org.gnome.SessionManager",
        "/org/gnome/SessionManager",
        "org.gnome.SessionManager",
    ) else {
        return;
    };
    let client_path: zbus::zvariant::OwnedObjectPath =
        match manager.call("RegisterClient", &("ferramari", "")) {
            Ok(path) => path,
            Err(err) => {
                log::debug!("[EndSession] RegisterClient failed: {err}");
                return;
            }
        };
    let Ok(client) = Proxy::new_owned(
        connection.clone(),
        "org.gnome.SessionManager",
        client_path.to_string(),
        "org.gnome.SessionManager.ClientPrivate",
    ) else {
        return;
    };

    // QueryEndSession must be answered immediately, otherwise the desktop
    // waits for the full timeout before shutting down.
    let query_client = client.clone();
    if let Ok(signals) = query_client.receive_signal("QueryEndSession") {
        std::thread::spawn(move || {
            for _ in signals {
                let _ = query_client.call::<_, _, ()>("EndSessionResponse", &(true, ""));
            }
        });
    }

    let Ok(signals) = client.receive_signal("EndSession") else {
        return;
    };
    std::thread::spawn(move || {
        for _ in signals {
            let _ = tx.send(Command::Quit);
            let _ = client.call::<_, _, ()>("EndSessionResponse", &(true, ""));
        }
    });
}

fn start_logind_watcher(tx: CommandSender) {
    let Ok(connection) = Connection::system() else {
        return;
    };
    let Ok(proxy) = Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    ) else {
        return;
    };
    let Ok(signals) = proxy.receive_signal("PrepareForShutdown") else {
        return;
    };
    for message in signals {
        if message.body().deserialize::<bool>().unwrap_or(false) {
            let _ = tx.send(Command::Quit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_probe_does_not_panic_on_any_desktop() {
        // A real session returns milliseconds; a headless CI session returns
        // None. Either way the probe must not panic or hang.
        if let Some(milliseconds) = session_idle_ms() {
            assert!(milliseconds < 86_400_000, "{milliseconds}");
        }
    }
}
