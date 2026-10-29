//! D-Bus API. The interface shape follows schema version 4; the names belong
//! to Ferramari.
//!
//! Even though the GUI, player and server live in one process, the same two
//! interfaces are exported so existing scripts and desktop integrations keep
//! working. Method and property names use the established snake_case spelling
//! with `#[zbus(name = ...)]`.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use crate::app::{Command, Snapshot};
use crate::paths;

pub const SERVER_INTERFACE: &str = "io.github.ferramari.ferramari.server";
#[allow(dead_code)] // documented API surface, asserted in tests
pub const PLAYER_INTERFACE: &str = "io.github.ferramari.ferramari.player";
pub const SERVER_PATH: &str = "/io/github/ferramari/ferramari/server";
pub const PLAYER_PATH: &str = "/io/github/ferramari/ferramari/player";

pub enum Acquired {
    Primary(zbus::blocking::Connection),
    AlreadyRunning,
}

/// Claim the server name, or report that another instance owns it.
pub fn acquire() -> anyhow::Result<Acquired> {
    let connection = zbus::blocking::Connection::session()?;

    // Ask the bus first: `request_name` reports a taken name as an error, and
    // the distinction between "someone else owns it" and "D-Bus is broken"
    // matters for the second-instance path.
    let reply = connection.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &(paths::SERVER_NAME,),
    )?;
    let has_owner: bool = reply.body().deserialize()?;
    if has_owner {
        return Ok(Acquired::AlreadyRunning);
    }

    let flags = zbus::fdo::RequestNameFlags::DoNotQueue.into();
    match connection.request_name_with_flags(paths::SERVER_NAME, flags) {
        Ok(_) => Ok(Acquired::Primary(connection)),
        // Lost a race with another instance.
        Err(err) => {
            log::debug!("[DBus] Name request failed: {err}");
            Ok(Acquired::AlreadyRunning)
        }
    }
}

/// Ask the running instance to show its window.
pub fn request_show_gui() -> anyhow::Result<()> {
    use zbus::blocking::{Connection, Proxy};
    let connection = Connection::session()?;
    let proxy = Proxy::new(
        &connection,
        paths::SERVER_NAME,
        SERVER_PATH,
        SERVER_INTERFACE,
    )?;
    proxy.call::<_, _, ()>("show_gui", &())?;
    Ok(())
}

pub fn register(
    connection: &zbus::blocking::Connection,
    tx: Sender<Command>,
    snapshot: Arc<Mutex<Snapshot>>,
) -> anyhow::Result<()> {
    connection.object_server().at(
        SERVER_PATH,
        ServerInterface {
            tx: tx.clone(),
            snapshot: Arc::clone(&snapshot),
        },
    )?;
    connection
        .object_server()
        .at(PLAYER_PATH, PlayerInterface { tx, snapshot })?;
    Ok(())
}

fn send(tx: &Sender<Command>, command: Command) {
    if tx.send(command).is_err() {
        log::debug!("[DBus] Main loop is gone, dropping command");
    }
}

pub struct ServerInterface {
    tx: Sender<Command>,
    pub snapshot: Arc<Mutex<Snapshot>>,
}

impl ServerInterface {
    fn snapshot(&self) -> Snapshot {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default()
    }
}

#[zbus::interface(name = "io.github.ferramari.ferramari.server")]
impl ServerInterface {
    #[zbus(name = "null")]
    fn null(&self) {
        send(&self.tx, Command::Null);
    }

    #[zbus(name = "video")]
    fn video(&self, video_path: String, monitor: String) {
        send(
            &self.tx,
            Command::Video {
                path: (!video_path.is_empty()).then_some(video_path),
                monitor: (!monitor.is_empty()).then_some(monitor),
            },
        );
    }

    #[zbus(name = "stream")]
    fn stream(&self, stream_url: String) {
        send(&self.tx, Command::Stream { url: stream_url });
    }

    #[zbus(name = "webpage")]
    fn webpage(&self, webpage_url: String) {
        send(&self.tx, Command::Webpage { url: webpage_url });
    }

    #[zbus(name = "pause_playback")]
    fn pause_playback(&self) {
        send(&self.tx, Command::PausePlayback);
    }

    #[zbus(name = "start_playback")]
    fn start_playback(&self) {
        send(&self.tx, Command::StartPlayback);
    }

    #[zbus(name = "reload")]
    fn reload(&self) {
        send(&self.tx, Command::Reload);
    }

    #[zbus(name = "feeling_lucky")]
    fn feeling_lucky(&self) {
        send(&self.tx, Command::FeelingLucky);
    }

    #[zbus(name = "show_gui")]
    fn show_gui(&self) {
        send(&self.tx, Command::ShowGui);
    }

    #[zbus(name = "quit")]
    fn quit(&self) {
        send(&self.tx, Command::Quit);
    }

