//! Volume fades on the GTK main loop.
//!
//! Upstream used a `threading.Timer` per step; this uses a single `glib`
//! timeout per fade, so a window can only ever have one fade in flight.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use glib::SourceId;

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
        let steps = (duration / interval).max(1.0);
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
}

#[cfg(test)]
mod tests {
    /// The step maths is the only part worth testing without a main loop.
    fn step(from: f64, to: f64, duration: f64, interval: f64) -> f64 {
        let steps = (duration / interval).max(1.0);
        (to - from) / steps
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
}
