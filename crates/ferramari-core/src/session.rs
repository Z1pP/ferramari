//! Platform-neutral session state: who says the screen is locked, and whether
//! anybody is at the keyboard.
//!
//! These facts arrive as a stream of OS signals — a screensaver became active,
//! logind announced a lock, the idle timer crossed the threshold. Turning each
//! one into a separate boolean inside the application was enough while Linux
//! was the only backend, but it loses two things a second platform needs:
//! *where* a fact came from, and whether anything said it at all.
//!
//! Both matter in practice:
//!
//! - A GNOME session reports a lock through the screensaver service *and*
//!   through logind. The unlock that arrives from one source must not cancel a
//!   lock the other still holds. With two independent booleans the correct
//!   answer is derivable but nothing states it, and each call site re-derived
//!   it a little differently.
//! - An idle service that vanishes at logout or a shell restart reports
//!   nothing. That is not the same statement as "the user is active", and
//!   treating the silence as a reading is how a wallpaper ends up paused
//!   forever behind a dead screensaver.
//!
//! So this module keeps per-source facts and derives the single answer the
//! policy layer consumes. A backend only reports what it observed; the
//! interpretation is shared and tested once, here.

/// Which OS facility reported a lock or an unlock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockSource {
    /// A freedesktop screensaver service (`…ScreenSaver.ActiveChanged`).
    ScreenSaver,
    /// The session manager's own lock (logind `Lock`/`Unlock` signals).
    SessionManager,
}

impl LockSource {
    pub fn as_str(self) -> &'static str {
        match self {
            LockSource::ScreenSaver => "screensaver",
            LockSource::SessionManager => "session manager",
        }
    }
}

/// Lock state aggregated over every source the session exposes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LockState {
    screensaver: bool,
    session_manager: bool,
}

impl LockState {
    /// Record what one source reported. Returns whether the derived answer
    /// changed, which is when the policy layer has anything to re-evaluate: a
    /// compositor that repeats its signal costs no rebuild.
    pub fn set(&mut self, source: LockSource, active: bool) -> bool {
        let before = self.locked();
        match source {
            LockSource::ScreenSaver => self.screensaver = active,
            LockSource::SessionManager => self.session_manager = active,
        }
        self.locked() != before
    }

    /// Locked if any source still says so. This is the rule that stops an
    /// unlock from one facility resuming playback under a lock held by
    /// another.
    pub fn locked(&self) -> bool {
        self.screensaver || self.session_manager
    }

    /// The sources currently holding a lock, for diagnostics.
    pub fn holding(&self) -> Vec<LockSource> {
        let mut sources = Vec::new();
        if self.screensaver {
            sources.push(LockSource::ScreenSaver);
        }
        if self.session_manager {
            sources.push(LockSource::SessionManager);
        }
        sources
    }
}

/// Desktop activity, as far as the session can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IdleState {
    /// The idle timer is below the threshold: somebody is at the keyboard.
    #[default]
    Active,
    /// The idle timer is past the threshold.
    Idle,
    /// Nothing to ask. A screensaver or idle-monitor service that is not
    /// running cannot answer, and any value here would be a claim about the
    /// user that nobody observed.
    Unknown,
}

impl IdleState {
    /// The rule the policy layer uses. `Unknown` deliberately reads as
    /// "not idle": a wallpaper keeps playing when nothing can prove the room
    /// is empty, and it stops the moment a service comes back and says idle.
    pub fn pauses_playback(self) -> bool {
        self == IdleState::Idle
    }

    /// Compare an observed idle duration with the configured threshold.
    ///
    /// A backend reports the duration it read and the threshold it was
    /// configured with; it does not decide what the combination means, so the
    /// inclusive boundary is stated once instead of in every poller.
    pub fn from_seconds(idle_seconds: Option<u64>, threshold_seconds: u64) -> Self {
        match idle_seconds {
            Some(idle) if idle >= threshold_seconds => IdleState::Idle,
            Some(_) => IdleState::Active,
            None => IdleState::Unknown,
        }
    }

