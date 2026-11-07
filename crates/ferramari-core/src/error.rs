//! User-facing error classification.
//!
//! Internal code still uses `anyhow` for context. When an error reaches the
//! command loop it is wrapped in a [`UserError`], which keeps the summary and
//! technical detail separate: the GUI gets a readable line while the log keeps
//! the full cause.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Config,
    FileSystem,
    Network,
    Playback,
    External,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Config => "config",
            ErrorKind::FileSystem => "filesystem",
            ErrorKind::Network => "network",
            ErrorKind::Playback => "playback",
            ErrorKind::External => "external",
        }
    }
}

#[derive(Debug, Clone)]
pub struct UserError {
    pub kind: ErrorKind,
    pub summary: String,
    pub detail: Option<String>,
}

impl UserError {
    pub fn new(kind: ErrorKind, summary: impl Into<String>) -> Self {
        Self {
            kind,
            summary: summary.into(),
            detail: None,
        }
    }

    pub fn with_detail(
        kind: ErrorKind,
        summary: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            summary: summary.into(),
            detail: Some(detail.into()),
        }
    }

    pub fn message(&self) -> String {
        match &self.detail {
            Some(detail) if !detail.trim().is_empty() => {
                format!("{}: {detail}", self.summary)
            }
            _ => self.summary.clone(),
        }
    }

    pub fn log(&self) {
        match &self.detail {
            Some(detail) if !detail.trim().is_empty() => {
                log::error!("[{}] {}: {detail}", self.kind.as_str(), self.summary);
            }
            _ => log::error!("[{}] {}", self.kind.as_str(), self.summary),
        }
    }
}

impl fmt::Display for UserError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_is_appended_to_the_summary() {
        let error = UserError::with_detail(
            ErrorKind::Playback,
            "Could not play video.mp4",
            "mpv exited with status 1",
        );
        assert_eq!(
            error.message(),
            "Could not play video.mp4: mpv exited with status 1"
        );
    }

    #[test]
    fn empty_detail_is_ignored() {
        let error = UserError::with_detail(ErrorKind::Network, "Could not connect", "  ");
        assert_eq!(error.message(), "Could not connect");
    }

    #[test]
    fn kinds_have_stable_log_labels() {
        assert_eq!(ErrorKind::FileSystem.as_str(), "filesystem");
        assert_eq!(ErrorKind::Playback.as_str(), "playback");
    }
}
