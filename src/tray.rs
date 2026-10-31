//! StatusNotifierItem tray icon (works with GNOME's AppIndicator extension,
//! KDE, and any other SNI host). No GTK dependency, so it is safe to spawn
//! from the main process.

use ksni::blocking::{Handle, TrayMethods};

use crate::util::tr;
use ksni::menu::StandardItem;
use ksni::{MenuItem, Tray as KsniTray};

use crate::app::{Command, CommandSender};
use crate::paths;

pub struct Tray {
    tx: CommandSender,
}

impl KsniTray for Tray {
    fn id(&self) -> String {
        paths::APP_ID.to_string()
    }

    fn title(&self) -> String {
        "Ferramari".to_string()
    }

    fn icon_name(&self) -> String {
        paths::APP_ID.to_string()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::SystemServices
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(Command::ShowGui);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            item("Show Ferramari", |tray| {
                let _ = tray.tx.send(Command::ShowGui);
            }),
            item("Toggle Mute Audio", |tray| {
                let _ = tray.tx.send(Command::ToggleMute);
            }),
            item("Toggle Play/Pause", |tray| {
                let _ = tray.tx.send(Command::TogglePause);
            }),
            item("Reload", |tray| {
                let _ = tray.tx.send(Command::Reload);
            }),
            item("I'm Feeling Lucky", |tray| {
                let _ = tray.tx.send(Command::FeelingLucky);
            }),
            MenuItem::Separator,
            item("Quit Ferramari", |tray| {
                let _ = tray.tx.send(Command::Quit);
            }),
        ]
    }
}

fn item<F>(label: &str, activate: F) -> MenuItem<Tray>
where
    F: Fn(&mut Tray) + Send + 'static,
{
    MenuItem::Standard(StandardItem {
        label: tr(label),
        activate: Box::new(activate),
        ..Default::default()
    })
}

/// Spawn the tray. Returns a handle so the caller can keep it alive; `None`
/// when the desktop has no StatusNotifierWatcher.
pub fn spawn(tx: CommandSender) -> Option<Handle<Tray>> {
    match (Tray { tx }).assume_sni_available(true).spawn() {
        Ok(handle) => {
            log::info!("[Systray] Ready");
            Some(handle)
        }
        Err(err) => {
            log::warn!("[Systray] Unavailable: {err}");
            None
        }
    }
}
