//! Session integrations that only produce events: screen lock and logout.

use zbus::blocking::{Connection, Proxy};

use crate::app::{Command, CommandSender};
use crate::paths;

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

/// Pause playback while the machine is suspended. logind emits `true` right
/// before sleep and `false` after resume.
pub fn start_sleep_watcher(tx: CommandSender) {
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
    let Ok(signals) = proxy.receive_signal("PrepareForSleep") else {
        return;
    };
    std::thread::Builder::new()
        .name("ferramari-sleep".to_string())
        .spawn(move || {
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
    if paths::is_gnome() {
        start_gnome_session_watcher(tx);
    } else {
        start_logind_watcher(tx);
    }
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
    std::thread::spawn(move || {
        for message in signals {
            if message.body().deserialize::<bool>().unwrap_or(false) {
                let _ = tx.send(Command::Quit);
            }
        }
    });
}
