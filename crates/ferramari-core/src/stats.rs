//! What the running decoder reports, flattened into plain data.
//!
//! The Home hero and the Performance page need more than the settings
//! snapshot: the frame rate the decoder produces, the codec it picked, the
//! geometry it is working with and how many frames it dropped. Those facts
//! live in mpv, so the media layer reads them over JSON IPC, publishes them
//! through the command bus, and the control surface receives them over D-Bus.
//!
//! Nothing in this module reaches for mpv, X11 or D-Bus types, and every
//! number is real or absent — never guessed. A field the player has not
//! answered stays `None` and the UI says "unknown", which is the same rule the
//! metrics module follows for CPU and memory.

/// Everything one probe of the primary player learned.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaybackStats {
    /// `hwdec-current`: API name, `no` for software, empty when unknown.
    pub hwdec: String,
    /// `container-fps`, the rate the file was authored at.
    pub fps: Option<f64>,
    /// `video-codec`, mpv's long description.
    pub codec: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// `frame-drop-count` since the file was loaded.
    pub dropped: Option<u64>,
    pub duration: Option<f64>,
    pub position: Option<f64>,
    /// True once the probe produced anything at all, so an idle player is
    /// distinguishable from a player that decodes at exactly zero.
    pub active: bool,
}

impl PlaybackStats {
    /// `1920 × 1080`, empty when the geometry is not known yet.
    pub fn resolution_text(&self) -> String {
        match (self.width, self.height) {
            (Some(width), Some(height)) if width > 0 && height > 0 => {
                format!("{width} × {height}")
            }
            _ => String::new(),
        }
    }

    /// mpv separates codec families with ` / `; the first token is the name
    /// users recognise (`H.264` out of `H.264 / AVC / MPEG-4 AVC / …`).
    pub fn codec_short(&self) -> &str {
        self.codec
            .split(" / ")
            .next()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(self.codec.as_str())
    }

    /// `vaapi` → `hwdec (vaapi)`. mpv answers `no` when the requested hardware
    /// API silently fell back, and that is worth naming rather than hiding.
    pub fn decoder_label(&self) -> String {
        if self.hwdec.is_empty() || self.hwdec == "no" {
            "hwdec (software)".to_string()
        } else {
            format!("hwdec ({})", self.hwdec)
        }
    }

    /// Dropped frames as a share of everything decoded so far, which is what
    /// makes a raw count mean something. `None` when the baseline is unknown.
    pub fn drop_percent(&self) -> Option<f64> {
        let dropped = self.dropped?;
        let fps = self.fps.filter(|value| *value > 0.0)?;
        let elapsed = self.position.filter(|value| *value > 0.0)?;
        let decoded = fps * elapsed;
        if decoded <= 0.0 {
            return None;
        }
        Some((dropped as f64 / decoded) * 100.0)
    }

    /// The flat string map exported over D-Bus. An unknown number is an empty
    /// value rather than a zero, so a client can tell "not measured" from
    /// "measured as none".
    pub fn to_bus_map(&self) -> std::collections::HashMap<String, String> {
        let number = |value: Option<f64>| match value {
            Some(value) if value.is_finite() => format!("{value}"),
            _ => String::new(),
        };
        let integer = |value: Option<u64>| value.map(|value| value.to_string()).unwrap_or_default();
        let dimension = |value: Option<u32>| value.map(|value| value.to_string()).unwrap_or_default();
        [
            ("hwdec".to_string(), self.hwdec.clone()),
            ("codec".to_string(), self.codec.clone()),
            ("fps".to_string(), number(self.fps)),
            ("dropped".to_string(), integer(self.dropped)),
            ("width".to_string(), dimension(self.width)),
            ("height".to_string(), dimension(self.height)),
            ("duration".to_string(), number(self.duration)),
            ("position".to_string(), number(self.position)),
            ("active".to_string(), self.active.to_string()),
        ]
        .into_iter()
        .collect()
    }

