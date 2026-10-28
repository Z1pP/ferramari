//! Ferramari — video wallpaper for Linux, written in Rust.

mod app;
mod autostart;
mod cli;
mod config;
mod dbus;
mod fade;
mod handlers;
mod monitors;
mod mpv;
mod paths;
mod thumbnails;
mod tray;
mod ui;
mod util;
mod wallpaper;
mod x11;
mod ytdlp;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use clap::Parser;
use gtk::prelude::*;

use app::{App, Command, Snapshot};

fn main() {
    paths::prepare_env();
    let args = cli::Args::parse();
    init_logging(args.debug);
    init_i18n();

    log::info!(
        "Ferramari v{}\n--- System information ---\n\
         is_gnome = {}\n\
         is_wayland = {}\n\
         is_flatpak = {}\n\
         --------------------------",
        env!("CARGO_PKG_VERSION"),
        paths::is_gnome(),
        paths::is_wayland(),
        paths::is_flatpak()
    );

    let (tx, rx) = mpsc::channel();
    let snapshot = Arc::new(Mutex::new(Snapshot::default()));

    // Single instance: a second launch only asks the first to show its GUI.
    let dbus_connection = match dbus::acquire() {
        Ok(dbus::Acquired::AlreadyRunning) => {
            let _ = dbus::request_show_gui();
            return;
        }
        Ok(dbus::Acquired::Primary(connection)) => {
            if args.pause > 0 {
                log::info!("[Server] Starting in {} second(s)", args.pause);
                std::thread::sleep(Duration::from_secs(args.pause));
            }
            if args.reset {
                config::Config::reset(&[]);
            }
            if let Err(err) = dbus::register(&connection, tx.clone(), Arc::clone(&snapshot)) {
                log::warn!("[DBus] Could not export interfaces: {err}");
            }
            Some(connection)
        }
        Err(err) => {
            log::warn!("[DBus] Unavailable, continuing without external API: {err}");
            None
        }
    };

    let gtk_app =
        gtk::Application::new(Some(paths::APP_ID), gtk::gio::ApplicationFlags::FLAGS_NONE);

    let state: Rc<RefCell<Option<Rc<RefCell<App>>>>> = Rc::new(RefCell::new(None));
    let receiver = RefCell::new(Some(rx));

    {
        let state = Rc::clone(&state);
        let tx = tx.clone();
        let snapshot = Arc::clone(&snapshot);
        let args = args.clone();
        gtk_app.connect_startup(move |gtk_app| {
            let rx = receiver
                .borrow_mut()
                .take()
                .expect("startup handler runs exactly once");
            let app = App::new(gtk_app, &args, tx.clone(), rx, Arc::clone(&snapshot));
            *state.borrow_mut() = Some(app);
        });
    }
    {
        let state = Rc::clone(&state);
        gtk_app.connect_activate(move |_| {
            if let Some(app) = state.borrow().as_ref() {
                app.borrow_mut().activate();
            }
        });
    }
    {
        let state = Rc::clone(&state);
        gtk_app.connect_shutdown(move |_| {
            if let Some(app) = state.borrow().as_ref() {
                app.borrow_mut().shutdown();
            }
        });
    }

    for signal in [libc::SIGINT, libc::SIGTERM] {
        let tx = tx.clone();
        glib::unix_signal_add(signal, move || {
            let _ = tx.send(Command::Quit);
            glib::ControlFlow::Continue
        });
    }

    log::info!("[Server] Started");
    gtk_app.run_with_args::<&str>(&[]);

    // Keep the D-Bus connection alive until the GTK loop has finished.
    drop(dbus_connection);
    log::info!("[Server] Stopped");
}

fn init_logging(debug: bool) {
    let default = if debug { "debug" } else { "info" };
    let env = env_logger::Env::default().default_filter_or(default);
    env_logger::Builder::from_env(env)
        .format_timestamp_millis()
        .init();
}

fn init_i18n() {
    // SAFETY: called before any thread is spawned, from the main thread, and
    // the locale string is a valid C string literal.
    unsafe {
        let _ = gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "");
    }
    let localedir = locale_dir();
    let _ = gettextrs::bindtextdomain(paths::TRANSLATION_DOMAIN, &localedir);
    let _ = gettextrs::textdomain(paths::TRANSLATION_DOMAIN);
}

/// `FERRAMARI_LOCALEDIR` wins; otherwise look next to the executable so a
/// non-default `PREFIX` (`/usr/local`, a staged install, Guix, Nix, …) finds
/// its own catalogs.
fn locale_dir() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("FERRAMARI_LOCALEDIR") {
        return dir.into();
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(prefix) = exe.parent().and_then(|bin| bin.parent()) {
            let candidate = prefix.join("share/locale");
            if candidate.is_dir() {
                return candidate;
            }
        }
    }
    std::path::PathBuf::from("/usr/share/locale")
}
