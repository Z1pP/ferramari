//! Ferramari media layer: mpv controller, GTK-free volume fades, native X11
//! wallpaper surfaces, and the playback engine that binds the three together.
//!
//! No GTK/GDK here — mpv is driven over its JSON IPC socket and draws into a
//! window this crate creates itself through x11rb.

pub mod engine;
pub mod fade;
pub mod mpv;
