//! Eco Engine policy: decide, per monitor, whether a wallpaper should play.
//!
//! The policy is pure and takes plain signals. Everything that observes the
//! operating system (X11 window state, screen lock, battery, idle time) feeds
//! these signals; keeping the decision separate makes the rules testable
//! without a display server or child processes.
//!
//! Priority, highest first:
//!   1. an explicit user pause, lock, suspend or the battery rule;
//!   2. a fully covered monitor (fullscreen game, maximised window);
//!   3. idle (profile dependent).
//!
//! A monitor that is not covered is never paused because *another* monitor is
//! covered. That is the whole point of per-monitor evaluation.

use std::time::{Duration, Instant};

/// Whether a paused video has been hidden long enough to release its decoder.
///
/// Parking is only offered while the monitor is not visible, so a black frame
/// can never appear in front of the user. The position is preserved by the
/// caller and restored when playback resumes.
pub fn should_park(paused_since: Option<Instant>, now: Instant, threshold: Duration) -> bool {
    paused_since.is_some_and(|since| now.saturating_duration_since(since) >= threshold)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Quality,
    Balanced,
    Eco,
}

impl Profile {
    /// Accept both the legacy config spelling and the names from the UI/spec.
    pub fn from_config(value: &str) -> Self {
        match value {
            "quality" | "performance" => Self::Quality,
            "eco" | "power" => Self::Eco,
            _ => Self::Balanced,
        }
    }

    /// Canonical value written to `config.json`. Keeping the old identifiers
    /// means a downgrade to 1.x still understands the profile.
    pub fn config_value(self) -> &'static str {
        match self {
            Self::Quality => "performance",
            Self::Balanced => "balanced",
            Self::Eco => "power",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Active,
    Idle,
    Occluded,
    Suspended,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Idle => "idle",
            Self::Occluded => "occluded",
            Self::Suspended => "suspended",
        }
    }
}

/// The result for one monitor. `play` is deliberately explicit: `Idle` may
/// still play (Balanced) or stand still (Eco), and callers must not infer it
/// from the state alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub state: State,
    pub play: bool,
    /// Frame-rate cap to apply while still playing. `None` keeps the profile's
    /// default filter. Balanced uses a gentle cap when idle; Eco stops
    /// entirely instead.
    pub fps: Option<u32>,
}

impl Plan {
    fn new(state: State, play: bool) -> Self {
        Self {
            state,
            play,
            fps: None,
        }
    }

    fn idle(play: bool, fps: Option<u32>) -> Self {
        Self {
            state: State::Idle,
            play,
            fps,
        }
    }
}

/// Inputs from one monitor plus the global session state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Signals {
    pub user_paused: bool,
    pub screen_locked: bool,
    pub suspended: bool,
    /// `pause_on_battery` is enabled and the machine is on battery.
    pub battery_paused: bool,
    pub idle: bool,
    /// A normal window covers this monitor completely.
    pub covered: bool,
    /// A fullscreen window is on this monitor.
    pub fullscreen: bool,
}