    #[zbus(property, name = "mode")]
    fn mode(&self) -> String {
        self.snapshot().mode.as_str().to_string()
    }

    #[zbus(property, name = "volume")]
    fn volume(&self) -> i32 {
        self.snapshot().volume as i32
    }

    #[zbus(property, name = "volume")]
    fn set_volume(&self, volume: i32) {
        send(&self.tx, Command::SetVolume(volume as i64));
    }

    #[zbus(property, name = "blur_radius")]
    fn blur_radius(&self) -> i32 {
        self.snapshot().blur_radius as i32
    }

    #[zbus(property, name = "blur_radius")]
    fn set_blur_radius(&self, radius: i32) {
        send(&self.tx, Command::SetBlurRadius(radius as i64));
    }

    #[zbus(property, name = "is_mute")]
    fn is_mute(&self) -> bool {
        self.snapshot().is_mute
    }

    #[zbus(property, name = "is_mute")]
    fn set_is_mute(&self, is_mute: bool) {
        send(&self.tx, Command::SetMute(is_mute));
    }

    #[zbus(property, name = "is_playing")]
    fn is_playing(&self) -> bool {
        self.snapshot().is_playing
    }

    #[zbus(property, name = "is_paused_by_user")]
    fn is_paused_by_user(&self) -> bool {
        self.snapshot().is_paused_by_user
    }

    #[zbus(property, name = "is_paused_by_user")]
    fn set_is_paused_by_user(&self, paused: bool) {
        send(&self.tx, Command::SetPausedByUser(paused));
    }

    #[zbus(property, name = "is_static_wallpaper")]
    fn is_static_wallpaper(&self) -> bool {
        self.snapshot().is_static_wallpaper
    }

    #[zbus(property, name = "is_static_wallpaper")]
    fn set_is_static_wallpaper(&self, enabled: bool) {
        send(&self.tx, Command::SetStaticWallpaper(enabled));
    }

    #[zbus(property, name = "is_pause_when_maximized")]
    fn is_pause_when_maximized(&self) -> bool {
        self.snapshot().is_pause_when_maximized
    }

    #[zbus(property, name = "is_pause_when_maximized")]
    fn set_is_pause_when_maximized(&self, enabled: bool) {
        send(&self.tx, Command::SetPauseWhenMaximized(enabled));
    }

    #[zbus(property, name = "is_mute_when_maximized")]
    fn is_mute_when_maximized(&self) -> bool {
        self.snapshot().is_mute_when_maximized
    }

    #[zbus(property, name = "is_mute_when_maximized")]
    fn set_is_mute_when_maximized(&self, enabled: bool) {
        send(&self.tx, Command::SetMuteWhenMaximized(enabled));
    }
}

pub struct PlayerInterface {
    tx: Sender<Command>,
    snapshot: Arc<Mutex<Snapshot>>,
}

impl PlayerInterface {
    fn snapshot(&self) -> Snapshot {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default()
    }
}

#[zbus::interface(name = "io.github.ferramari.ferramari.player")]
impl PlayerInterface {
    #[zbus(name = "pause_playback")]
    fn pause_playback(&self) {
        send(&self.tx, Command::PausePlayback);
    }

    #[zbus(name = "start_playback")]
    fn start_playback(&self) {
        send(&self.tx, Command::StartPlayback);
    }

    #[zbus(name = "quit_player")]
    fn quit_player(&self) {
        send(&self.tx, Command::QuitPlayer);
    }

    #[zbus(name = "reload_config")]
    fn reload_config(&self) {
        send(&self.tx, Command::ReloadConfig);
    }

    #[zbus(property, name = "mode")]
    fn mode(&self) -> String {
        self.snapshot().mode.as_str().to_string()
    }

    #[zbus(property, name = "data_source")]
    fn data_source(&self) -> HashMap<String, String> {
        self.snapshot().data_source
    }

    #[zbus(property, name = "volume")]
    fn volume(&self) -> i32 {
        self.snapshot().volume as i32
    }

    #[zbus(property, name = "volume")]
    fn set_volume(&self, volume: i32) {
        send(&self.tx, Command::SetVolume(volume as i64));
    }

    #[zbus(property, name = "is_mute")]
    fn is_mute(&self) -> bool {
        self.snapshot().is_mute
    }

    #[zbus(property, name = "is_mute")]
    fn set_is_mute(&self, is_mute: bool) {
        send(&self.tx, Command::SetMute(is_mute));
    }

    #[zbus(property, name = "is_playing")]
    fn is_playing(&self) -> bool {
        self.snapshot().is_playing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_match_pydbus_defaults() {
        assert_eq!(SERVER_PATH, "/io/github/ferramari/ferramari/server");
        assert_eq!(PLAYER_PATH, "/io/github/ferramari/ferramari/player");
        assert!(SERVER_INTERFACE.ends_with(".server"));
        assert!(PLAYER_INTERFACE.ends_with(".player"));
    }
}
