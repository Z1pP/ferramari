//! Session integrations that only produce events: screen lock, logout, sleep
//! and desktop idle time.
//!
//! D-Bus signal iterators block. A watcher therefore keeps a clone of its
//! connection in a shared slot; dropping the watcher closes that connection,
//! which wakes the iterator. All watchers are then joined under one deadline,
//! and a thread that still refuses to exit is detached instead of holding the
//! caller indefinitely. Closing the connection is the only reliable wake-up
//! for the blocking zbus API; a stop flag alone would leave the thread asleep
//! until the next signal.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use zbus::blocking::{Connection, Proxy};

use ferramari_core::paths;
use ferramari_core::session::{IdleState, LockSource};
use ferramari_core::{Command, CommandSender};

/// Consecutive unanswered polls before the idle facility is declared gone.
/// A restarted desktop shell takes a few seconds to own its bus name again,
/// and flipping to `Unknown` (and therefore resuming playback) during that
/// gap would stutter the wallpaper.
const IDLE_MISSINGS_BEFORE_UNKNOWN: u8 = 4;

const IDLE_POLL_EVERY: Duration = Duration::from_secs(15);
const STOP_POLL_EVERY: Duration = Duration::from_millis(250);
const STOP_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

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

/// Owns every session watcher started by the Linux backend.
pub struct SessionWatchers {
    _dbus: Vec<DbusWatcher>,
    _idle: Option<IdleWatcher>,
}

impl SessionWatchers {
    pub fn start(tx: CommandSender, threshold_secs: Arc<AtomicU64>) -> Self {
        let mut dbus = Vec::new();
        dbus.extend(screen_lock_watchers(tx.clone()));
        dbus.extend(logind_lock_watchers(tx.clone()));
        dbus.extend(sleep_watcher(tx.clone()));
        dbus.extend(end_session_watchers(tx.clone()));
        let idle = start_idle_watcher(tx, threshold_secs);
        Self {
            _dbus: dbus,
            _idle: idle,
        }
    }
}

impl Drop for SessionWatchers {
    fn drop(&mut self) {
        // Two phases on purpose: ask every watcher to stop before waiting on
        // any one of them. Each close wakes one blocked iterator immediately,
        // so the whole teardown fits in a single deadline instead of one per
        // watcher.
        for watcher in &mut self._dbus {
            watcher.signal_stop();
        }
        if let Some(idle) = &mut self._idle {
            idle.signal_stop();
        }
        let deadline = Instant::now() + STOP_JOIN_TIMEOUT;
        for watcher in &mut self._dbus {
            watcher.join(deadline);
        }
        if let Some(idle) = &mut self._idle {
            idle.join(deadline);
        }
    }
}

struct DbusWatcher {
    name: String,
    stop: Arc<AtomicBool>,
    connection: Arc<Mutex<Option<zbus::Connection>>>,
    thread: Option<JoinHandle<()>>,
}

impl DbusWatcher {
    /// Ask the thread to finish and wake it if it is parked in a signal
    /// iterator.
    fn signal_stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut slot) = self.connection.lock() {
            if let Some(connection) = slot.take() {
                // Closing the connection is what releases a blocking signal
                // iterator; the stop flag alone would only be seen after the
                // next signal arrived.
                let _ = zbus::blocking::Connection::from(connection).close();
            }
        }
    }

    fn join(&mut self, deadline: Instant) {
        join_bounded(&self.name, &mut self.thread, deadline);
    }
}

impl Drop for DbusWatcher {
    fn drop(&mut self) {
        self.signal_stop();
        self.join(Instant::now() + STOP_JOIN_TIMEOUT);
    }
}

struct IdleWatcher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl IdleWatcher {
    fn signal_stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    fn join(&mut self, deadline: Instant) {
        join_bounded("ferramari-idle", &mut self.thread, deadline);
    }
}

impl Drop for IdleWatcher {
    fn drop(&mut self) {
        self.signal_stop();
        self.join(Instant::now() + STOP_JOIN_TIMEOUT);
    }
}

/// Join a watcher without letting a wedged D-Bus call hold shutdown forever.
/// If the deadline expires the handle is detached; the closed connection is
/// expected to make the thread exit on its own.
fn join_bounded(name: &str, thread: &mut Option<JoinHandle<()>>, deadline: Instant) {
    let Some(handle) = thread.take() else {
        return;
    };
    while !handle.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if handle.is_finished() {
        let _ = handle.join();
    } else {
        log::warn!("[Session] {name} did not stop before the shutdown deadline");
    }
}

