//! Command-line interface.

use clap::Parser;

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
}
