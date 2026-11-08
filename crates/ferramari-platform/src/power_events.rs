//! Real-time charger plug / unplug detection via sysfs inotify.
//!
//! Linux exposes the AC adapter state as `/sys/class/power_supply/<name>/online`
//! for every supply of type `Mains`, and writing to that attribute is what the
//! kernel does when a charger is plugged or unplugged. Watching the file with
//! inotify turns that into an immediate event, instead of waiting for the
//! 30-second `PowerProvider::status()` poll that Eco policy falls back on.
//!
//! The watcher deliberately never blocks: the inotify descriptor is
//! non-blocking (`inotify_init1(IN_NONBLOCK)`), so the thread drains what is
//! there, sleeps briefly, and always re-reads the attribute rather than
//! trusting event payloads — sysfs coalesces and re-exports attributes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ferramari_core::bus::{Command, CommandSender};
use ferramari_core::power::PowerEventType;

/// Cadence of the safety re-read when no inotify event arrives: the watcher
/// must still notice kernels or devices whose attributes do not emit events.
const RECHECK_EVERY: Duration = Duration::from_secs(5);

/// Sleep slice between inotify drains, so thread teardown is prompt without
/// burning CPU on a tight loop.
const DRAIN_SLICE: Duration = Duration::from_millis(100);

/// The production sysfs root. Tests pass their own directory instead.
const POWER_SUPPLY_ROOT: &str = "/sys/class/power_supply";

/// Owns the charger watcher thread. Dropping it does not stop the thread —
/// like the other session watchers, the thread exits when the command
/// channel closes, i.e. when the server shuts down.
pub struct InotifyPowerWatcher {
    /// Last observed `online` value plus the file it came from, shared with
    /// the watcher thread so `--doctor` and tests can see the live state.
    state: Arc<Mutex<Option<WatcherData>>>,
    /// Whether a watcher thread was actually spawned (AC adapter found).
    running: bool,
}

/// Plain data for the shared last-known state.
struct WatcherData {
    online: bool,
}

impl InotifyPowerWatcher {
    /// Start watching the first Mains supply's `online` attribute.
    ///
    /// Returns the watcher in either case: `is_running()` distinguishes a
    /// machine without an AC adapter from one with live events.
    pub fn start(tx: CommandSender) -> Self {
        Self::start_from(Path::new(POWER_SUPPLY_ROOT), tx)
    }

    /// Test seam: same logic against an alternate sysfs root.
    fn start_from(root: &Path, tx: CommandSender) -> Self {
        let target_path = find_ac_online_path(root);
        let initial_online = target_path
            .as_ref()
            .and_then(|path| read_online_value(path));
        let data = target_path.as_ref().map(|_| WatcherData {
            online: initial_online.unwrap_or(false),
        });
        let state = Arc::new(Mutex::new(data));
        let running = target_path.is_some();

        match target_path {
            Some(path) => {
                log::debug!("[Power] Watching charger events on {}", path.display());
                // Clone Arc before moving into the thread closure.
                let thread_state = Arc::clone(&state);
                let _handle = thread::Builder::new()
                    .name("ferramari-power-events".to_string())
                    .spawn(move || run_watcher(path, thread_state, tx))
                    .ok();
            }
            None => {
                log::debug!(
                    "[Power] No Mains supply under {}; charger events disabled",
                    POWER_SUPPLY_ROOT
                );
            }
        }

        Self { state, running }
    }

    /// Whether a watcher thread was started for this session.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// The last `online` value the watcher observed, if any.
    pub fn last_online(&self) -> Option<bool> {
        let guard = self.state.lock().ok()?;
        guard.as_ref().map(|d| d.online)
    }
}

/// Walk `root` for the first supply of type `Mains` that has a readable
/// `online` attribute. Desktop machines have exactly one; machines with
/// several adapters watch the first one, which is what the poll-based
/// `PowerProvider` also effectively reports.
fn find_ac_online_path(root: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = std::fs::read_to_string(path.join("type")) else {
            continue;
        };
        if kind.trim() != "Mains" {
            continue;
        }
        let online = path.join("online");
        if online.is_file() {
            return Some(online);
        }
    }
    None
}