/// Spawn a watcher whose bus connection is created on its worker thread.
/// D-Bus can block for seconds when the bus is slow or absent, and startup
/// must not stall on it.
fn spawn_dbus<F>(
    name: &str,
    connect: fn() -> zbus::Result<Connection>,
    setup: F,
) -> Option<DbusWatcher>
where
    F: FnOnce(&Connection, &AtomicBool) -> anyhow::Result<()> + Send + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    let connection = Arc::new(Mutex::new(None));
    let thread_stop = Arc::clone(&stop);
    let thread_connection = Arc::clone(&connection);
    let thread_name = name.to_string();
    let log_name = thread_name.clone();
    let thread = std::thread::Builder::new()
        .name(thread_name.clone())
        .spawn(move || {
            let connection = match connect() {
                Ok(connection) => connection,
                Err(err) => {
                    log::debug!("[Session] {log_name} could not connect: {err}");
                    return;
                }
            };
            if let Ok(mut slot) = thread_connection.lock() {
                *slot = Some(connection.inner().clone());
            }
            if thread_stop.load(Ordering::SeqCst) {
                return;
            }
            if let Err(err) = setup(&connection, &thread_stop) {
                log::debug!("[Session] {log_name} stopped: {err}");
            }
        })
        .ok()?;
    Some(DbusWatcher {
        name: thread_name,
        stop,
        connection,
        thread: Some(thread),
    })
}

