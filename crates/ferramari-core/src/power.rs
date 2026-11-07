//! Platform-neutral power-supply state consumed by Eco policy.

/// Snapshot of the machine's external power state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PowerStatus {
    pub on_battery: bool,
    pub percent: Option<u8>,
}