pub fn plan(profile: Profile, smart_pause: bool, signals: Signals) -> Plan {
    if signals.user_paused || signals.screen_locked || signals.suspended || signals.battery_paused {
        return Plan::new(State::Suspended, false);
    }

    let hidden = smart_pause && (signals.covered || signals.fullscreen);
    if hidden {
        return Plan::new(State::Occluded, false);
    }

    match profile {
        // Quality only stops for explicit/session reasons. It still honours
        // the user's "pause when maximised" switch, which is why `hidden` was
        // checked above; idle never lowers smoothness.
        Profile::Quality => Plan::new(State::Active, true),
        // Balanced keeps playing when the user is merely idle, but saves the
        // decoder as soon as the monitor is actually invisible.
        Profile::Balanced => {
            if signals.idle {
                Plan::idle(true, Some(30))
            } else {
                Plan::new(State::Active, true)
            }
        }
        // Eco is aggressive: an idle desktop does not need a decoder either.
        Profile::Eco => {
            if signals.idle {
                Plan::idle(false, None)
            } else {
                Plan::new(State::Active, true)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals() -> Signals {
        Signals::default()
    }

    #[test]
    fn occlusion_pauses_every_profile_when_smart_pause_is_on() {
        let covered = Signals {
            covered: true,
            ..signals()
        };
        for profile in [Profile::Quality, Profile::Balanced, Profile::Eco] {
            let result = plan(profile, true, covered);
            assert_eq!(result.state, State::Occluded);
            assert!(!result.play);
            // A disabled smart-pause switch leaves everything playing.
            assert!(plan(profile, false, covered).play);
        }
    }

    #[test]
    fn fullscreen_counts_as_occlusion_even_without_a_rectangle_match() {
        let fullscreen = Signals {
            fullscreen: true,
            ..signals()
        };
        let result = plan(Profile::Balanced, true, fullscreen);
        assert_eq!(result.state, State::Occluded);
        assert!(!result.play);
    }

    #[test]
    fn parking_requires_a_start_time_and_the_full_threshold() {
        let threshold = Duration::from_secs(600);
        let now = Instant::now();
        assert!(!should_park(None, now, threshold));
        assert!(!should_park(Some(now), now, threshold));
        assert!(!should_park(
            Some(now),
            now + threshold - Duration::from_secs(1),
            threshold
        ));
        assert!(should_park(Some(now), now + threshold, threshold));
        assert!(should_park(
            Some(now),
            now + threshold + Duration::from_secs(30),
            threshold
        ));
    }

    #[test]
    fn idle_only_stops_the_eco_profile() {
        let idle = Signals {
            idle: true,
            ..signals()
        };
        assert!(plan(Profile::Quality, true, idle).play);
        assert!(plan(Profile::Balanced, true, idle).play);
        assert!(!plan(Profile::Eco, true, idle).play);
        assert_eq!(plan(Profile::Eco, true, idle).state, State::Idle);
        // Balanced keeps playing but asks mpv for a lower frame rate.
        assert_eq!(plan(Profile::Balanced, true, idle).fps, Some(30));
        assert_eq!(plan(Profile::Quality, true, idle).fps, None);
    }

    #[test]
    fn session_and_explicit_pauses_win_over_every_profile() {
        for profile in [Profile::Quality, Profile::Balanced, Profile::Eco] {
            for signal in [
                Signals {
                    user_paused: true,
                    ..signals()
                },
                Signals {
                    screen_locked: true,
                    ..signals()
                },
                Signals {
                    suspended: true,
                    ..signals()
                },
                Signals {
                    battery_paused: true,
                    ..signals()
                },
            ] {
                let result = plan(profile, true, signal);
                assert_eq!(result.state, State::Suspended);
                assert!(!result.play);
            }
        }
    }

    #[test]
    fn a_covered_monitor_does_not_affect_an_uncovered_one() {
        let covered = plan(
            Profile::Balanced,
            true,
            Signals {
                covered: true,
                ..signals()
            },
        );
        let visible = plan(Profile::Balanced, true, signals());
        assert!(!covered.play);
        assert!(visible.play);
    }

    #[test]
    fn legacy_and_new_profile_names_map_to_the_same_policy() {
        assert_eq!(
            Profile::from_config("performance"),
            Profile::from_config("quality")
        );
        assert_eq!(Profile::from_config("power"), Profile::from_config("eco"));
        assert_eq!(Profile::from_config("balanced"), Profile::Balanced);
        assert_eq!(Profile::from_config("unknown"), Profile::Balanced);
        assert_eq!(Profile::Quality.config_value(), "performance");
        assert_eq!(Profile::Eco.config_value(), "power");
    }

    // -------------------------------------------------------------------------
    // Extended coverage: every flag combination per profile
    // -------------------------------------------------------------------------

    #[test]
    fn quality_play_always_unless_suspended_or_hidden() {
        let empty = Signals::default();

        // Idle alone never stops Quality profile.
        let p_idle = plan(
            Profile::Quality,
            true,
            Signals {
                idle: true,
                ..empty
            },
        );
        assert!(p_idle.play);
        assert_ne!(p_idle.state, State::Occluded);
        assert_eq!(p_idle.state, State::Active);

        // Idle+covered: occlusion still beats idle for Quality.
        let p_idle_covered = plan(
            Profile::Quality,
            true,
            Signals {
                idle: true,
                covered: true,
                ..empty
            },
        );
        assert!(!p_idle_covered.play);
        assert_eq!(p_idle_covered.state, State::Occluded);

        // Explicit occlusion only applies when smart_pause is on.
        let covered = Signals {
            covered: true,
            ..empty
        };
        let p_smart = plan(Profile::Quality, true, covered);
        assert_eq!(p_smart.state, State::Occluded);
        assert!(!p_smart.play);
        let p_no_smart = plan(Profile::Quality, false, covered);
        assert!(p_no_smart.play);

        // Fullscreen also counts as occlusion with smart_pause on.
        let fullscreen = Signals {
            fullscreen: true,
            ..empty
        };
        assert_eq!(
            plan(Profile::Quality, true, fullscreen).state,
            State::Occluded
        );
        assert!(!plan(Profile::Quality, true, fullscreen).play);
    }

    #[test]
    fn balanced_idle_triggers_framerate_cap_but_still_plays() {
        let idle = Signals {
            idle: true,
            ..Signals::default()
        };
        let p = plan(Profile::Balanced, true, idle);
        assert_eq!(p.state, State::Idle);
        assert!(p.play);
        assert_eq!(p.fps, Some(30));
    }

    #[test]
    fn eco_idle_does_not_play_at_all() {
        let idle = Signals {
            idle: true,
            ..Signals::default()
        };
        let p = plan(Profile::Eco, true, idle);
        assert_eq!(p.state, State::Idle);
        assert!(!p.play);
        assert!(p.fps.is_none());
    }

    #[test]
    fn battery_pause_wins_over_idle_activity() {
        let idle_battery = Signals {
            idle: true,
            battery_paused: true,
            ..Signals::default()
        };
        for profile in [Profile::Quality, Profile::Balanced, Profile::Eco] {
            let p = plan(profile, true, idle_battery);
            assert_eq!(p.state, State::Suspended);
            assert!(!p.play);
        }
    }

    #[test]
    fn smart_pause_false_disables_occlusion_handling() {
        let covered = Signals {
            covered: true,
            ..Signals::default()
        };
        for profile in [Profile::Quality, Profile::Balanced, Profile::Eco] {
            assert!(plan(profile, false, covered).play);
        }
    }

    #[test]
    fn all_state_transitions_are_mutable_via_signals() {
        let base = Signals::default();
        // Active: no signals set
        for prof in [Profile::Quality, Profile::Balanced] {
            let p = plan(prof, true, base);
            assert_eq!(p.state, State::Active);
            assert!(p.play);
        }
        let eco_active = plan(Profile::Eco, true, base);
        assert_eq!(eco_active.state, State::Active);
        assert!(eco_active.play);

        // Idle: active + idle signal
        let idle_sig = Signals { idle: true, ..base };
        assert_eq!(plan(Profile::Quality, true, idle_sig).state, State::Active);
        assert_eq!(plan(Profile::Balanced, true, idle_sig).state, State::Idle);
        assert_eq!(plan(Profile::Eco, true, idle_sig).state, State::Idle);

        // Occluded: covered/fullscreen
        let occluded = Signals {
            covered: true,
            ..base
        };
        for prof in [Profile::Quality, Profile::Balanced, Profile::Eco] {
            assert_eq!(plan(prof, true, occluded).state, State::Occluded);
        }

        // Suspended: any high-priority signal
        for field in [
            "user_paused",
            "screen_locked",
            "suspended",
            "battery_paused",
        ] {
            // Construct via bit flags equivalent
            let suspended = match field {
                "user_paused" => Signals {
                    user_paused: true,
                    ..base
                },
                "screen_locked" => Signals {
                    screen_locked: true,
                    ..base
                },
                "suspended" => Signals {
                    suspended: true,
                    ..base
                },
                _ => Signals {
                    battery_paused: true,
                    ..base
                },
            };
            for prof in [Profile::Quality, Profile::Balanced, Profile::Eco] {
                assert_eq!(plan(prof, true, suspended).state, State::Suspended);
            }
        }
    }

    #[test]
    fn fps_caps_are_profile_specific_not_signal_dependent() {
        let idle = Signals {
            idle: true,
            ..Signals::default()
        };
        let quality = plan(Profile::Quality, true, idle);
        let balanced = plan(Profile::Balanced, true, idle);
        let eco = plan(Profile::Eco, true, idle);
        // Only Balanced applies a cap while playing; others are None.
        assert!(quality.fps.is_none());
        assert_eq!(balanced.fps, Some(30));
        assert!(eco.fps.is_none());
    }

    #[test]
    fn as_str_values_match_expected_strings() {
        assert_eq!(State::Active.as_str(), "active");
        assert_eq!(State::Idle.as_str(), "idle");
        assert_eq!(State::Occluded.as_str(), "occluded");
        assert_eq!(State::Suspended.as_str(), "suspended");
    }
}
