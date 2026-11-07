//! Restart policy for external players.
//!
//! mpv and WebKitGTK can die independently of the GTK process. Restarting them
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
}

impl RestartPolicy {
    pub fn new(now: Instant) -> Self {
        Self {
            failures: 0,
            started_at: now,
            retry_at: None,
        }
    }

    pub fn on_started(&mut self, now: Instant) {
        self.started_at = now;
        self.retry_at = None;
    }

    /// Record an exit (or a failed start) and decide what to do next.
    pub fn on_exit(&mut self, now: Instant) -> Retry {
        self.retry_at = None;
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
        if self.failures > 0 && now.duration_since(self.started_at) >= STABLE_AFTER {
            self.failures = 0;
            self.retry_at = None;
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
}
