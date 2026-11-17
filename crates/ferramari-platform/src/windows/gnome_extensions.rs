//! Placeholder for the GNOME desktop-icon workaround on Windows.
//!
//! Explorer keeps the icon layer (`SHELLDLL_DefView`) above the `WorkerW`
//! host, and it redraws it itself after an Explorer restart. No extension
//! restart or shell command is needed, so the call site in the server stays
//! shared and this backend deliberately does nothing.

/// Explorer owns the layering, so there is no workaround to apply.
pub fn gnome_desktop_icon_workaround() {}
