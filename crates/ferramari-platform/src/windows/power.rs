//! Windows power-supply provider backed by `GetSystemPowerStatus`.
//!
//! Charger transitions are also delivered as `WM_POWERBROADCAST` by the
//! session watcher in [`crate::handlers`], but the server polls the current
//! state every few seconds anyway. Keeping `subscribe()` at the default
//! (`None`) makes that polling the single source of truth instead of two
//! racing paths.

use ferramari_core::platform::PowerProvider;
use ferramari_core::power::PowerStatus;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

/// Battery flag values documented for `SYSTEM_POWER_STATUS::BatteryFlag`.
const BATTERY_FLAG_NO_BATTERY: u8 = 128;
const BATTERY_FLAG_UNKNOWN: u8 = 255;
const ACLINE_STATUS_OFFLINE: u8 = 0;
const BATTERY_PERCENT_UNKNOWN: u8 = 255;

#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsPowerProvider;

impl PowerProvider for WindowsPowerProvider {
    fn status(&self) -> PowerStatus {
        read_status().unwrap_or_default()
    }
}

fn read_status() -> Option<PowerStatus> {
    let mut raw = SYSTEM_POWER_STATUS::default();
    unsafe {
        GetSystemPowerStatus(&mut raw).ok()?;
    }
    Some(convert(&raw))
}

/// Translate the Win32 structure. A desktop with no battery reports
/// `BatteryFlag = 128` and `ACLineStatus = 1`; the percentage is meaningless
/// there and must not become `Some(255)`.
fn convert(raw: &SYSTEM_POWER_STATUS) -> PowerStatus {
    let has_battery =
        raw.BatteryFlag & BATTERY_FLAG_NO_BATTERY == 0 && raw.BatteryFlag != BATTERY_FLAG_UNKNOWN;
    let percent = (has_battery && raw.BatteryLifePercent != BATTERY_PERCENT_UNKNOWN)
        .then_some(raw.BatteryLifePercent.min(100));
    PowerStatus {
        on_battery: has_battery && raw.ACLineStatus == ACLINE_STATUS_OFFLINE,
        percent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(ac: u8, flags: u8, percent: u8) -> SYSTEM_POWER_STATUS {
        SYSTEM_POWER_STATUS {
            ACLineStatus: ac,
            BatteryFlag: flags,
            BatteryLifePercent: percent,
            ..Default::default()
        }
    }

    #[test]
    fn a_desktop_without_a_battery_reports_neither_ac_nor_percent() {
        let status = convert(&raw(1, BATTERY_FLAG_NO_BATTERY, BATTERY_PERCENT_UNKNOWN));
        assert!(!status.on_battery);
        assert_eq!(status.percent, None);
    }

    #[test]
    fn a_discharging_laptop_is_on_battery_with_its_percentage() {
        let status = convert(&raw(ACLINE_STATUS_OFFLINE, 1, 42));
        assert!(status.on_battery);
        assert_eq!(status.percent, Some(42));
    }

    #[test]
    fn a_charging_laptop_is_not_on_battery() {
        let status = convert(&raw(1, 2, 91));
        assert!(!status.on_battery);
        assert_eq!(status.percent, Some(91));
    }

    #[test]
    fn an_unknown_percentage_is_reported_as_absent() {
        let status = convert(&raw(ACLINE_STATUS_OFFLINE, 1, BATTERY_PERCENT_UNKNOWN));
        assert!(status.on_battery);
        assert_eq!(status.percent, None);
    }

    #[test]
    fn an_out_of_range_percentage_is_clamped() {
        // Drivers have shipped 101/255 by accident; publishing that verbatim
        // would overflow the UI's progress bar.
        let status = convert(&raw(1, 1, 101));
        assert_eq!(status.percent, Some(100));
    }

    #[test]
    fn the_live_probe_does_not_panic() {
        let _ = read_status();
    }
}
