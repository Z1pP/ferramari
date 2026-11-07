//! Platform-neutral window geometry and visibility model.
//!
//! Backends translate native window-system observations into these plain
//! rectangles. The Eco Engine consumes the same model on every operating
//! system and never needs to know whether the source was EWMH, Win32 or
//! WindowServer.

/// One visible top-level window, in the coordinate space used by monitors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
    pub fullscreen: bool,
}

/// All visible windows that can occlude a wallpaper. The list is global; the
/// Eco Engine evaluates it separately for every monitor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowState {
    pub windows: Vec<WindowRect>,
}

/// What a window list says about one monitor rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MonitorVisibility {
    pub covered: bool,
    pub fullscreen: bool,
}

impl WindowState {
    /// Whether a monitor is completely covered by a normal window, and
    /// whether a fullscreen window is present. A maximised window counts as
    /// covered even when decorations/panels leave a few pixels around its
    /// client area: the user cannot see the wallpaper there either.
    pub fn visibility_at(&self, x: i32, y: i32, width: i32, height: i32) -> MonitorVisibility {
        if width <= 0 || height <= 0 {
            return MonitorVisibility::default();
        }
        let mut visibility = MonitorVisibility::default();
        for window in &self.windows {
            let center_inside = center_inside(window, x, y, width, height);
            let covers = rect_covers(window, x, y, width, height);
            if covers || (window.maximized && center_inside) {
                visibility.covered = true;
            }
            if window.fullscreen && (covers || center_inside) {
                visibility.covered = true;
                visibility.fullscreen = true;
            }
        }
        visibility
    }
}

fn rect_covers(window: &WindowRect, x: i32, y: i32, width: i32, height: i32) -> bool {
    window.width > 0
        && window.height > 0
        && window.x <= x
        && window.y <= y
        && window.x.saturating_add(window.width) >= x.saturating_add(width)
        && window.y.saturating_add(window.height) >= y.saturating_add(height)
}

fn center_inside(window: &WindowRect, x: i32, y: i32, width: i32, height: i32) -> bool {
    if window.width <= 0 || window.height <= 0 {
        return false;
    }
    let center_x = window.x.saturating_add(window.width / 2);
    let center_y = window.y.saturating_add(window.height / 2);
    center_x >= x
        && center_x < x.saturating_add(width)
        && center_y >= y
        && center_y < y.saturating_add(height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: i32, height: i32) -> WindowRect {
        WindowRect {
            x,
            y,
            width,
            height,
            maximized: false,
            fullscreen: false,
        }
    }

    #[test]
    fn an_empty_window_list_covers_nothing() {
        let state = WindowState::default();
        assert_eq!(state.visibility_at(0, 0, 1920, 1080), MonitorVisibility::default());
    }

    #[test]
    fn a_fullscreen_window_covers_only_its_own_monitor() {
        let state = WindowState {
            windows: vec![WindowRect { fullscreen: true, ..rect(1920, 0, 1920, 1080) }],
        };
        assert_eq!(state.visibility_at(0, 0, 1920, 1080), MonitorVisibility::default());
        assert_eq!(
            state.visibility_at(1920, 0, 1920, 1080),
            MonitorVisibility { covered: true, fullscreen: true }
        );
    }

    #[test]
    fn a_window_spanning_both_monitors_covers_both() {
        let state = WindowState {
            windows: vec![WindowRect { fullscreen: true, ..rect(0, 0, 3840, 1080) }],
        };
        for x in [0, 1920] {
            let visibility = state.visibility_at(x, 0, 1920, 1080);
            assert!(visibility.covered, "monitor at {x} should be covered");
            assert!(visibility.fullscreen);
        }
    }

    #[test]
    fn a_small_window_does_not_pause_the_wallpaper() {
        let state = WindowState { windows: vec![rect(100, 100, 800, 600)] };
        assert!(!state.visibility_at(0, 0, 1920, 1080).covered);
    }

    #[test]
    fn a_maximized_window_with_a_panel_inset_still_counts() {
        let state = WindowState {
            windows: vec![WindowRect { maximized: true, ..rect(0, 32, 1920, 1048) }],
        };
        let visibility = state.visibility_at(0, 0, 1920, 1080);
        assert!(visibility.covered);
        assert!(!visibility.fullscreen);
    }
}