/// Read one `online` attribute: `1` is plugged in, `0` is not.
fn read_online_value(path: &Path) -> Option<bool> {
    let contents = std::fs::read_to_string(path).ok()?;
    match contents.trim() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

/// The one transition rule, separated from I/O so it is testable: an
/// unchanged value is never news, an unknown previous state is not a
/// transition, and the direction decides which event fires.
fn transition(previous: Option<bool>, current: bool) -> Option<PowerEventType> {
    match previous {
        Some(was) if was == current => None,
        Some(true) => Some(PowerEventType::Disconnected),
        Some(false) => Some(PowerEventType::Connected),
        None => None,
    }
}

/// The worker loop: drain inotify without blocking, act on any signal by
/// re-reading the attribute, and re-read periodically as a safety net.
/// The loop ends when the command channel is closed, i.e. at server exit.
fn run_watcher(target_path: PathBuf, state: Arc<Mutex<Option<WatcherData>>>, tx: CommandSender) {
    let dir = match target_path.parent() {
        Some(dir) => dir.to_path_buf(),
        None => return,
    };

    let mut notifier = match inotify::Inotify::init() {
        Ok(notifier) => notifier,
        Err(err) => {
            log::warn!(
                "[Power] inotify unavailable ({err}); charger events fall back to the periodic poll"
            );
            return;
        }
    };

    let mask = inotify::WatchMask::MODIFY | inotify::WatchMask::MOVED_TO;
    if notifier.watches().add(&dir, mask).is_err() {
        log::warn!(
            "[Power] Could not watch {} ; falling back to the periodic poll",
            dir.display()
        );
        return;
    }

    // Seed from the shared state so the opening reading is not reported as
    // a transition; the app already has it.
    let mut last_online = state
        .lock()
        .ok()
        .as_ref()
        .and_then(|g| g.as_ref())
        .map(|d| d.online);
    let mut next_recheck = Instant::now() + RECHECK_EVERY;
    let mut buffer = [0u8; 1024];

    loop {
        // Drain every pending event without blocking; sysfs attributes are
        // noisy (udev rewrites them), so the file's value decides, not the
        // event count.
        let mut signaled = false;
        loop {
            match notifier.read_events(&mut buffer) {
                Ok(_events) => {
                    signaled = true;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => {
                    log::warn!(
                        "[Power] inotify read failed ({err}); falling back to the periodic poll"
                    );
                    return;
                }
            }
        }

        let deadline_due = Instant::now() >= next_recheck;
        if signaled || deadline_due {
            if deadline_due {
                next_recheck = Instant::now() + RECHECK_EVERY;
            }
            match read_online_value(&target_path) {
                Some(current) => {
                    if let Some(event) = transition(last_online, current) {
                        last_online = Some(current);
                        if let Ok(mut guard) = state.lock() {
                            if let Some(ref mut data) = *guard {
                                data.online = current;
                            }
                        }
                        log::debug!("[Power] Charger {}", event.as_str());
                        if tx.send(Command::PowerEvent(event)).is_err() {
                            log::debug!(
                                "[Power] Command bus closed; charger watcher stopping"
                            );
                            return;
                        }
                    } else {
                        last_online = Some(current);
                    }
                }
                // Unreadable attribute: keep the last known value, the next
                // event or recheck may bring it back.
                None => {
                    log::trace!(
                        "[Power] {} unreadable",
                        target_path.display()
                    );
                }
            }
        }

        thread::sleep(DRAIN_SLICE);
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn unique_tag() -> u128 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    /// Build a fake sysfs root: one battery (must be ignored) and one Mains
    /// supply whose `online` starts unplugged. Returns the root and the path
    /// of the online attribute.
    fn fake_supply_root(name: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "ferramari-power-events-{}-{name}",
            unique_tag()
        ));
        let battery = root.join("BAT0");
        fs::create_dir_all(&battery).unwrap();
        fs::write(battery.join("type"), "Battery\n").unwrap();

        let ac = root.join("AC");
        fs::create_dir_all(&ac).unwrap();
        fs::write(ac.join("type"), "Mains\n").unwrap();
        let online = ac.join("online");
        fs::write(&online, "0\n").unwrap();

        (root, online)
    }

    #[test]
    fn finds_the_mains_online_attribute_and_ignores_batteries() {
        let (root, online) = fake_supply_root("find");
        assert_eq!(find_ac_online_path(&root), Some(online));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_supply_without_an_online_attribute_is_not_a_watch_target() {
        let root = std::env::temp_dir().join(format!(
            "ferramari-power-events-{}",
            unique_tag()
        ));
        let ac = root.join("ADP0");
        fs::create_dir_all(&ac).unwrap();
        fs::write(ac.join("type"), "Mains\n").unwrap();
        // No `online` file: older drivers export `status` only.
        assert_eq!(find_ac_online_path(&root), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_root_is_not_an_error() {
        assert_eq!(
            find_ac_online_path(Path::new("/definitely/not/a/power/supply")),
            None
        );
    }

    #[test]
    fn online_values_map_to_the_expected_state() {
        let (root, online) = fake_supply_root("values");
        assert_eq!(read_online_value(&online), Some(false));
        fs::write(&online, "1\n").unwrap();
        assert_eq!(read_online_value(&online), Some(true));
        fs::write(&online, "unknown\n").unwrap();
        assert_eq!(read_online_value(&online), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn only_a_known_change_in_value_is_a_transition() {
        // The direction decides the event type.
        assert_eq!(
            transition(Some(false), true),
            Some(PowerEventType::Connected)
        );
        assert_eq!(
            transition(Some(true), false),
            Some(PowerEventType::Disconnected)
        );
        // An unchanged value is never news, even after a hundred events.
        assert_eq!(transition(Some(true), true), None);
        assert_eq!(transition(Some(false), false), None);
        // The opening reading is a baseline, not a plug event.
        assert_eq!(transition(None, true), None);
        assert_eq!(transition(None, false), None);
    }

    #[test]
    fn watcher_start_reports_running_only_with_an_ac_adapter() {
        // A root without any Mains supply must not claim live events.
        let (command_tx, _rx) = std::sync::mpsc::channel::<Command>();
        let watcher = InotifyPowerWatcher::start_from(
            Path::new("/definitely/not/a/power/supply"),
            CommandSender::new(command_tx),
        );
        assert!(!watcher.is_running());
        assert_eq!(watcher.last_online(), None);

        // With the fake supply present, the watcher starts and seeds from it.
        let (root, online) = fake_supply_root("running");
        let (command_tx, _rx) = std::sync::mpsc::channel::<Command>();
        let watcher =
            InotifyPowerWatcher::start_from(&root, CommandSender::new(command_tx));
        assert!(watcher.is_running());
        assert_eq!(watcher.last_online(), Some(false));
        let _ = fs::remove_file(&online);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_charger_toggle_reaches_the_command_bus_without_waiting_for_a_poll() {
        // The whole point of the module: plug in, and a PowerEvent is queued
        // well before the 30 s status poll would notice.
        let (root, online) = fake_supply_root("toggle");
        let (command_tx, command_rx) = std::sync::mpsc::channel::<Command>();
        let watcher =
            InotifyPowerWatcher::start_from(&root, CommandSender::new(command_tx));
        assert!(watcher.is_running());

        // Give the watcher thread time to open inotify and enter its loop.
        thread::sleep(Duration::from_millis(300));

        // Write while the watcher is sleeping between iterations — the next
        // non-blocking read should get the IN_MODIFY event immediately.
        fs::write(&online, "1\n").unwrap();
        let event = command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("no charger-connected event within 2 s");
        matches!(event, Command::PowerEvent(PowerEventType::Connected));

        fs::write(&online, "0\n").unwrap();
        let event = command_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("no charger-disconnected event within 2 s");
        matches!(event, Command::PowerEvent(PowerEventType::Disconnected));

        let _ = fs::remove_dir_all(root);
    }
}
