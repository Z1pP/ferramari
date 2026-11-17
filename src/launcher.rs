#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! Windows GUI entry point for Ferramari.
//!
//! Double-clicking `ferramari.exe` or launching it from the Start menu would
//! otherwise create a console window for the server. This tiny GUI-subsystem
//! launcher starts the server with `CREATE_NO_WINDOW`, forwards its arguments
//! (`-b` for autostart, `-d` for a deliberate debug session) and exits. The
//! server then opens the Iced control surface as usual.
//!
//! The server and the UI stay ordinary executables: `ferramari.exe` keeps its
//! console for `--doctor`, `--version`, package commands and debug logs, and
//! `ferramari-ui.exe` is already a GUI-subsystem binary.

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn server_path() -> PathBuf {
    let name = if cfg!(windows) {
        "ferramari.exe"
    } else {
        "ferramari"
    };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|directory| directory.join(name)))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn main() {
    let mut command = Command::new(server_path());
    command
        .args(std::env::args_os().skip(1))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: a console-subsystem server gets no console at all.
        command.creation_flags(0x0800_0000);
    }

    let _ = command.spawn();
}
