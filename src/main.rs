//! Ferramari — video wallpaper for Linux, written in Rust.
//!
//! The server owns the wallpaper players and answers D-Bus. It has no GUI:
//! the control surface is a separate binary (`ferramari-ui`) that talks to
//! this process over the session bus, so nothing here links a toolkit.

// Business logic, the command bus and the data models live in `ferramari-core`.
// The binary re-exports the modules under their historic names so that `app.rs`
// keeps referring to `crate::paths`, `crate::util`, … unchanged.
pub use ferramari_core::bus::{Command, CommandReceiver, CommandSender, PackageReport, Snapshot};
pub use ferramari_core::MonitorInfo;
pub use ferramari_core::{
    bus, catalog_api, catalogue, cli, config, display_ids, eco, error, library, metrics, package,
    paths, plugin, signing, supervisor, thumbnails, util, window, ytdlp,
};
pub use ferramari_media::mpv;
pub use ferramari_platform::{
    autostart, dbus, desktop, event_source, gnome_bg, gnome_extensions, handlers, monitors, power,
    surface, tray, x11,
};
// The static-background module kept its historic name in the binary.
pub use ferramari_platform::gnome_bg as wallpaper;

mod app;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::Parser;

use app::App;
use cli::PackageCommand;

fn main() {
    let args = cli::Args::parse();
    init_logging(args.debug);
    // GNU gettext gives `LANGUAGE` priority over the system locale. The
    // in-app choice lets someone run a Russian UI on an English desktop.
    if let Some(language) = config::Config::language_override() {
        std::env::set_var("LANGUAGE", language);
    }
    init_i18n();

    if let Some(cli::Command::Package(command)) = args.command.clone() {
        std::process::exit(run_package_command(command));
    }
    if args.doctor {
        println!("{}", desktop::doctor());
        return;
    }
    if let Some(base) = args.catalog.as_deref() {
        let result = catalog_api::Client::new(base).and_then(|client| client.list(1, None, None));
        match result {
            Ok(page) => {
                println!("page {} next {:?}", page.page, page.next);
                for item in page.items {
                    println!("{}\t{}", item.id, item.name);
                }
            }
            Err(err) => {
                eprintln!("catalog: {err}");
                std::process::exit(2);
            }
        }
        return;
    }

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

    let (command_tx, rx) = std::sync::mpsc::channel();
    let tx = CommandSender::new(command_tx);
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

    install_signal_handlers(&tx);

    log::info!("[Server] Started");
    let app = App::new(
        &args,
        tx.clone(),
        Arc::clone(&snapshot),
        Box::new(surface::X11SurfaceProvider),
        Box::new(monitors::X11DisplayProvider),
        Box::new(power::LinuxPowerProvider),
        Box::new(event_source::LinuxPlatformEventSource),
    );
    // Start the players (and the control surface unless `--background`)
    // before waiting for commands, matching the server's startup contract.
    app.borrow_mut().activate();
    App::run(&app, rx);
    log::info!("[Server] Stopped");

    // Keep the D-Bus connection alive until the loop has finished.
    drop(dbus_connection);
}

/// SIGINT/SIGTERM are blocked process-wide and delivered by one thread.
///
/// Blocking them before any thread starts is what makes this work: without
/// it the default action would kill the process mid-fade, and a handler
/// could not send on the command channel without risking an async-signal-
/// unsafe call. `sigwait` returns only for the signals we asked for.
#[cfg(unix)]
fn install_signal_handlers(tx: &CommandSender) {
    let tx = tx.clone();
    std::thread::Builder::new()
        .name("ferramari-signal".to_string())
        .spawn(move || {
            let signals = [libc::SIGINT, libc::SIGTERM];
            let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
            unsafe {
                libc::sigemptyset(&mut set);
                for signal in signals {
                    libc::sigaddset(&mut set, signal);
                    libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
                }
            }
            let mut received = 0;
            loop {
                if unsafe { libc::sigwait(&set, &mut received) } != 0 {
                    log::warn!("[Signal] sigwait failed, stopping signal thread");
                    return;
                }
                log::info!("[Signal] {received}");
                if tx.send(Command::Quit).is_err() {
                    return;
                }
            }
        })
        .ok();
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

/// `ferramari package …`: creator-facing tooling for `.ferramari` packages.
/// Runs before the server starts, so it works over SSH and headless.
fn run_package_command(command: PackageCommand) -> i32 {
    match command {
        PackageCommand::Key { name, export } => {
            let name = author_name(&name);
            match signing::load_or_create_key(&name) {
                Ok(key) => {
                    println!("key id: {}", key.key_id);
                    println!("name:   {}", key.name);
                    println!("public: {}", key.public);
                    println!("store:  {}", signing::key_path().display());
                    if let Some(path) = export {
                        if let Err(err) = std::fs::write(&path, signing::public_key_document(&key))
                        {
                            eprintln!("could not write {}: {err}", path.display());
                            return 2;
                        }
                        println!("wrote public key: {}", path.display());
                    }
                    0
                }
                Err(err) => {
                    eprintln!("{err}");
                    2
                }
            }
        }
        PackageCommand::Sign { dir, name } => {
            let name = author_name(&name);
            match signing::load_or_create_key(&name)
                .and_then(|key| package::write_checksums(&dir).map(|()| key))
                .and_then(|key| signing::sign_package(&dir, &key))
            {
                Ok(()) => {
                    println!("signed {}", dir.display());
                    0
                }
                Err(err) => {
                    eprintln!("could not sign {}: {err}", dir.display());
                    2
                }
            }
        }
        PackageCommand::Verify { dir } => {
            let package = match package::load(&dir) {
                Ok(package) => package,
                Err(err) => {
                    eprintln!("INVALID {}: {err}", dir.display());
                    return cli::VERIFY_INVALID;
                }
            };
            let integrity = if package.checksummed {
                "checksums ok"
            } else {
                "no checksums.json"
            };
            match signing::verify_package(&package.root) {
                Ok(status) => {
                    let code = match &status {
                        signing::SignatureStatus::Trusted { .. } => cli::VERIFY_OK,
                        signing::SignatureStatus::Unsigned
                        | signing::SignatureStatus::Unknown { .. } => cli::VERIFY_NOT_TRUSTED,
                        signing::SignatureStatus::Invalid { .. } => cli::VERIFY_INVALID,
                    };
                    println!("{}: {integrity}; {}", dir.display(), status.label());
                    code
                }
                Err(err) => {
                    eprintln!("INVALID {}: {err}", dir.display());
                    cli::VERIFY_INVALID
                }
            }
        }
        PackageCommand::Trust { source } => match signing::trust_source(&source) {
            Ok(Some(key_id)) => {
                println!("trusted key {key_id}");
                0
            }
            Ok(None) => {
                eprintln!("{} carries no key to trust", source.display());
                2
            }
            Err(err) => {
                eprintln!("could not trust {}: {err}", source.display());
                2
            }
        },
        PackageCommand::Keys => {
            let keys = signing::trusted_keys();
            if keys.is_empty() {
                println!("no trusted keys");
                return 0;
            }
            for key in keys {
                println!("{}\t{}", key.key_id, key.name);
            }
            0
        }
    }
}

fn author_name(requested: &str) -> String {
    if !requested.trim().is_empty() {
        return requested.trim().to_string();
    }
    signing::default_author_name()
}

#[cfg(not(unix))]
fn install_signal_handlers(_tx: &CommandSender) {
    // Native console-control handling belongs to the Windows platform
    // backend. Until that backend is enabled, retain the OS default behavior.
}
