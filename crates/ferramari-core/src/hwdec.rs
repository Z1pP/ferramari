//! Hardware-decoding status, independent of the player.
//!
//! The user picks `auto`, `vaapi` or `off` in settings. What actually decodes
//! is a fact about the machine, and the setting alone cannot tell the two
//! apart: mpv accepts `--hwdec=vaapi` on a GPU with no VA-API path and plays
//! anyway, decoding on the CPU. Nothing said so until the player was asked
//! what it is really doing.
//!
//! This module turns the two strings — the requested mode and the decoder in
//! use — into a verdict the UI and `--doctor` can show. It lives in core
//! because both the server (which reads the player) and the control surface
//! (which renders the settings page) need the same conclusion. Sentences are
//! returned untranslated: the UI is the layer that owns gettext, so it calls
//! [`crate::util::tr`] on the source string, and the log and `--doctor` can
//! print the same text without depending on a locale.

/// The user-facing hardware-decoding setting. Anything unrecognised behaves
/// as `auto`, matching [`crate::config`], which repairs unknown values.
pub const HWDEC_AUTO: &str = "auto";
pub const HWDEC_VAAPI: &str = "vaapi";
pub const HWDEC_NONE: &str = "none";

/// The mpv value for a user-facing mode. `auto-safe` is the default because
/// it only enables decoders mpv knows to be safe on the current platform;
/// `vaapi` is an explicit override for hardware testing.
pub fn hwdec_arg(mode: &str) -> &'static str {
    match mode {
        HWDEC_VAAPI => "vaapi",
        HWDEC_NONE => "no",
        _ => "auto-safe",
    }
}

/// Which decoder is actually running, as reported by the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HwDec {
    /// Decoding on the GPU (`vaapi`, `vulkan`, `cuda-copy`, …).
    Hardware(String),
    /// Decoding on the CPU, whether that was requested or a fallback.
    Software,
    /// Nothing is decoding yet, so the answer does not exist. mpv reports the
    /// current decoder as *unavailable* until a file has been opened, so this
    /// is the normal reading at startup and while a decoder is parked.
    Unknown,
}

impl HwDec {
    /// Interpret a reported decoder name. Any value other than `no` names the
    /// hardware API that won; an absent or empty value means nothing decoded.
    pub fn from_report(value: Option<&str>) -> Self {
        match value {
            None | Some("") => HwDec::Unknown,
            Some("no") => HwDec::Software,
            Some(api) => HwDec::Hardware(api.to_string()),
        }
    }

    /// Whether the reading is the hardware API `want` refers to. The same
    /// interface can be reported as `vaapi` or `vaapi-copy` depending on the
    /// interop path the player picked, so both count.
    pub fn is(&self, want: &str) -> bool {
        match self {
            HwDec::Hardware(api) => api == want || api.starts_with(&format!("{want}-")),
            _ => false,
        }
    }

    /// The decoder name for diagnostics, or an empty string when there is
    /// nothing to show.
    pub fn label(&self) -> &'static str {
        match self {
            HwDec::Hardware(_) | HwDec::Unknown => "",
            HwDec::Software => "software",
        }
    }

    /// The API name when decoding on the GPU.
    pub fn api(&self) -> Option<&str> {
        match self {
            HwDec::Hardware(api) => Some(api),
            _ => None,
        }
    }

    /// The report this reading came from, so it can travel through a
    /// string-typed bus property and be reconstructed exactly.
    pub fn report(&self) -> String {
        match self {
            HwDec::Hardware(api) => api.clone(),
            HwDec::Software => "no".to_string(),
            HwDec::Unknown => String::new(),
        }
    }
}

/// What to tell the user about the requested mode versus the running decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HwDecCheck {
    /// Nothing to report: the player has not decoded a file yet, or the
    /// setting is honoured exactly.
    Ok,
    /// `auto` ended up on the CPU. Legitimate on a machine with no safe
    /// hardware path, so it is information rather than an error.
    AutoIsSoftware,
    /// An explicit API was requested and the player quietly decoded on the
    /// CPU instead. This is the silent fallback the setting used to hide.
    FellBack { want: String },
    /// An explicit API was requested and a *different* one is active.
    Replaced { want: String, got: String },
    /// Hardware decoding is off, and yet something decoded on the GPU.
    HardwareAgainstRequest { got: String },
}