/// Portable idle detection. There is no freedesktop signal for "the user has
/// been idle for N seconds", so this polls the desktop's idle counter. The
/// poll runs on its own thread and only turns into a command when the
/// threshold is crossed, keeping the command loop untouched.
fn start_idle_watcher(tx: CommandSender, threshold_secs: Arc<AtomicU64>) -> Option<IdleWatcher> {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("ferramari-idle".to_string())
        .spawn(move || {
            let mut poller = IdlePoller::default();
            while !thread_stop.load(Ordering::SeqCst) {
                let mut waited = Duration::ZERO;
                while waited < IDLE_POLL_EVERY {
                    if thread_stop.load(Ordering::SeqCst) {
                        return;
                    }
                    let slice = STOP_POLL_EVERY.min(IDLE_POLL_EVERY - waited);
                    std::thread::sleep(slice);
                    waited += slice;
                }
                let threshold_ms = threshold_secs.load(Ordering::Relaxed).saturating_mul(1000);
                if let Some(state) = poller.poll(session_idle_ms(), threshold_ms) {
                    if tx.send(Command::Idle(state)).is_err() {
                        return;
                    }
                }
            }
        })
        .ok()?;
    Some(IdleWatcher {
        stop,
        thread: Some(thread),
    })
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
/// its own connection and thread because a missing service must not stop the
/// others, and closing one while stopping must not disturb the rest.
fn screen_lock_watchers(tx: CommandSender) -> Vec<DbusWatcher> {
    let mut watchers = Vec::new();
    for service in [
        "org.gnome.ScreenSaver",
        "org.cinnamon.ScreenSaver",
        "org.freedesktop.ScreenSaver",
    ] {
        let tx = tx.clone();
        let name = format!(
            "ferramari-lock-{}",
            service.rsplit('.').next().unwrap_or("saver")
        );
        let setup = move |connection: &Connection, stop: &AtomicBool| {
            let path = format!("/{}", service.replace('.', "/"));
            let proxy = Proxy::new(connection, service, path.as_str(), service)?;
            let signals = proxy.receive_signal("ActiveChanged")?;
            log::debug!("[ScreenLock] Watching {service}");
            for message in signals {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
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
            Ok(())
        };
        if let Some(watcher) = spawn_dbus(&name, Connection::session, setup) {
            watchers.push(watcher);
        }
    }
    watchers
}

/// Watch the login session's lock/unlock signals as a fallback for
/// compositors that do not expose one of the screensaver services above.
/// Both sources are aggregated by the app, so a logind unlock cannot override
/// a still-active screensaver lock.
fn logind_lock_watchers(tx: CommandSender) -> Vec<DbusWatcher> {
    let mut watchers = Vec::new();
    for (member, active, name) in [
        ("Lock", true, "ferramari-logind-lock-on"),
        ("Unlock", false, "ferramari-logind-lock-off"),
    ] {
        let tx = tx.clone();
        let setup = move |connection: &Connection, stop: &AtomicBool| {
            let proxy = Proxy::new(
                connection,
                "org.freedesktop.login1",
                "/org/freedesktop/login1/session/auto",
                "org.freedesktop.login1.Session",
            )?;
            let signals = proxy.receive_signal(member)?;
            for _ in signals {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                if tx
                    .send(Command::LockChanged {
                        source: LockSource::SessionManager,
                        active,
                    })
                    .is_err()
                {
                    break;
                }
            }
            Ok(())
        };
        if let Some(watcher) = spawn_dbus(name, Connection::system, setup) {
            watchers.push(watcher);
        }
    }
    watchers
}

/// Pause playback while the machine is suspended. logind emits `true` right
/// before sleep and `false` after resume.
fn sleep_watcher(tx: CommandSender) -> Vec<DbusWatcher> {
    let mut watchers = Vec::new();
    let setup = move |connection: &Connection, stop: &AtomicBool| {
        let proxy = Proxy::new(
            connection,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )?;
        let signals = proxy.receive_signal("PrepareForSleep")?;
        for message in signals {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let Ok(sleeping) = message.body().deserialize::<bool>() else {
                continue;
            };
            if tx.send(Command::Suspend(sleeping)).is_err() {
                break;
            }
        }
        Ok(())
    };
    if let Some(watcher) = spawn_dbus("ferramari-sleep", Connection::system, setup) {
        watchers.push(watcher);
    }
    watchers
}

/// GNOME's session manager and logind expose different end-session protocols.
/// Pick one at startup; `paths::is_gnome()` already knows the desktop.
fn end_session_watchers(tx: CommandSender) -> Vec<DbusWatcher> {
    if paths::is_gnome() {
        gnome_session_watchers(tx)
    } else {
        logind_session_watchers(tx)
    }
}

/// GNOME expects clients to register, answer `QueryEndSession` immediately and
/// answer `EndSession` while quitting. Both arrive on the registered client's
/// private interface.
fn gnome_session_watchers(tx: CommandSender) -> Vec<DbusWatcher> {
    let setup = move |connection: &Connection, stop: &AtomicBool| {
        let manager = Proxy::new(
            connection,
            "org.gnome.SessionManager",
            "/org/gnome/SessionManager",
            "org.gnome.SessionManager",
        )?;
        let client_path: zbus::zvariant::OwnedObjectPath =
            manager.call("RegisterClient", &("ferramari", ""))?;
        // One client and one thread. Registering separately for the two
        // signals would make gnome-session see the app twice and answer the
        // same end-session query twice. A match rule for this client's
        // interface delivers both signals to a single iterator; the member
        // tells them apart.
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender("org.gnome.SessionManager")?
            .path(client_path.as_str())?
            .interface("org.gnome.SessionManager.ClientPrivate")?
            .build();
        let signals = zbus::blocking::MessageIterator::for_match_rule(rule, connection, Some(4))?;
        let client = Proxy::new_owned(
            connection.clone(),
            "org.gnome.SessionManager",
            client_path.to_string(),
            "org.gnome.SessionManager.ClientPrivate",
        )?;
        for message in signals {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let Ok(message) = message else {
                continue;
            };
            match message.header().member().map(|member| member.as_str()) {
                // QueryEndSession must be answered immediately, otherwise
                // the desktop waits for the full timeout before shutting
                // down.
                Some("QueryEndSession") => {
                    let _ = client.call::<_, _, ()>("EndSessionResponse", &(true, ""));
                }
                Some("EndSession") => {
                    // Answer before asking the app to quit: shutdown closes
                    // this connection, and the response must reach
                    // gnome-session first or the logout waits for its timeout.
                    let _ = client.call::<_, _, ()>("EndSessionResponse", &(true, ""));
                    let _ = tx.send(Command::Quit);
                }
                _ => {}
            }
        }
        Ok(())
    };
    spawn_dbus("ferramari-session", Connection::session, setup)
        .into_iter()
        .collect()
}

/// Non-GNOME sessions announce shutdown through logind's manager interface.
fn logind_session_watchers(tx: CommandSender) -> Vec<DbusWatcher> {
    let mut watchers = Vec::new();
    let setup = move |connection: &Connection, stop: &AtomicBool| {
        let proxy = Proxy::new(
            connection,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )?;
        let signals = proxy.receive_signal("PrepareForShutdown")?;
        for message in signals {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            if message.body().deserialize::<bool>().unwrap_or(false) {
                let _ = tx.send(Command::Quit);
            }
        }
        Ok(())
    };
    if let Some(watcher) = spawn_dbus("ferramari-session", Connection::system, setup) {
        watchers.push(watcher);
    }
    watchers
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

    #[test]
    fn the_idle_watcher_stops_without_waiting_for_a_poll() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let watcher = start_idle_watcher(CommandSender::new(tx), Arc::new(AtomicU64::new(300)))
            .expect("idle watcher thread");
        drop(watcher);
    }

    #[test]
    fn join_bounded_reaps_a_finished_thread() {
        let mut thread = Some(std::thread::spawn(|| {}));
        join_bounded("test", &mut thread, Instant::now() + Duration::from_secs(1));
        assert!(thread.is_none(), "the finished thread is consumed");
    }

    #[test]
    fn join_bounded_hands_the_thread_a_deadline() {
        // A watcher that ignores its closed connection must not hold the
        // caller past the deadline. The released thread finishes on its own
        // after the test stops waiting for it.
        let (release, wait) = std::sync::mpsc::channel();
        let mut thread = Some(std::thread::spawn(move || {
            let _ = wait.recv();
        }));
        let started = Instant::now();
        join_bounded("test", &mut thread, Instant::now());
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(thread.is_none(), "the wedged thread was detached");
        let _ = release.send(());
    }
}
