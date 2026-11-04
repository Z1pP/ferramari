//! Volume fades on the GTK main loop.
//!
//! A single `glib` timer drives all steps instead of one timer per step, so
//! one `Fade` slot can only ever have one fade in flight. A wallpaper window
//! therefore keeps separate slots for concerns that must not cancel each
//! other: the volume fade started by the mute rule and the pause timer
//! started when the Eco Engine pauses playback.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use glib::SourceId;

/// Number of ticks for a fade with the given duration and interval, after the
/// interval has been clamped to a sane range.
pub fn fade_steps(duration: f64, interval: f64) -> f64 {
    let interval = interval.clamp(0.01, 2.0);
    (duration / interval).max(1.0)
}

#[derive(Default)]
pub struct Fade {
    /// Shared with the timeout: the timeout clears its own slot when it
    /// finishes, so `cancel` never tries to remove an already-dead source
    /// (which is a hard GLib error).
    source: Rc<RefCell<Option<SourceId>>>,
}

impl Fade {
    pub fn cancel(&mut self) {
        if let Some(source) = self.source.borrow_mut().take() {
            source.remove();
        }
    }

    /// Fade `from` to `to` in `duration` seconds, calling `update` every
    /// `interval` seconds and `complete` once the target is reached.
    pub fn start<U, C>(
        &mut self,
        from: f64,
        to: f64,
        duration: f64,
        interval: f64,
        update: U,
        complete: C,
    ) where
        U: Fn(f64) + 'static,
        C: FnOnce() + 'static,
    {
        self.cancel();

        let interval = interval.clamp(0.01, 2.0);
        let steps = fade_steps(duration, interval);
        let step = (to - from) / steps;
        let mut current = from;
        let mut remaining = steps as u64;
        let mut complete = Some(complete);
        let slot = Rc::new(RefCell::new(None::<SourceId>));
        let slot_in_closure = Rc::clone(&slot);

        let source = glib::timeout_add_local(Duration::from_secs_f64(interval), move || {
            remaining = remaining.saturating_sub(1);
            current += step;
            let done =
                remaining == 0 || (step < 0.0 && current <= to) || (step > 0.0 && current >= to);
            if done {
                update(to);
                if let Some(complete) = complete.take() {
                    complete();
                }
                slot_in_closure.borrow_mut().take();
                glib::ControlFlow::Break
            } else {
                update(current);
                glib::ControlFlow::Continue
            }
        });
        *slot.borrow_mut() = Some(source);
        self.source = slot;
    }

    /// Call `complete` once the time a fade of `duration`/`interval` would
    /// take has passed, without driving anything on the way.
    ///
    /// This is for a step that must land when a fade ends but must not be
    /// cancelled by the next fade on the volume slot, such as pausing the
    /// player after the audio has faded out. It uses the same step maths as
    /// `start` so both timers finish together.
    pub fn finish_after<C>(&mut self, duration: f64, interval: f64, complete: C)
    where
        C: FnOnce() + 'static,
    {
        self.cancel();

        let interval = interval.clamp(0.01, 2.0);
        let mut remaining = fade_steps(duration, interval) as u64;
        let mut complete = Some(complete);
        let slot = Rc::new(RefCell::new(None::<SourceId>));
        let slot_in_closure = Rc::clone(&slot);

        let source = glib::timeout_add_local(Duration::from_secs_f64(interval), move || {
            remaining = remaining.saturating_sub(1);
            if remaining > 0 {
                return glib::ControlFlow::Continue;
            }
            if let Some(complete) = complete.take() {
                complete();
            }
            slot_in_closure.borrow_mut().take();
            glib::ControlFlow::Break
        });
        *slot.borrow_mut() = Some(source);
        self.source = slot;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The step maths is the only part worth testing without a main loop.
    fn step(from: f64, to: f64, duration: f64, interval: f64) -> f64 {
        (to - from) / fade_steps(duration, interval)
    }

    #[test]
    fn fades_up_and_down() {
        assert!(step(0.0, 50.0, 1.5, 0.1) > 0.0);
        assert!(step(50.0, 0.0, 1.5, 0.1) < 0.0);
        assert_eq!(step(0.0, 50.0, 1.5, 0.1) * 15.0, 50.0);
    }

    #[test]
    fn zero_duration_still_moves_one_step() {
        assert_eq!(step(0.0, 50.0, 0.0, 0.1), 50.0);
    }

    #[test]
    fn pause_timer_ends_with_the_volume_fade() {
        // Both timers must run the same number of ticks for one fade, or the
        // pause would land before the audio has faded out.
        assert_eq!(fade_steps(1.5, 0.1), 15.0);
        assert_eq!(fade_steps(1.5, 10.0), 1.0);
        assert_eq!(fade_steps(0.0, 0.1), 1.0);
        // A clamped interval is what the timeout actually uses.
        assert_eq!(fade_steps(1.5, 0.001), fade_steps(1.5, 0.01));
    }
}