impl HwDecCheck {
    /// Compare the user's mode with the decoder in use.
    pub fn evaluate(requested: &str, current: &HwDec) -> Self {
        match current {
            HwDec::Unknown => HwDecCheck::Ok,
            HwDec::Software => match requested {
                HWDEC_NONE => HwDecCheck::Ok,
                HWDEC_VAAPI => HwDecCheck::FellBack {
                    want: HWDEC_VAAPI.to_string(),
                },
                _ => HwDecCheck::AutoIsSoftware,
            },
            HwDec::Hardware(api) => match requested {
                HWDEC_NONE => HwDecCheck::HardwareAgainstRequest { got: api.clone() },
                HWDEC_VAAPI if current.is(HWDEC_VAAPI) => HwDecCheck::Ok,
                HWDEC_VAAPI => HwDecCheck::Replaced {
                    want: HWDEC_VAAPI.to_string(),
                    got: api.clone(),
                },
                // `auto`/`auto-safe` may pick any API; that is the point of it.
                _ => HwDecCheck::Ok,
            },
        }
    }

    /// Whether the verdict deserves a line in the UI at all.
    pub fn is_silent(&self) -> bool {
        matches!(self, HwDecCheck::Ok)
    }

    /// The source sentence for the settings page, untranslated on purpose.
    /// The control surface calls `tr` on it; logs and `--doctor` print it as
    /// is, so a report reads the same in every locale.
    ///
    /// No placeholders either: catalogs hold whole sentences, and the decoder
    /// name is shown next to the sentence by [`Self::decoder`] rather than
    /// interpolated into a translated string.
    pub fn message(&self) -> &'static str {
        match self {
            HwDecCheck::Ok => "",
            // A fact, not a fault: `auto` may legitimately find nothing safe.
            HwDecCheck::AutoIsSoftware => {
                "Hardware decoding is not available here; decoding on the CPU."
            }
            // The case the setting used to hide: an explicit request ignored.
            HwDecCheck::FellBack { .. } => {
                "The selected decoder is not available here; decoding on the CPU instead."
            }
            HwDecCheck::Replaced { .. } => {
                "The selected decoder is not available; a different hardware decoder is active."
            }
            HwDecCheck::HardwareAgainstRequest { .. } => {
                "Hardware decoding is active although software decoding was selected."
            }
        }
    }

    /// The decoder name worth showing next to [`Self::message`], when the
    /// reading carries one. `None` for software decoding, which the sentence
    /// already states.
    pub fn decoder(&self) -> Option<&str> {
        match self {
            HwDecCheck::Replaced { got, .. } | HwDecCheck::HardwareAgainstRequest { got } => {
                Some(got.as_str())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpv_values_match_the_user_facing_modes() {
        assert_eq!(hwdec_arg("auto"), "auto-safe");
        assert_eq!(hwdec_arg("vaapi"), "vaapi");
        assert_eq!(hwdec_arg("none"), "no");
        // An unknown setting is treated as auto, as config.rs repairs it too.
        assert_eq!(hwdec_arg("nonsense"), "auto-safe");
    }

    #[test]
    fn a_missing_report_means_nothing_decoded_yet() {
        assert_eq!(HwDec::from_report(None), HwDec::Unknown);
        assert_eq!(HwDec::from_report(Some("")), HwDec::Unknown);
        // Unknown never contradicts a request, so the UI stays silent.
        for requested in ["auto", "vaapi", "none"] {
            assert_eq!(
                HwDecCheck::evaluate(requested, &HwDec::Unknown),
                HwDecCheck::Ok
            );
        }
    }

    #[test]
    fn cpu_decoding_is_only_a_fallback_when_hardware_was_demanded() {
        assert_eq!(
            HwDecCheck::evaluate("none", &HwDec::Software),
            HwDecCheck::Ok,
            "software decoding is exactly what 'Off' asked for"
        );
        assert_eq!(
            HwDecCheck::evaluate("auto", &HwDec::Software),
            HwDecCheck::AutoIsSoftware,
            "auto may legitimately land on the CPU"
        );
        assert_eq!(
            HwDecCheck::evaluate("vaapi", &HwDec::Software),
            HwDecCheck::FellBack {
                want: "vaapi".to_string()
            },
            "the silent fallback must become visible"
        );
    }

    #[test]
    fn auto_accepts_whichever_api_wins() {
        for api in ["vulkan", "vaapi", "cuda-copy", "d3d11va"] {
            assert_eq!(
                HwDecCheck::evaluate("auto", &HwDec::Hardware(api.into())),
                HwDecCheck::Ok
            );
        }
        assert!(!HwDecCheck::evaluate("auto", &HwDec::Software).is_silent());
    }

    #[test]
    fn vaapi_reports_both_interop_shapes_as_honoured() {
        assert_eq!(
            HwDecCheck::evaluate("vaapi", &HwDec::Hardware("vaapi".into())),
            HwDecCheck::Ok
        );
        assert_eq!(
            HwDecCheck::evaluate("vaapi", &HwDec::Hardware("vaapi-copy".into())),
            HwDecCheck::Ok
        );
    }

    #[test]
    fn a_different_api_is_reported_as_a_replacement() {
        let check = HwDecCheck::evaluate("vaapi", &HwDec::Hardware("vulkan".into()));
        assert_eq!(
            check,
            HwDecCheck::Replaced {
                want: "vaapi".to_string(),
                got: "vulkan".to_string()
            }
        );
        // The sentence stays free of placeholders so catalogs hold whole
        // strings; the API that replaced the requested one is exposed next to
        // it, never interpolated.
        assert!(check.message().contains("different hardware decoder"));
        assert_eq!(check.decoder(), Some("vulkan"));
    }

    #[test]
    fn hardware_wins_against_an_off_request() {
        let check = HwDecCheck::evaluate("none", &HwDec::Hardware("vulkan".into()));
        assert_eq!(
            check,
            HwDecCheck::HardwareAgainstRequest {
                got: "vulkan".to_string()
            }
        );
        assert!(check.message().contains("software decoding was selected"));
        assert_eq!(check.decoder(), Some("vulkan"));
    }

    #[test]
    fn every_reported_verdict_says_something_specific() {
        // A generic "something is wrong" line is not actionable: each verdict
        // has to name the CPU or the other hardware decoder.
        let cases = [
            ("auto", HwDec::Software, true),
            ("vaapi", HwDec::Software, true),
            ("vaapi", HwDec::Hardware("vulkan".into()), false),
            ("none", HwDec::Hardware("vulkan".into()), false),
        ];
        for (requested, current, mentions_cpu) in cases {
            let check = HwDecCheck::evaluate(requested, &current);
            assert!(
                !check.is_silent(),
                "{requested} + {current:?} must be reported"
            );
            let text = check.message();
            assert!(!text.is_empty(), "{requested} + {current:?} has no message");
            if mentions_cpu {
                assert!(text.contains("CPU"), "{requested} + {current:?}: {text}");
                assert_eq!(
                    check.decoder(),
                    None,
                    "software decoding has no API to name"
                );
            } else {
                assert!(
                    check.decoder().is_some(),
                    "{requested} + {current:?}: {text}"
                );
            }
        }
    }

    #[test]
    fn a_report_round_trips_through_the_bus() {
        // The D-Bus property carries a string, so a reading must survive
        // being flattened and parsed again without changing its verdict.
        for reading in [
            HwDec::Software,
            HwDec::Hardware("vaapi".into()),
            HwDec::Hardware("vaapi-copy".into()),
            HwDec::Unknown,
        ] {
            let report = reading.report();
            let parsed = HwDec::from_report((!report.is_empty()).then_some(report.as_str()));
            assert_eq!(parsed, reading, "{reading:?}");
        }
    }

    #[test]
    fn honoured_settings_say_nothing() {
        assert!(HwDecCheck::Ok.is_silent());
        assert!(HwDecCheck::evaluate("none", &HwDec::Software).is_silent());
        assert!(HwDecCheck::evaluate("vaapi", &HwDec::Hardware("vaapi".into())).is_silent());
        assert!(HwDecCheck::evaluate("auto", &HwDec::Hardware("vulkan".into())).is_silent());
        assert_eq!(HwDecCheck::Ok.message(), "");
    }

    #[test]
    fn the_api_accessor_only_yields_a_real_api() {
        assert_eq!(HwDec::Hardware("vulkan".into()).api(), Some("vulkan"));
        assert_eq!(HwDec::Software.api(), None);
        assert_eq!(HwDec::Unknown.api(), None);
        assert_eq!(HwDec::Software.label(), "software");
        assert_eq!(HwDec::Unknown.label(), "");
    }

    #[test]
    fn a_hardware_reading_never_matches_software_wants() {
        assert!(!HwDec::Software.is("vaapi"));
        assert!(!HwDec::Unknown.is("vaapi"));
        assert!(HwDec::Hardware("vaapi".into()).is("vaapi"));
    }
}
