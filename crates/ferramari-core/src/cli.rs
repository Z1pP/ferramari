//! Command-line interface.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Clone, Parser)]
#[command(name = "ferramari", version, about = "Video wallpaper for Linux")]
pub struct Args {
    /// Add pause before launching Ferramari [sec]
    #[arg(short = 'p', long = "pause", value_name = "SEC", default_value_t = 0)]
    pub pause: u64,

    /// Launch only the live wallpaper
    #[arg(short = 'b', long = "background")]
    pub background: bool,

    /// Print debug messages
    #[arg(short = 'd', long = "debug")]
    pub debug: bool,

    /// Reset user configuration
    #[arg(short = 'r', long = "reset")]
    pub reset: bool,

    /// Print desktop/session/tool diagnostics and exit
    #[arg(long = "doctor")]
    pub doctor: bool,

    /// Fetch page 1 from a Ferramari Catalog API and print its items
    #[arg(long = "catalog", value_name = "URL")]
    pub catalog: Option<String>,

    /// Work with `.ferramari` packages; exits without starting the wallpaper
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Creator tooling for `.ferramari` packages
    #[command(subcommand)]
    Package(PackageCommand),
}

#[derive(Debug, Clone, Subcommand)]
pub enum PackageCommand {
    /// Show or create the local Ed25519 signing key
    Key {
        /// Name recorded on the key
        #[arg(long, default_value = "")]
        name: String,
        /// Write the public half to this file
        #[arg(long, value_name = "PATH")]
        export: Option<PathBuf>,
    },
    /// Checksum and sign a package directory
    Sign {
        /// The `.ferramari` package directory
        dir: PathBuf,
        /// Name recorded on the key, if it has to be created
        #[arg(long, default_value = "")]
        name: String,
    },
    /// Verify checksums and the signature of a package directory
    Verify {
        /// The `.ferramari` package directory
        dir: PathBuf,
    },
    /// Review a package directory the way a catalogue would show it
    Lint {
        /// The `.ferramari` package directory
        dir: PathBuf,
        /// Machine-readable findings (for CI)
        #[arg(long)]
        json: bool,
        /// Suppress findings by code (comma-separated); unknown codes fail
        #[arg(long, value_name = "CODES")]
        ignore: Option<String>,
        /// Fail the exit code for any warning, not only blockers
        #[arg(long)]
        strict: bool,
    },
    /// Trust a key: a package directory with a signature, or a public key file
    Trust {
        /// Package directory or `*.json` public key
        source: PathBuf,
    },
    /// List the keys currently trusted
    Keys,
}

/// Exit codes for `ferramari package verify`.
pub const VERIFY_OK: i32 = 0;
pub const VERIFY_NOT_TRUSTED: i32 = 1;
pub const VERIFY_INVALID: i32 = 2;

/// Exit codes for `ferramari package lint`.
///
/// Warnings exit 0 by default: the point of splitting findings into two
/// severities is that a creator's CI can require "nothing blocks
/// publication" without an incomplete-but-valid package failing the build.
/// `--strict` promotes warnings to failures for those who want the bar higher.
pub const LINT_OK: i32 = 0;
pub const LINT_BLOCKED: i32 = 2;
pub const LINT_BAD_INPUT: i32 = 3;