    /// Inverse of [`PlaybackStats::to_bus_map`]. Anything unparseable reads
    /// back as absent, so a newer server cannot make an older client guess.
    pub fn from_bus_map(values: &std::collections::HashMap<String, String>) -> Self {
        let number = |key: &str| {
            values
                .get(key)
                .and_then(|value| value.trim().parse::<f64>().ok())
                .filter(|value| value.is_finite())
        };
        let unsigned = |key: &str| {
            values
                .get(key)
                .and_then(|value| value.trim().parse::<u64>().ok())
        };
        let dimension = |key: &str| unsigned(key).map(|value| value as u32);
        let hwdec = values.get("hwdec").cloned().unwrap_or_default();
        Self {
            active: values
                .get("active")
                .map(|value| value == "true")
                .unwrap_or(false),
            fps: number("fps"),
            dropped: unsigned("dropped"),
            codec: values.get("codec").cloned().unwrap_or_default(),
            width: dimension("width"),
            height: dimension("height"),
            duration: number("duration"),
            position: number("position"),
            hwdec,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_text_needs_both_dimensions() {
        let mut stats = PlaybackStats {
            width: Some(1920),
            height: Some(1080),
            ..Default::default()
        };
        assert_eq!(stats.resolution_text(), "1920 × 1080");
        stats.height = None;
        assert_eq!(stats.resolution_text(), "");
        stats.height = Some(0);
        assert_eq!(stats.resolution_text(), "");
    }

    #[test]
    fn the_codec_is_shortened_on_the_family_separator() {
        let stats = PlaybackStats {
            codec: "H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10".into(),
            ..Default::default()
        };
        assert_eq!(stats.codec_short(), "H.264");
        // A single-token codec passes through untouched.
        let vp9 = PlaybackStats {
            codec: "VP9 (codec vp9)".into(),
            ..Default::default()
        };
        assert_eq!(vp9.codec_short(), "VP9 (codec vp9)");
        assert_eq!(PlaybackStats::default().codec_short(), "");
    }

    #[test]
    fn a_software_fallback_is_named_instead_of_left_blank() {
        assert_eq!(
            PlaybackStats {
                hwdec: "vaapi".into(),
                ..Default::default()
            }
            .decoder_label(),
            "hwdec (vaapi)"
        );
        for hwdec in ["no", ""] {
            let stats = PlaybackStats {
                hwdec: hwdec.into(),
                ..Default::default()
            };
            assert_eq!(stats.decoder_label(), "hwdec (software)");
        }
    }

    #[test]
    fn drop_percentage_needs_a_decoded_baseline() {
        let stats = PlaybackStats {
            dropped: Some(3),
            fps: Some(30.0),
            position: Some(10.0),
            ..Default::default()
        };
        // 300 frames decoded, 3 dropped.
        assert_eq!(stats.drop_percent(), Some(1.0));
        // Nothing decoded yet: a percentage would be a guess.
        assert_eq!(
            PlaybackStats {
                dropped: Some(0),
                fps: Some(30.0),
                position: Some(0.0),
                ..Default::default()
            }
            .drop_percent(),
            None
        );
        assert_eq!(PlaybackStats::default().drop_percent(), None);
    }

    #[test]
    fn the_bus_map_round_trips_a_full_reading() {
        let stats = PlaybackStats {
            hwdec: "vaapi".into(),
            codec: "H.264 / AVC".into(),
            fps: Some(59.94),
            dropped: Some(12),
            width: Some(3840),
            height: Some(2160),
            duration: Some(35.366),
            position: Some(1.667),
            active: true,
        };
        let parsed = PlaybackStats::from_bus_map(&stats.to_bus_map());
        assert_eq!(parsed, stats);
    }

    #[test]
    fn unknown_values_stay_unknown_through_the_bus() {
        let parsed = PlaybackStats::from_bus_map(&PlaybackStats::default().to_bus_map());
        assert!(!parsed.active);
        assert_eq!(parsed.fps, None);
        assert_eq!(parsed.width, None);
        assert_eq!(parsed.dropped, None);
        assert_eq!(parsed.hwdec, "");
        // Garbage from a newer server is absent, not zero.
        let mut hostile = std::collections::HashMap::new();
        hostile.insert("fps".to_string(), "NaN".to_string());
        hostile.insert("width".to_string(), "-4".to_string());
        let parsed = PlaybackStats::from_bus_map(&hostile);
        assert_eq!(parsed.fps, None);
        assert_eq!(parsed.width, None);
    }
}