    /// The same comparison in milliseconds, which is the unit Mutter's
    /// IdleMonitor reports. Compared in its own unit rather than divided into
    /// seconds first, so a reading of 300.999 s against a 300 s threshold is
    /// idle, as it should be, instead of being truncated to equality-by-accident.
    pub fn from_millis(idle_millis: Option<u64>, threshold_millis: u64) -> Self {
        match idle_millis {
            Some(idle) if idle >= threshold_millis => IdleState::Idle,
            Some(_) => IdleState::Active,
            None => IdleState::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            IdleState::Active => "active",
            IdleState::Idle => "idle",
            IdleState::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_unlock_does_not_cancel_another_source_s_lock() {
        // GNOME reports a lock through both the screensaver service and
        // logind. The first unlock must not resume playback while the second
        // still says locked.
        let mut lock = LockState::default();
        assert!(lock.set(LockSource::ScreenSaver, true));
        // The second source agreeing changes the derived answer not at all,
        // and `false` here is what tells the app there is nothing to re-plan.
        assert!(
            !lock.set(LockSource::SessionManager, true),
            "already locked: no new decision to make"
        );
        assert!(lock.locked());

        assert!(
            !lock.set(LockSource::ScreenSaver, false),
            "logind still holds the lock, so nothing changes"
        );
        assert!(lock.locked());
        assert_eq!(lock.holding(), vec![LockSource::SessionManager]);

        assert!(lock.set(LockSource::SessionManager, false));
        assert!(!lock.locked());
        assert!(lock.holding().is_empty());
    }

    #[test]
    fn a_repeated_signal_reports_no_change() {
        let mut lock = LockState::default();
        assert!(lock.set(LockSource::ScreenSaver, true));
        // Compositors re-emit ActiveChanged; an echo must not trigger a rebuild.
        assert!(!lock.set(LockSource::ScreenSaver, true));
        assert!(lock.set(LockSource::ScreenSaver, false));
        assert!(!lock.set(LockSource::ScreenSaver, false));
    }

    #[test]
    fn each_source_is_remembered_separately() {
        let mut lock = LockState::default();
        lock.set(LockSource::ScreenSaver, true);
        lock.set(LockSource::SessionManager, true);
        assert_eq!(
            lock.holding(),
            vec![LockSource::ScreenSaver, LockSource::SessionManager]
        );
    }

    #[test]
    fn unknown_idle_is_not_treated_as_the_room_being_empty() {
        assert!(IdleState::Idle.pauses_playback());
        assert!(!IdleState::Active.pauses_playback());
        assert!(
            !IdleState::Unknown.pauses_playback(),
            "a vanished screensaver service must not stop a wallpaper forever"
        );
        assert_eq!(IdleState::default(), IdleState::Active);
    }

    #[test]
    fn the_idle_threshold_is_inclusive() {
        assert_eq!(IdleState::from_seconds(Some(300), 300), IdleState::Idle);
        assert_eq!(IdleState::from_seconds(Some(299), 300), IdleState::Active);
        assert_eq!(IdleState::from_seconds(None, 300), IdleState::Unknown);
    }

    #[test]
    fn millisecond_readings_are_compared_in_their_own_unit() {
        // Mutter reports milliseconds. Truncating to seconds first would make
        // 300.999 s against a 300 s threshold "active", so the comparison
        // stays in milliseconds.
        assert_eq!(
            IdleState::from_millis(Some(300_999), 300_000),
            IdleState::Idle
        );
        assert_eq!(
            IdleState::from_millis(Some(299_999), 300_000),
            IdleState::Active
        );
        assert_eq!(
            IdleState::from_millis(Some(300_000), 300_000),
            IdleState::Idle
        );
        assert_eq!(IdleState::from_millis(None, 300_000), IdleState::Unknown);
    }

    #[test]
    fn the_millisecond_rule_agrees_with_the_second_rule() {
        // The two entry points must not disagree on whole-second readings,
        // or the unit a backend happens to report would change the outcome.
        for seconds in [0u64, 1, 120, 299, 300, 301, 86_400] {
            assert_eq!(
                IdleState::from_seconds(Some(seconds), 300),
                IdleState::from_millis(Some(seconds * 1000), 300 * 1000),
                "{seconds} s"
            );
        }
    }

    #[test]
    fn a_zero_threshold_still_distinguishes_observed_from_unanswered() {
        // A misconfigured threshold must not turn "no answer" into "idle".
        assert_eq!(IdleState::from_seconds(Some(0), 0), IdleState::Idle);
        assert_eq!(IdleState::from_seconds(None, 0), IdleState::Unknown);
    }

    #[test]
    fn state_names_are_stable_for_logs_and_diagnostics() {
        assert_eq!(LockSource::ScreenSaver.as_str(), "screensaver");
        assert_eq!(LockSource::SessionManager.as_str(), "session manager");
        assert_eq!(IdleState::Unknown.as_str(), "unknown");
        assert_eq!(IdleState::Idle.as_str(), "idle");
        assert_eq!(IdleState::Active.as_str(), "active");
    }
}
