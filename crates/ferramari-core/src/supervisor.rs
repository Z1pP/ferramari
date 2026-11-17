//! Restart policy for external players.
//!
//! mpv can die independently of the Ferramari process. Restarting it
//! immediately on every exit turns a broken file or driver into a busy loop, so
//! failures are spaced out and eventually given up on. The policy is pure and
//! time-injected, which keeps it testable without spawning processes.

use std::time::{Duration, Instant};

/// Number of retries after the first unexpected exit.
pub const MAX_RETRIES: u32 = 5;

/// A player that survives this long is considered healthy; its failure count
/// is reset so a later crash starts with a short delay again.
pub const STABLE_AFTER: Duration = Duration::from_secs(30);

const BASE_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    Scheduled(Duration),
    GivenUp,
}

#[derive(Debug)]
pub struct RestartPolicy {
    failures: u32,
    started_at: Instant,
    retry_at: Option<Instant>,
    /// The surface disappeared rather than the player failing (an Explorer
    /// restart on Windows). While set, failed rebuilds keep retrying instead
    /// of counting against [`MAX_RETRIES`], because the player is not at
    /// fault and the host can stay away for as long as the shell is down.
    host_lost: bool,
}

impl RestartPolicy {
    pub fn new(now: Instant) -> Self {
        Self {
            failures: 0,
            started_at: now,
            retry_at: None,
            host_lost: false,
        }
    }

    pub fn on_started(&mut self, now: Instant) {
        self.started_at = now;
        self.retry_at = None;
        self.host_lost = false;
    }

    /// The wallpaper host disappeared while the player was still running.
    /// Schedule one quick rebuild and forget earlier failures: an Explorer
    /// restart is not a broken video.
    pub fn on_host_lost(&mut self, now: Instant) -> Duration {
        self.failures = 0;
        self.host_lost = true;
        self.retry_at = Some(now + BASE_DELAY);
        BASE_DELAY
    }

    /// Record an exit (or a failed start) and decide what to do next.
    pub fn on_exit(&mut self, now: Instant) -> Retry {
        self.retry_at = None;
        if self.host_lost {
            // Still waiting for Explorer to recreate its desktop host. Keep
            // the player out of the failure budget until a rebuild succeeds.
            self.retry_at = Some(now + BASE_DELAY);
            return Retry::Scheduled(BASE_DELAY);
        }
        if self.failures > 0 && now.duration_since(self.started_at) >= STABLE_AFTER {
            self.failures = 0;
        }
        self.failures += 1;
        if self.failures > MAX_RETRIES {
            return Retry::GivenUp;
        }
        let delay = BASE_DELAY
            .saturating_mul(1u32 << (self.failures - 1).min(5))
            .min(MAX_DELAY);
        self.retry_at = Some(now + delay);
        Retry::Scheduled(delay)
    }

    /// If a retry is scheduled and due, mark it as consumed and return true.
    pub fn retry_is_due(&mut self, now: Instant) -> bool {
        // Only clear the failure history while no retry is pending. After an
        // exit, `started_at` still points at the player that just died; a
        // stability check here would erase the retry that `on_exit` scheduled
        // for a player that had been running longer than `STABLE_AFTER`.
        if self.retry_at.is_none()
            && self.failures > 0
            && now.duration_since(self.started_at) >= STABLE_AFTER
        {
            self.failures = 0;
        }
        if self.retry_at.is_some_and(|at| now >= at) {
            self.retry_at = None;
            true
        } else {
            false
        }
    }

    /// True while a scheduled retry is still in the future. Used by the
    /// regular window sync so it does not bypass the backoff.
    pub fn waits_before_start(&self, now: Instant) -> bool {
        self.retry_at.is_some_and(|at| now < at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_increase_and_then_give_up() {
        let start = Instant::now();
        let mut policy = RestartPolicy::new(start);
        let expected = [1, 2, 4, 8, 16];
        for (index, seconds) in expected.iter().enumerate() {
            let now = start + Duration::from_secs(index as u64);
            match policy.on_exit(now) {
                Retry::Scheduled(delay) => assert_eq!(delay, Duration::from_secs(*seconds)),
                Retry::GivenUp => panic!("gave up too early at attempt {}", index + 1),
            }
            policy.on_started(now);
        }
        assert_eq!(
            policy.on_exit(start + Duration::from_secs(10)),
            Retry::GivenUp
        );
    }

    #[test]
    fn a_stable_player_resets_the_failure_count() {
        let start = Instant::now();
        let mut policy = RestartPolicy::new(start);
        assert_eq!(
            policy.on_exit(start),
            Retry::Scheduled(Duration::from_secs(1))
        );
        policy.on_started(start + Duration::from_secs(2));
        assert_eq!(
            policy.on_exit(start + Duration::from_secs(2) + STABLE_AFTER),
            Retry::Scheduled(Duration::from_secs(1))
        );
    }

    #[test]
    fn retries_are_only_due_after_the_delay() {
        let start = Instant::now();
        let mut policy = RestartPolicy::new(start);
        assert!(!policy.retry_is_due(start));
        assert_eq!(
            policy.on_exit(start),
            Retry::Scheduled(Duration::from_secs(1))
        );
        assert!(policy.waits_before_start(start));
        assert!(!policy.retry_is_due(start + Duration::from_millis(500)));
        assert!(policy.retry_is_due(start + Duration::from_secs(1)));
        assert!(!policy.waits_before_start(start + Duration::from_secs(1)));
    }

    #[test]
    fn a_player_that_ran_for_a_while_still_restarts_after_exit() {
        let start = Instant::now();
        let mut policy = RestartPolicy::new(start);
        let crashed = start + STABLE_AFTER + Duration::from_secs(10);
        assert_eq!(
            policy.on_exit(crashed),
            Retry::Scheduled(Duration::from_secs(1))
        );
        assert!(policy.retry_is_due(crashed + Duration::from_secs(1)));
    }

    #[test]
    fn a_lost_host_keeps_retrying_without_giving_up() {
        let start = Instant::now();
        let mut policy = RestartPolicy::new(start);
        // An Explorer restart can leave the desktop unavailable for a while;
        // every failed rebuild must schedule another attempt, not exhaust the
        // player's failure budget.
        let mut now = start;
        for _ in 0..(MAX_RETRIES * 3) {
            assert_eq!(policy.on_host_lost(now), Duration::from_secs(1));
            assert!(policy.retry_is_due(now + Duration::from_secs(1)));
            now += Duration::from_secs(1);
            assert_eq!(
                policy.on_exit(now),
                Retry::Scheduled(Duration::from_secs(1))
            );
        }
    }

    #[test]
    fn a_successful_rebuild_ends_the_host_loss_retries() {
        let start = Instant::now();
        let mut policy = RestartPolicy::new(start);
        policy.on_host_lost(start);
        policy.on_started(start + Duration::from_secs(1));
        // The player is treated normally again: a real crash counts.
        assert_eq!(
            policy.on_exit(start + Duration::from_secs(2)),
            Retry::Scheduled(Duration::from_secs(1))
        );
        assert_eq!(
            policy.on_exit(start + Duration::from_secs(3)),
            Retry::Scheduled(Duration::from_secs(2))
        );
    }
}
