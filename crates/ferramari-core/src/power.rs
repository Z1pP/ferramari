//! Platform-neutral power-supply state consumed by Eco policy.

/// Snapshot of the machine's external power state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PowerStatus {
    pub on_battery: bool,
    pub percent: Option<u8>,
}

/// Instantaneous charger plug / unplug event delivered in real time.
///
/// Unlike [`PowerStatus`](struct.PowerStatus.html), which is a periodic
/// snapshot evaluated against the user's `pause_on_battery` setting,
/// `PowerEventType` fires the moment the kernel toggles the AC adapter
/// attribute — typically within milliseconds — and lets the eco engine
/// resume playback immediately when a charger is plugged in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEventType {
    /// Charger was plugged in (machine is now on AC).
    Connected,
    /// Charger was unplugged (machine is now on battery).
    Disconnected,
}

impl PowerEventType {
    /// Human-readable label for logging.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
        }
    }
}
