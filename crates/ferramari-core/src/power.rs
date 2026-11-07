//! Battery state for the power-saving profile.
//!
//! Linux exposes batteries under `/sys/class/power_supply`. The app only needs
//! to know whether the machine is running on battery and, when available, the
//! remaining percentage; there is no dependency on UPower or D-Bus here.

use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PowerStatus {
    pub on_battery: bool,
    pub percent: Option<u8>,
}

pub fn status() -> PowerStatus {
    read_from(Path::new("/sys/class/power_supply"))
}

fn read_from(root: &Path) -> PowerStatus {
    let Ok(entries) = fs::read_dir(root) else {
        return PowerStatus::default();
    };
    let mut on_battery = false;
    let mut percent: Option<u8> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = fs::read_to_string(path.join("type")) else {
            continue;
        };
        if kind.trim() != "Battery" {
            continue;
        }
        if let Ok(status) = fs::read_to_string(path.join("status")) {
            if status.trim() == "Discharging" {
                on_battery = true;
            }
        }
        if let Ok(capacity) = fs::read_to_string(path.join("capacity")) {
            if let Ok(value) = capacity.trim().parse::<u8>() {
                percent = Some(percent.map_or(value, |current| current.min(value)));
            }
        }
    }
    PowerStatus {
        on_battery,
        percent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn reads_a_discharging_battery() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("ferramari-power-{}-{unique}", std::process::id()));
        let battery = root.join("BAT0");
        fs::create_dir_all(&battery).unwrap();
        fs::write(battery.join("type"), "Battery\n").unwrap();
        fs::write(battery.join("status"), "Discharging\n").unwrap();
        fs::write(battery.join("capacity"), "73\n").unwrap();
        // A second BAT1 at a lower charge wins the percentage.
        let battery2 = root.join("BAT1");
        fs::create_dir_all(&battery2).unwrap();
        fs::write(battery2.join("type"), "Battery\n").unwrap();
        fs::write(battery2.join("status"), "Discharging\n").unwrap();
        fs::write(battery2.join("capacity"), "51\n").unwrap();
        // An AC adapter must be ignored.
        let ac = root.join("AC");
        fs::create_dir_all(&ac).unwrap();
        fs::write(ac.join("type"), "Mains\n").unwrap();
        fs::write(ac.join("online"), "0\n").unwrap();

        let status = read_from(&root);
        assert!(status.on_battery);
        assert_eq!(status.percent, Some(51));

        fs::write(battery.join("status"), "Charging\n").unwrap();
        fs::write(battery2.join("status"), "Charging\n").unwrap();
        let status = read_from(&root);
        assert!(!status.on_battery);
        assert_eq!(status.percent, Some(51));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_supply_is_not_an_error() {
        assert_eq!(
            read_from(Path::new("/definitely/not/a/power/supply")),
            PowerStatus::default()
        );
    }
}
