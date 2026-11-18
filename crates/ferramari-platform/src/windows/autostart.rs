//! Autostart through the per-user `Run` key.
//!
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` is the documented
//! per-user location and needs no elevation. The value launches the server in
//! background mode (`-b`), exactly like the Linux desktop entry.

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, REG_VALUE_TYPE,
};

use crate::win32util::wide;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Ferramari";

/// The command line stored in the registry. Quoted because the executable can
/// live under `C:\Program Files`, where an unquoted path with spaces is a
/// classic Windows startup bug.
fn command_line(exe: &Path) -> String {
    format!("\"{}\" -b", exe.display())
}

fn open_key(access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Option<HKEY> {
    let subkey = wide(RUN_KEY);
    let mut key = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            access,
            &mut key,
        )
    };
    (status == ERROR_SUCCESS).then_some(key)
}

pub fn is_enabled() -> bool {
    let Some(key) = open_key(KEY_QUERY_VALUE) else {
        return false;
    };
    let name = wide(VALUE_NAME);
    let mut kind = REG_VALUE_TYPE::default();
    let mut size = 0u32;
    let status = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            None,
            Some(&mut size),
        )
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    status == ERROR_SUCCESS && size > 0
}

pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    let key = open_key(KEY_SET_VALUE)
        .ok_or_else(|| anyhow::anyhow!("could not open HKCU\\{RUN_KEY} for writing"))?;
    let result = if enabled {
        let exe =
            std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("ferramari.exe"));
        // The launcher is a GUI-subsystem binary: autostart must not flash a
        // console window. It passes `-b` through to the server.
        let launcher = exe.with_file_name("ferramari-launcher.exe");
        let target = if launcher.exists() { launcher } else { exe };
        let command = wide(&command_line(&target));
        // The registry stores bytes; `REG_SZ` is UTF-16 including the NUL.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                command.as_ptr().cast::<u8>(),
                command.len() * std::mem::size_of::<u16>(),
            )
        };
        let name = wide(VALUE_NAME);
        let status =
            unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes)) };
        if status == ERROR_SUCCESS {
            log::info!("[Autostart] Enabled ({})", target.display());
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "RegSetValueExW failed with error {}",
                status.0
            ))
        }
    } else {
        let name = wide(VALUE_NAME);
        let status = unsafe { RegDeleteValueW(key, PCWSTR(name.as_ptr())) };
        if status == ERROR_SUCCESS {
            log::info!("[Autostart] Disabled");
            Ok(())
        } else if status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "RegDeleteValueW failed with error {}",
                status.0
            ))
        }
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_quotes_the_executable() {
        let line = command_line(Path::new(r"C:\Program Files\Ferramari\ferramari.exe"));
        assert_eq!(line, r#""C:\Program Files\Ferramari\ferramari.exe" -b"#);
    }

    #[test]
    fn reading_the_registry_does_not_panic() {
        let _ = is_enabled();
    }
}
