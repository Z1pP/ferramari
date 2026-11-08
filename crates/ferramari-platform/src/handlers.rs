//! Session integrations that only produce events: screen lock, logout, sleep
//! and desktop idle time.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use zbus::blocking::{Connection, Proxy};

use ferramari_core::paths;
use ferramari_core::session::{IdleState, LockSource};
use ferramari_core::{Command, CommandSender};

/// Consecutive unanswered polls before the idle facility is declared gone.
/// A restarted desktop shell takes a few seconds to own its bus name again,
/// and flipping to `Unknown` (and therefore resuming playback) during that
/// gap would stutter the wallpaper.
const IDLE_MISSINGS_BEFORE_UNKNOWN: u8 = 4;

/// Idle polling rule, separated from the thread so it can be tested.
///
/// The watcher has to answer three questions on every poll: what the reading
/// means, whether that is news, and whether silence means a transient D-Bus
/// failure or a service that really went away. Getting the third one wrong
/// either stutters the wallpaper while a shell restarts or leaves it paused
/// forever behind a dead screensaver.
#[derive(Debug)]
pub struct IdlePoller {
    /// The last state handed to the command bus. `None` until the first one,
    /// so even an opening reading of `Unknown` reaches the app, which still
    /// thinks its default `Active` was a measured fact.
    reported: Option<IdleState>,
    misses: u8,
    miss_limit: u8,
}

impl Default for IdlePoller {
    /// The production starting point: nothing reported yet, and the miss
    /// tolerance the watcher polls with. A derived default would leave the
    /// limit at zero, which reports `Unknown` on the very first unanswered
    /// poll and makes a restarting shell stutter the wallpaper.
    fn default() -> Self {
        Self {
            reported: None,
            misses: 0,
            miss_limit: IDLE_MISSINGS_BEFORE_UNKNOWN,
        }
    }
}

impl IdlePoller {
    /// Feed one poll: `Some(millis)` is the idle counter, `None` is "the
    /// service did not answer". Returns the state to send, if anything
    /// changed.
    pub fn poll(&mut self, idle_millis: Option<u64>, threshold_millis: u64) -> Option<IdleState> {
        let state = match idle_millis {
            Some(millis) => {
                self.misses = 0;
                // The boundary itself belongs to core, so every backend
                // compares the same way.
                IdleState::from_millis(Some(millis), threshold_millis)
            }
            None => {
                self.misses = self.misses.saturating_add(1);
                if self.misses < self.miss_limit {
                    // Not news yet: keep the last reported state rather than
                    // flickering through `Unknown` during a shell restart.
                    return None;
                }
                IdleState::Unknown
            }
        };
        (self.reported != Some(state)).then(|| {
            self.reported = Some(state);
            state
        })
    }
}

/// Portable idle detection. There is no freedesktop signal for "the user has
/// been idle for N seconds", so this polls the desktop's idle counter. The
/// poll runs on its own thread and only turns into a command when the
/// threshold is crossed, keeping the command loop untouched.
pub fn start_idle_watcher(tx: CommandSender, threshold_secs: Arc<AtomicU64>) {
    std::thread::Builder::new()
        .name("ferramari-idle".to_string())
        .spawn(move || {
            let mut poller = IdlePoller::default();
            loop {
                std::thread::sleep(Duration::from_secs(15));
                let threshold_ms = threshold_secs.load(Ordering::Relaxed).saturating_mul(1000);
                if let Some(state) = poller.poll(session_idle_ms(), threshold_ms) {
                    if tx.send(Command::Idle(state)).is_err() {
                        break;
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
                    if tx
                        .send(Command::LockChanged {
                            source: LockSource::ScreenSaver,
                            active,
                        })
                        .is_err()
                    {
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
                            if tx
                                .send(Command::LockChanged {
                                    source: LockSource::SessionManager,
                                    active: true,
                                })
                                .is_err()
                            {
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
                            if unlock_tx
                                .send(Command::LockChanged {
                                    source: LockSource::SessionManager,
                                    active: false,
                                })
                                .is_err()
                            {
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
/// seconds when the bus is slow or absent, and startup must not stall on
/// it.
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
    // command loop so a missing/slow bus cannot delay the first frame.
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

    fn new_poller() -> IdlePoller {
        IdlePoller::default()
    }

    #[test]
    fn the_first_reading_always_reaches_the_app() {
        // The app starts from `Active` as a default, not a measurement. If the
        // service is already idle at startup, or already gone, the app has to
        // be told; swallowing the opening reading would hide both.
        let mut poller = new_poller();
        assert_eq!(poller.poll(Some(600_000), 300_000), Some(IdleState::Idle));

        let mut poller = new_poller();
        assert!(poller.poll(None, 300_000).is_none(), "one miss is not news");
        for _ in 0..10 {
            if let Some(state) = poller.poll(None, 300_000) {
                assert_eq!(state, IdleState::Unknown);
                break;
            }
        }
        assert_eq!(poller.reported, Some(IdleState::Unknown));
    }

    #[test]
    fn a_restarted_shell_does_not_flicker_through_unknown() {
        // A few missed polls while GNOME Shell reacquires its bus name must
        // not pause and resume the wallpaper: the last reported state stands.
        let mut poller = new_poller();
        assert_eq!(poller.poll(Some(10_000), 300_000), Some(IdleState::Active));
        for _ in 1..IDLE_MISSINGS_BEFORE_UNKNOWN {
            assert_eq!(poller.poll(None, 300_000), None, "still tolerating misses");
        }
        // The service answers again before the limit; nothing new is sent.
        assert_eq!(poller.poll(Some(10_000), 300_000), None);
        assert_eq!(poller.misses, 0, "an answer clears the miss counter");
    }

    #[test]
    fn a_service_that_stays_gone_is_reported_once_as_unknown() {
        let mut poller = new_poller();
        assert_eq!(poller.poll(Some(10_000), 300_000), Some(IdleState::Active));
        let mut sent = Vec::new();
        for _ in 0..20 {
            if let Some(state) = poller.poll(None, 300_000) {
                sent.push(state);
            }
        }
        assert_eq!(sent, vec![IdleState::Unknown], "exactly one Unknown");
    }

    #[test]
    fn idle_and_active_edges_are_reported_once_each() {
        let mut poller = new_poller();
        assert_eq!(poller.poll(Some(10_000), 300_000), Some(IdleState::Active));
        assert_eq!(
            poller.poll(Some(20_000), 300_000),
            None,
            "same state, no spam"
        );
        assert_eq!(poller.poll(Some(400_000), 300_000), Some(IdleState::Idle));
        assert_eq!(poller.poll(Some(900_000), 300_000), None);
        assert_eq!(poller.poll(Some(1_000), 300_000), Some(IdleState::Active));
    }

    #[test]
    fn a_raised_threshold_can_move_a_stale_reading_back_to_active() {
        // The threshold is read from a shared atomic every poll, so changing it
        // in Settings must be able to flip the conclusion without a new event.
        let mut poller = new_poller();
        assert_eq!(poller.poll(Some(400_000), 300_000), Some(IdleState::Idle));
        assert_eq!(poller.poll(Some(400_000), 600_000), Some(IdleState::Active));
    }

    #[test]
    fn idle_probe_does_not_panic_on_any_desktop() {
        // A real session returns milliseconds; a headless CI session returns
        // None. Either way the probe must not panic or hang.
        if let Some(milliseconds) = session_idle_ms() {
            assert!(milliseconds < 86_400_000, "{milliseconds}");
        }
    }
}
