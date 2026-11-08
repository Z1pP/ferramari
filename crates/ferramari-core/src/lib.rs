//! Ferramari core: toolkit-independent business logic, the command bus, and
//! the data models shared across the platform, media and UI layers.
//!
//! This crate must never depend on GTK/GDK, Iced, or any windowing backend.
//! It is compiled on every target and is the layer that stays put through the
//! GTK → Iced migration.

pub mod bus;
pub mod catalog_api;
pub mod catalogue;
pub mod cli;
pub mod config;
pub mod display_ids;
pub mod eco;
pub mod error;
pub mod hwdec;
pub mod library;
pub mod lint;
pub mod metrics;
pub mod monitor;
pub mod package;
pub mod paths;
pub mod platform;
pub mod plugin;
pub mod power;
pub mod session;
pub mod signing;
pub mod supervisor;
pub mod thumbnails;
pub mod util;
pub mod window;
pub mod ytdlp;

// Convenience aliases so call sites read the way the modules are known.
pub use bus::{Command, CommandReceiver, CommandSender, PackageReport, Snapshot};
pub use monitor::{display_name, geometry_matches, DisplayIdentity, MonitorInfo};
