//! GTK-free volume fades.
//!
//! The GTK implementation drove every fade step from one `glib` timer per
//! slot. This version runs the same slot model on a per-fade thread: a fade
//! owns its interval loop, and `cancel`/`start` take effect on the next tick
//! (the slot's shared `generation` counter is bumped, old loops observe it
//! and exit). The mpv calls from the callback are cheap socket writes, so
//! threading is safe here in a way it never was with GTK widgets.
//!
//! Contract parity with the GTK `Fade`:
//! * one in-flight fade per slot (`start` replaces the previous one),
//! * `finish_after` schedules only the completion (no interpolation),
//! * `cancel` stops the fade without running the completion.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Shared slot state: bumped by every `start`/`cancel` so running threads
/// notice they have been superseded.
#[derive(Default)]
struct Slot {
    generation: AtomicU64,
    active: AtomicBool,
}

pub struct Fade {
    slot: Arc<Slot>,
}

impl Default for Fade {
    fn default() -> Self {
        Self {
            slot: Arc::new(Slot::default()),
        }
    }
}

impl Fade {
    /// Stop the in-flight fade, if any. The completion callback does not run.
    pub fn cancel(&mut self) {
        self.slot.generation.fetch_add(1, Ordering::SeqCst);
        self.slot.active.store(false, Ordering::SeqCst);
    }

    /// Interpolate `from → to` over `duration` seconds, calling `each_value`
    /// every `interval` seconds, then `complete`.
    pub fn start<F, C>(&mut self, from: f64, to: f64, duration: f64, interval: f64, each_value: F, complete: C)
    where
        F: Fn(f64) + Send + 'static,
        C: FnOnce() + Send + 'static,
    {
        let steps = fade_steps(duration, interval).max(1.0);
        let step_size = (to - from) / steps;
        let generation = self.slot.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.slot.active.store(true, Ordering::SeqCst);

        let slot = Arc::clone(&self.slot);
        let _ = std::thread::Builder::new()
            .name("ferramari-fade".into())
            .spawn(move || {
                let mut value = from;
                let tick = Duration::from_secs_f64(interval.max(0.001));
                for _ in 0..steps as u64 {
                    std::thread::sleep(tick);
                    if slot.generation.load(Ordering::SeqCst) != generation {
                        return; // superseded or cancelled
                    }
                    value += step_size;
                    each_value(value);
                }
                if slot.generation.load(Ordering::SeqCst) == generation {
                    slot.active.store(false, Ordering::SeqCst);
                    each_value(to);
                    complete();
                }
            });
    }

    /// Run `complete` once after `duration`, with interval ticks in between
    /// (the GTK pause timer rechecked liveness every `interval`). A newer
    /// `start`/`cancel` on the same slot preempts it.
    pub fn finish_after<C>(&mut self, duration: f64, interval: f64, complete: C)
    where
        C: FnOnce() + Send + 'static,
    {
        let generation = self.slot.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.slot.active.store(true, Ordering::SeqCst);
        let slot = Arc::clone(&self.slot);
        let tick = Duration::from_secs_f64(interval.max(0.001));
        let deadline = Instant::now() + Duration::from_secs_f64(duration.max(0.0));
        let _ = std::thread::Builder::new()
            .name("ferramari-fade-done".into())
            .spawn(move || {
                while Instant::now() < deadline {
                    if slot.generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    std::thread::sleep(tick.min(deadline - Instant::now()));
                }
                if slot.generation.load(Ordering::SeqCst) == generation {
                    slot.active.store(false, Ordering::SeqCst);
                    complete();
                }
            });
    }
}

/// Number of interpolation steps for a fade (mirrors the GTK helper).
pub fn fade_steps(duration: f64, interval: f64) -> f64 {
    if interval <= 0.0 {
        return 0.0;
    }
    (duration / interval).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn steps_scale_with_duration() {
        assert_eq!(fade_steps(1.0, 0.1), 10.0);
        assert_eq!(fade_steps(0.0, 0.1), 0.0);
        assert_eq!(fade_steps(1.0, 0.0), 0.0);
    }

    #[test]
    fn a_new_start_supersedes_the_running_fade() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut fade = Fade::default();

        fade.start(
            0.0,
            100.0,
            10.0,
            0.05,
            {
                let counter = Arc::clone(&counter);
                move |_| {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            },
            || {},
        );
        // Replace immediately: the first loop must stop on its next tick.
        fade.start(0.0, 50.0, 0.1, 0.05, |_| {}, || {});
        std::thread::sleep(Duration::from_millis(400));

        // Only the second (0.1s / 0.05s = 2 steps) fade should have run.
        assert!(
            counter.load(Ordering::Relaxed) <= 2,
            "superseded fade kept ticking: {}",
            counter.load(Ordering::Relaxed)
        );
    }

    #[test]
    fn cancel_stops_the_fade() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut fade = Fade::default();
        fade.start(
            0.0,
            100.0,
            5.0,
            0.05,
            {
                let counter = Arc::clone(&counter);
                move |_| {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            },
            || {},
        );
        fade.cancel();
        let observed = counter.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            counter.load(Ordering::Relaxed),
            observed,
            "fade kept running after cancel"
        );
    }

    #[test]
    fn finish_after_runs_once() {
        let done = Arc::new(AtomicBool::new(false));
        let mut fade = Fade::default();
        fade.finish_after(0.05, 0.02, {
            let done = Arc::clone(&done);
            move || done.store(true, Ordering::Relaxed)
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(done.load(Ordering::Relaxed));
    }

    #[test]
    fn finish_after_preempted_by_cancel() {
        let done = Arc::new(AtomicBool::new(false));
        let mut fade = Fade::default();
        fade.finish_after(0.2, 0.02, {
            let done = Arc::clone(&done);
            move || done.store(true, Ordering::Relaxed)
        });
        fade.cancel();
        std::thread::sleep(Duration::from_millis(400));
        assert!(!done.load(Ordering::Relaxed));
    }
}
