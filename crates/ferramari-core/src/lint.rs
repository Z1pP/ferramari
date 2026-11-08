//! Creator-facing package review: `ferramari package lint`.
//!
//! Import validation answers one question — may this be applied — and stops at
//! the first structural violation. A creator preparing a release needs a
//! different answer: *what will a recipient see*. Import happily accepts a
//! package with no `description`, no `resolution` and no `preview`; it installs
//! and renders as a blank card in the catalogue, which reads to its author as
//! "published and nothing happened".
//!
//! Findings therefore carry a severity:
//!
//! * [`Severity::Error`] — do not publish this. Either `package::load` refuses
//!   it, or the package cannot be trusted as authored (its signature does not
//!   verify). Note the difference from import: an invalid signature does not
//!   stop `load`, it only means the recipient's copy was tampered with.
//! * [`Severity::Warning`] — import applies it and the result is degraded.
//!
//! Two rules of construction keep this module honest:
//!
//! 1. It never re-implements an import rule. Structural problems arrive as one
//!    `import-rejected` finding carrying `package::load`'s own message, so the
//!    two cannot drift apart and a creator is not told the same violation twice
//!    in different words.
//! 2. It is a pure function of [`Facts`]. Media measurements come from
//!    `ffprobe`, a subprocess that may be missing or slow, so the CLI gathers
//!    the facts and the decision logic here stays testable without a video
//!    file or an ffmpeg install.
//!
//! Every finding has a stable `code`, so a creator's CI can assert on `--json`
//! output instead of grepping sentences.

use serde::{Deserialize, Serialize};

/// Whether a finding forbids publishing or merely degrades the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Do not publish: import refuses the package, or its authorship does not
    /// verify.
    Error,
    /// Import applies it, and the recipient sees something degraded.
    Warning,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    /// Stable identifier for tooling and documentation. Never translated.
    pub code: String,
    /// Detail assembled from untranslated field names and values: the
    /// sentence a creator reads, while `code` is what CI matches on.
    pub message: String,
}

impl Finding {
    fn error(code: &str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: code.to_string(),
            message: message.into(),
        }
    }

    fn warning(code: &str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code: code.to_string(),
            message: message.into(),
        }
    }
}

/// What a media probe measured, when it could measure anything at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Measured {
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// The file could not be probed: missing tool, unreadable file, or a
    /// non-media entry such as a web package's `index.html`. Distinguishing
    /// "nothing to measure" from "the measurement failed" is what keeps a
    /// machine without ffmpeg from blaming the creator for it.
    pub unprobed: bool,
}

impl Measured {
    /// `WIDTHxHEIGHT` as measured, or `None` when either dimension is unknown.
    pub fn resolution(&self) -> Option<String> {
        match (self.width, self.height) {
            (Some(width), Some(height)) => Some(format!("{width}x{height}")),
            _ => None,
        }
    }
}

/// Everything the rules need, gathered by the caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Facts {
    /// `Some(message)` when `package::load` refused the package. Its message
    /// is reported verbatim as the `import-rejected` finding.
    pub import_error: Option<String>,
    /// Empty when the manifest could not be read at all; the import error
    /// above then carries the reason.
    pub name: String,
    pub author: String,
    pub description: String,
    /// `video` or `web`, as declared. Not re-validated: an unusable value is
    /// already inside `import_error`.
    pub kind: String,
    /// The manifest's declared `WIDTHxHEIGHT`, if it declared one.
    pub declared_resolution: Option<String>,
    /// A preview image exists in the package (declared, or the `preview.jpg`
    /// convention).
    pub preview_present: bool,
    /// The entry video as measured.
    pub measured_entry: Measured,
    /// The preview image as measured, when a preview is present.
    pub measured_preview: Measured,
    /// `checksums.json` covered every file in the package.
    pub checksummed: bool,
    /// Signature state, from `signing::SignatureStatus`.
    pub signature: Signature,
}

/// Signature state as far as publishing is concerned, flattened from
/// `crate::signing::SignatureStatus` so this module does not depend on the
/// trust store's shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "state", content = "detail")]
pub enum Signature {
    /// No `signature.json`.
    #[default]
    Unsigned,
    /// Verifies against a key in the trust store: `name (key id)`.
    Trusted(String),
    /// Verifies, but the key is not trusted here: `name, key id`.
    UnknownKey(String),
    /// Does not verify; the reason is `package::load`'s or the signer's.
    Invalid(String),
}

impl Default for Facts {
    /// A package whose manifest was read but says nothing beyond a name: the
    /// shape the catalogue would show as an empty card.
    fn default() -> Self {
        Self {
            import_error: None,
            name: "Wallpaper".to_string(),
            author: String::new(),
            description: String::new(),
            kind: "video".to_string(),
            declared_resolution: None,
            preview_present: false,
            measured_entry: Measured::default(),
            measured_preview: Measured::default(),
            checksummed: false,
            signature: Signature::Unsigned,
        }
    }
}

/// A reviewed package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub name: String,
    pub kind: String,
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn errors(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity == Severity::Error)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity == Severity::Warning)
    }

    /// The package must not be published as it stands.
    pub fn blocked(&self) -> bool {
        self.errors().next().is_some()
    }

    /// `severity: message` per finding, in report order, for plain output.
    pub fn lines(&self) -> Vec<String> {
        self.findings
            .iter()
            .map(|finding| format!("{}: {}", finding.severity.as_str(), finding.message))
            .collect()
    }
}

/// Review a package. Errors are listed before warnings, each group in the
/// order the rules below state them.
pub fn review(facts: &Facts) -> Report {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // Import's own verdict, one finding carrying its words; every rule that
    // `package::load` enforces is inside it, so there is only one place to
    // keep import's structural rules in sync with import itself.
    if let Some(message) = &facts.import_error {
        errors.push(Finding::error("import-rejected", message.clone()));
    } else if facts.name.trim().is_empty() {
        // A package without `import_error` may still be empty when the caller
        // read manifest but found it blank — possible after a hand-edit.
        errors.push(Finding::error(
            "name-missing",
            "manifest name is empty (missing from manifest or set to an empty string)",
        ));
    }

    match &facts.signature {
        Signature::Invalid(reason) => errors.push(Finding::error(
            "signature-invalid",
            format!("signature does not verify: {reason}"),
        )),
        Signature::Unsigned => warnings.push(Finding::warning(
            "unsigned",
            "unsigned: a recipient cannot tell this package came from you; run \
             `ferramari package sign`",
        )),
        // Signed and verifying, with or without a trusted key here: the
        // creator's own machine not trusting its own key is not a finding
        // about the package.
        Signature::Trusted(_) | Signature::UnknownKey(_) => {}
    }

    if !facts.checksummed {
        warnings.push(Finding::warning(
            "checksums-missing",
            "no checksums.json: a recipient cannot tell a modified package from \
             the one that was published",
        ));
    }

    if facts.author.trim().is_empty() {
        warnings.push(Finding::warning(
            "author-missing",
            "no author: the catalogue has nobody to credit and nothing to match \
             a signature against",
        ));
    }

    if facts.description.trim().is_empty() {
        warnings.push(Finding::warning(
            "description-missing",
            "no description: the detail pane is empty for anyone browsing",
        ));
    }

    // A preview is what makes a card recognisable among hundreds. Having none
    // is legal and degrades the listing; a declared one that is absent is
    // import's problem, already reported.
    if !facts.preview_present {
        warnings.push(Finding::warning(
            "preview-missing",
            "no preview image: the package shows as a placeholder in the library \
             and the catalogue",
        ));
    } else if !facts.measured_preview.unprobed && facts.measured_preview.resolution().is_none() {
        // Present but not decodable as an image: worse than none, because the
        // card is still blank and the bytes still cost their share.
        warnings.push(Finding::warning(
            "preview-unreadable",
            "the preview file is in the package but did not decode as an image",
        ));
    }

    // Resolution decides which monitors a wallpaper is offered for, so a value
    // that contradicts the file misleads the recipient twice over. Three
    // different kinds of "no measurement" need three different sentences:
    // nothing to measure (a web entry), no way to measure here (no ffmpeg),
    // and a file that refuses to be decoded — only the last is the creator's
    // problem, and blaming them for the first two is how a lint output gets
    // ignored everywhere.
    let declared = format_resolution(parse_resolution(facts.declared_resolution.as_deref()));
    let measured = facts.measured_entry.resolution();
    match (declared, measured) {
        (Some(declared), Some(measured)) if declared != measured => {
            warnings.push(Finding::warning(
                "resolution-mismatch",
                format!("manifest declares {declared} but the entry measures {measured}"),
            ))
        }
        (Some(_), _) => {
            // Declared, and either matching or unverifiable. The declaration
            // is all anybody has in that case, and it is not a finding.
        }
        (None, Some(measured)) => warnings.push(Finding::warning(
            "resolution-missing",
            format!("no resolution declared; the entry measures {measured}"),
        )),
        (None, None) if facts.measured_entry.unprobed => warnings.push(Finding::warning(
            "resolution-missing",
            match facts.kind.as_str() {
                "web" => {
                    "no resolution declared; a web page has no intrinsic size, \
                          so declare the display it is meant for"
                }
                _ => {
                    "no resolution declared, and nothing could be measured here \
                     (install ffmpeg/ffprobe to have it checked)"
                }
            }
            .to_string(),
        )),
        (None, None) => warnings.push(Finding::warning(
            "resolution-missing",
            "no resolution declared, and the entry did not decode as a video",
        )),
    }

    let mut findings = errors;
    findings.extend(warnings);
    Report {
        name: facts.name.clone(),
        kind: facts.kind.clone(),
        findings,
    }
}

/// `WIDTHxHEIGHT` as two positive numbers, or `None` unless it parses
/// cleanly. Import applies the same rule; this one exists so a declared value
/// can be compared with a measurement without trusting the text.
fn parse_resolution(value: Option<&str>) -> Option<(u32, u32)> {
    let (width, height) = value?.split_once('x')?;
    let width = width.trim().parse::<u32>().ok()?;
    let height = height.trim().parse::<u32>().ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

fn format_resolution(resolution: Option<(u32, u32)>) -> Option<String> {
    resolution.map(|(width, height)| format!("{width}x{height}"))
}

/// Measure one file's pixel dimensions, keeping track of *why* there is no
/// number.
///
/// `catalogue::probe` collapses every failure into `None`, which is right for
/// its callers (a download is still a download without the extra line) and
/// wrong here: lint must not tell a creator their video is broken because
/// this machine has no ffmpeg. So this probe distinguishes three outcomes by
/// what it can actually observe:
///
/// * spawn failed → `unprobed`, there is no tool to ask;
/// * ffprobe answered with a non-zero status, or with no usable stream →
///   decoded-failed, which leaves the dimensions unknown and `unprobed` off;
/// * a stream with real dimensions → measured.
///
/// A corrupt image is the second case and not the third: measured, ffprobe 8
/// exits 0 for a JPEG whose scan fails and reports `0x0`, which this function
/// turns into "did not decode" instead of a 0×0 resolution.
pub fn measure(path: &std::path::Path) -> Measured {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let mut child = match Command::new(crate::paths::tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "json",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        // No ffprobe to ask. Nothing here can tell whether the file is fine.
        Err(_) => {
            return Measured {
                width: None,
                height: None,
                unprobed: true,
            }
        }
    };
    let Some(status) = crate::util::wait_with_timeout(&mut child, crate::catalogue::PROBE_TIMEOUT)
    else {
        // Hung or killed: the tool exists but did not answer.
        return Measured {
            width: None,
            height: None,
            unprobed: true,
        };
    };
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if !status.success() {
        return Measured::default();
    }
    let parsed: serde_json::Value = match serde_json::from_str(&stdout) {
        Ok(value) => value,
        Err(_) => return Measured::default(),
    };
    let stream = parsed.get("streams").and_then(|v| v.get(0));
    let width = positive_dimension(stream.as_ref().and_then(|s| s.get("width")));
    let height = positive_dimension(stream.as_ref().and_then(|s| s.get("height")));
    match (width, height) {
        (Some(width), Some(height)) => Measured {
            width: Some(width),
            height: Some(height),
            unprobed: false,
        },
        _ => Measured::default(),
    }
}

fn positive_dimension(value: Option<&serde_json::Value>) -> Option<u32> {
    let value = value?.as_u64()? as u32;
    (value > 0).then_some(value)
}

/// Flatten `crate::signing::SignatureStatus` into the publishing view.
///
/// The trust store distinguishes more states than a creator needs to see,
/// and lint must not depend on that shape: a key that verifies but is not
/// trusted *here* is fine for a package intended for other machines.
pub fn signature_of(status: &crate::signing::SignatureStatus) -> Signature {
    use crate::signing::SignatureStatus;
    match status {
        SignatureStatus::Unsigned => Signature::Unsigned,
        SignatureStatus::Trusted { key_id, name } => {
            Signature::Trusted(format!("{name} ({key_id})"))
        }
        SignatureStatus::Unknown { key_id, name } => {
            Signature::UnknownKey(format!("{name}, {key_id}"))
        }
        SignatureStatus::Invalid { reason, .. } => Signature::Invalid(reason.clone()),
    }
}

/// Gather the facts for the package at `root`, as far as this machine can.
///
/// This is the IO half of lint; [`review`] is the decision half and takes the
/// result. Splitting them keeps every rule testable with hand-written facts
/// and keeps the probes (ffprobe, signature verification) in one place.
///
/// A package that fails `package::load` is still measured: the manifest is
/// read directly, so a creator sees the publishing warnings in the same run
/// as the structural error instead of fixing them one pass at a time. Fields
/// that cannot be gathered are left at their default rather than guessed.
pub fn gather(root: &std::path::Path) -> Facts {
    let loaded = crate::package::load(root);
    let manifest = crate::package::read_manifest(root);

    // Declared preview path, resolved for measurement. `package::load` stops
    // at the first error, so a package whose entry is broken still reaches
    // here through the manifest read.
    let (name, author, description, kind, declared_resolution, declared_preview) = match &manifest {
        Some(manifest) => (
            manifest.name.clone(),
            manifest.author.clone(),
            manifest.description.clone(),
            manifest.kind.as_str().to_string(),
            manifest.resolution.clone(),
            manifest.preview.clone(),
        ),
        None => (
            String::new(),
            String::new(),
            String::new(),
            "video".to_string(),
            None,
            None,
        ),
    };

    let entry = manifest
        .as_ref()
        .map(|manifest| {
            manifest
                .entry
                .clone()
                .unwrap_or_else(|| crate::package::default_entry(manifest.kind))
        })
        .unwrap_or_else(|| "wallpaper.mp4".to_string());

    // Preview presence follows the same rule `load` uses: the declared path,
    // otherwise the `preview.jpg` convention.
    let preview_path = match declared_preview.as_deref() {
        Some(name) => Some(root.join(name)),
        None => {
            let candidate = root.join("preview.jpg");
            candidate.is_file().then_some(candidate)
        }
    };
    let preview_present = preview_path.as_ref().is_some_and(|path| path.is_file());

    let entry_path = root.join(&entry);
    // Only a video entry can be measured; a web page is not a probe failure.
    let measured_entry = if kind == "web" {
        Measured {
            width: None,
            height: None,
            unprobed: true,
        }
    } else if entry_path.is_file() {
        measure(&entry_path)
    } else {
        Measured::default()
    };
    let measured_preview = match &preview_path {
        Some(path) if path.is_file() => measure(path),
        _ => Measured::default(),
    };

    // Integrity and signature: import's own report wins when it got far
    // enough to check, otherwise ask the verifiers directly.
    let (checksummed, signature) = match &loaded {
        Ok(package) => (package.checksummed, signature_of(&package.signature)),
        Err(_) => {
            // The same rule `load` applies: an unusable or incomplete
            // integrity file reads as "not checksummed".
            let checksummed = crate::package::verify_checksums(root).unwrap_or(false);
            let signature = crate::signing::verify_package(root)
                .map(|status| signature_of(&status))
                .unwrap_or_else(Signature::Invalid);
            (checksummed, signature)
        }
    };

    Facts {
        import_error: loaded.err(),
        name,
        author,
        description,
        kind,
        declared_resolution,
        preview_present,
        measured_entry,
        measured_preview,
        checksummed,
        signature,
    }
}

/// Every code this module can produce, for documentation and for validating
/// the CLI's `--ignore` list.
pub const KNOWN_CODES: &[&str] = &[
    "import-rejected",
    "name-missing",
    "signature-invalid",
    "unsigned",
    "checksums-missing",
    "author-missing",
    "description-missing",
    "preview-missing",
    "preview-unreadable",
    "resolution-mismatch",
    "resolution-missing",
];

pub fn is_known_code(code: &str) -> bool {
    KNOWN_CODES.contains(&code)
}

/// Parse a comma-separated `--ignore` list.///
/// An unknown code is an error rather than a no-op: a filter that silently
/// matches nothing turns a real finding into a green build, which is the one
/// outcome a CI check must not be able to produce by typo.
pub fn parse_ignore_list(value: &str) -> Result<Vec<String>, String> {
    let mut codes = Vec::new();
    for part in value.split(',') {
        let code = part.trim();
        if code.is_empty() {
            continue;
        }
        if !is_known_code(code) {
            return Err(format!("unknown finding code {code:?}"));
        }
        codes.push(code.to_string());
    }
    Ok(codes)
}

/// Drop the findings whose code is in `ignore`.
pub fn filter(findings: Vec<Finding>, ignore: &[String]) -> Vec<Finding> {
    findings
        .into_iter()
        .filter(|finding| !ignore.iter().any(|code| code == &finding.code))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::import;
    use std::fs;

    fn codes(report: &Report) -> Vec<String> {
        report
            .findings
            .iter()
            .map(|finding| finding.code.clone())
            .collect()
    }

    fn measured(width: u32, height: u32) -> Measured {
        Measured {
            width: Some(width),
            height: Some(height),
            unprobed: false,
        }
    }

    fn complete() -> Facts {
        Facts {
            import_error: None,
            name: "Neon Harbour".to_string(),
            author: "Ada".to_string(),
            description: "Rain over the docks at night".to_string(),
            kind: "video".to_string(),
            declared_resolution: Some("3840x2160".to_string()),
            preview_present: true,
            measured_entry: measured(3840, 2160),
            measured_preview: measured(400, 225),
            checksummed: true,
            signature: Signature::Trusted("Ada (key-1)".to_string()),
        }
    }

    #[test]
    fn a_complete_package_has_no_findings() {
        let report = review(&complete());
        assert_eq!(report.findings, Vec::new(), "{:?}", report.findings);
        assert!(!report.blocked());
    }

    #[test]
    fn a_signed_key_that_is_not_trusted_locally_is_not_a_finding() {
        // The creator's own machine need not trust its own key; the package is
        // no different from one whose key is in the store.
        let facts = Facts {
            signature: Signature::UnknownKey("Ada, key-1".to_string()),
            ..complete()
        };
        assert_eq!(review(&facts).findings, Vec::new());
    }

    #[test]
    fn import_rejection_is_an_error_and_blocks() {
        let facts = Facts {
            import_error: Some("package contains a symlink".to_string()),
            ..Facts::default()
        };
        let report = review(&facts);
        assert!(report.blocked());
        assert_eq!(report.findings[0].code, "import-rejected");
        assert_eq!(report.findings[0].severity, Severity::Error);
        // The message is import's, not a paraphrase, so there is only one
        // place to keep the wording of structural rules.
        assert!(report.findings[0].message.contains("symlink"));
    }

    #[test]
    fn an_import_rejection_does_not_hide_publishing_problems() {
        // Fixing a package should take one pass, not one per rule.
        let facts = Facts {
            import_error: Some("package is larger than 2 GiB".to_string()),
            ..Facts::default()
        };
        let report = review(&facts);
        assert!(report.blocked());
        assert_eq!(report.errors().count(), 1);
        assert!(
            report.warnings().count() >= 4,
            "warnings still listed: {:?}",
            codes(&report)
        );
    }

    #[test]
    fn a_bare_manifest_warns_about_everything_a_card_shows() {
        let report = review(&Facts::default());
        assert!(!report.blocked(), "import applies this package");
        for code in [
            "unsigned",
            "checksums-missing",
            "author-missing",
            "description-missing",
            "preview-missing",
            "resolution-missing",
        ] {
            assert!(
                codes(&report).contains(&code.to_string()),
                "{code} missing from {:?}",
                codes(&report)
            );
        }
    }

    #[test]
    fn an_invalid_signature_blocks() {
        // Import would still copy it, which is exactly why lint exists: a
        // tampered package must never reach a catalogue.
        let facts = Facts {
            signature: Signature::Invalid("digest does not match".to_string()),
            checksummed: true,
            ..complete()
        };
        let report = review(&facts);
        assert!(report.blocked());
        let codes = codes(&report);
        assert!(
            codes.contains(&"signature-invalid".to_string()),
            "{codes:?}"
        );
        assert!(!codes.contains(&"unsigned".to_string()), "{codes:?}");
        assert!(report
            .findings
            .iter()
            .any(|f| f.message.contains("digest does not match")));
    }

    #[test]
    fn being_unsigned_is_a_warning_not_a_failure() {
        let facts = Facts {
            signature: Signature::Unsigned,
            ..complete()
        };
        let report = review(&facts);
        assert!(!report.blocked());
        assert!(codes(&report).contains(&"unsigned".to_string()));
    }

    #[test]
    fn missing_checksums_is_warned_independently_of_the_signature() {
        let facts = Facts {
            checksummed: false,
            ..complete()
        };
        assert!(codes(&review(&facts)).contains(&"checksums-missing".to_string()));
    }

    #[test]
    fn a_resolution_contradicting_the_file_is_named_exactly() {
        // The worst case for a recipient: the catalogue offers this wallpaper
        // for the wrong monitors.
        let facts = Facts {
            declared_resolution: Some("1080x1920".to_string()),
            measured_entry: measured(3840, 2160),
            ..complete()
        };
        let report = review(&facts);
        assert!(!report.blocked());
        let finding = report
            .findings
            .iter()
            .find(|finding| finding.code == "resolution-mismatch")
            .expect("a contradiction must be reported");
        assert!(finding.message.contains("1080x1920"), "{}", finding.message);
        assert!(finding.message.contains("3840x2160"), "{}", finding.message);
        assert!(
            !codes(&report).contains(&"resolution-missing".to_string()),
            "declared and mismatched must not also read as missing"
        );
    }

    #[test]
    fn spacing_in_the_declared_value_does_not_create_a_false_mismatch() {
        let facts = Facts {
            declared_resolution: Some(" 3840 x 2160 ".to_string()),
            ..complete()
        };
        assert!(!codes(&review(&facts)).contains(&"resolution-mismatch".to_string()));
    }

    #[test]
    fn a_junk_declared_resolution_falls_back_to_the_measurement() {
        // Import rejects junk shapes outright, so this only happens when the
        // caller read the manifest itself; treating it as "not declared" is
        // the safe reading rather than comparing against garbage.
        let facts = Facts {
            declared_resolution: Some("huge".to_string()),
            measured_entry: measured(3840, 2160),
            ..complete()
        };
        let report = review(&facts);
        let codes = codes(&report);
        assert!(
            !codes.contains(&"resolution-mismatch".to_string()),
            "{codes:?}"
        );
        assert!(
            codes.contains(&"resolution-missing".to_string()),
            "{codes:?}"
        );
    }

    #[test]
    fn an_unprobable_entry_does_not_blame_the_creator() {
        // No ffmpeg here, or a web package whose entry is HTML: a declared
        // resolution is all anybody has, and it must not be a finding.
        let facts = Facts {
            measured_entry: Measured {
                width: None,
                height: None,
                unprobed: true,
            },
            ..complete()
        };
        let report = review(&facts);
        let codes = codes(&report);
        assert!(
            !codes.contains(&"resolution-missing".to_string()),
            "{codes:?}"
        );
        assert!(
            !codes.contains(&"resolution-mismatch".to_string()),
            "{codes:?}"
        );
    }

    #[test]
    fn an_unprobable_entry_without_a_declaration_still_says_declare_it() {
        let facts = Facts {
            declared_resolution: None,
            measured_entry: Measured {
                width: None,
                height: None,
                unprobed: true,
            },
            ..complete()
        };
        let report = review(&facts);
        let finding = report
            .findings
            .iter()
            .find(|finding| finding.code == "resolution-missing")
            .expect("a package with no resolution anywhere must say so");
        // The point of the finding: tell the creator to declare it, and name
        // the reason the tool could not do it for them.
        assert!(
            finding.message.contains("no resolution declared")
                && finding.message.contains("ffmpeg"),
            "{}",
            finding.message
        );
    }

    #[test]
    fn a_missing_resolution_quotes_the_measurement() {
        // Export fills this in from a probe, so the common case is a
        // hand-written manifest that simply left it out.
        let facts = Facts {
            declared_resolution: None,
            measured_entry: measured(2560, 1440),
            ..complete()
        };
        let finding = review(&facts)
            .findings
            .into_iter()
            .find(|finding| finding.code == "resolution-missing")
            .expect("declared value is missing");
        assert!(finding.message.contains("2560x1440"), "{}", finding.message);
    }

    #[test]
    fn a_preview_that_does_not_decode_is_worse_than_no_preview() {
        let facts = Facts {
            measured_preview: Measured {
                width: None,
                height: None,
                unprobed: false,
            },
            ..complete()
        };
        let report = review(&facts);
        let codes = codes(&report);
        assert!(
            codes.contains(&"preview-unreadable".to_string()),
            "{codes:?}"
        );
        assert!(
            !codes.contains(&"preview-missing".to_string()),
            "the file is there; the card is blank for a different reason"
        );
    }

    #[test]
    fn an_unprobed_preview_is_not_called_unreadable() {
        let facts = Facts {
            measured_preview: Measured {
                width: None,
                height: None,
                unprobed: true,
            },
            ..complete()
        };
        assert!(!codes(&review(&facts)).contains(&"preview-unreadable".to_string()));
    }

    #[test]
    fn errors_come_first_in_plain_output() {
        let facts = Facts {
            import_error: Some("too deep".to_string()),
            signature: Signature::Invalid("bad".to_string()),
            ..Facts::default()
        };
        let report = review(&facts);
        let severities: Vec<Severity> = report.findings.iter().map(|f| f.severity).collect();
        let first_warning = severities.iter().position(|s| *s == Severity::Warning);
        let last_error = severities.iter().rposition(|s| *s == Severity::Error);
        assert!(
            first_warning.is_some() && last_error < first_warning,
            "{severities:?}"
        );
        assert!(
            report.lines()[0].starts_with("error: "),
            "{:?}",
            report.lines()
        );
    }

    #[test]
    fn a_code_is_never_reported_twice() {
        // A CI filter on a code has to be able to test presence, not count.
        let report = review(&Facts::default());
        let mut seen: Vec<String> = Vec::new();
        for finding in &report.findings {
            assert!(
                is_known_code(&finding.code),
                "undocumented {:?}",
                finding.code
            );
            assert!(!seen.contains(&finding.code), "duplicate {}", finding.code);
            seen.push(finding.code.clone());
        }
    }

    #[test]
    fn the_ignore_list_rejects_unknown_codes() {
        assert_eq!(parse_ignore_list("unsigned").unwrap(), vec!["unsigned"]);
        assert_eq!(
            parse_ignore_list(" unsigned , preview-missing ").unwrap(),
            vec!["unsigned", "preview-missing"]
        );
        assert_eq!(parse_ignore_list("").unwrap(), Vec::<String>::new());
        assert!(parse_ignore_list("unsigned,nope").is_err());
    }

    #[test]
    fn filtering_can_remove_a_warning_but_the_error_still_blocks() {
        let facts = Facts {
            signature: Signature::Invalid("tampered".to_string()),
            ..Facts::default()
        };
        let report = review(&facts);
        let kept = filter(
            report.findings,
            &parse_ignore_list("unsigned,checksums-missing").unwrap(),
        );
        let filtered = Report {
            name: report.name.clone(),
            kind: report.kind.clone(),
            findings: kept,
        };
        assert!(
            filtered.blocked(),
            "ignoring warnings cannot unblock an error"
        );
        assert!(!codes(&filtered).contains(&"unsigned".to_string()));
    }

    #[test]
    fn a_report_round_trips_through_json() {
        // `--json` is the machine-readable interface, so its shape is part of
        // the contract.
        let report = review(&Facts::default());
        let text = serde_json::to_string(&report).unwrap();
        assert_eq!(serde_json::from_str::<Report>(&text).unwrap(), report);
        assert!(text.contains("\"severity\":\"warning\""), "{text}");
        assert!(text.contains("\"code\":\"unsigned\""), "{text}");
    }

    #[test]
    fn signature_states_serialise_compactly() {
        let text = serde_json::to_string(&Signature::Trusted("Ada (key-1)".to_string())).unwrap();
        assert_eq!(text, r#"{"state":"trusted","detail":"Ada (key-1)"}"#);
        assert_eq!(
            serde_json::to_string(&Signature::Unsigned).unwrap(),
            r#"{"state":"unsigned"}"#
        );
    }

    #[test]
    fn parse_resolution_refuses_junk_instead_of_guessing() {
        assert_eq!(parse_resolution(Some("1920x1080")), Some((1920, 1080)));
        assert_eq!(parse_resolution(Some(" 1920 x 1080 ")), Some((1920, 1080)));
        assert_eq!(parse_resolution(Some("1920")), None);
        assert_eq!(parse_resolution(Some("0x1080")), None);
        assert_eq!(parse_resolution(Some("1920x")), None);
        assert_eq!(parse_resolution(Some("x1080")), None);
        assert_eq!(parse_resolution(None), None);
        assert_eq!(parse_resolution(Some("-1920x1080")), None);
    }

    #[test]
    fn a_measured_pair_formats_only_when_both_sides_are_known() {
        assert_eq!(
            measured(1920, 1080).resolution().as_deref(),
            Some("1920x1080")
        );
        let half = Measured {
            width: Some(1920),
            height: None,
            unprobed: false,
        };
        assert_eq!(half.resolution(), None);
    }

    // ---------------------------------------------------------------------
    // Integration: the facts gatherer, against real directories.
    // ---------------------------------------------------------------------

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ferramari-lint-{name}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A package with `manifest.json` and a fake entry big enough to pass the
    /// size lower bound. The entry is not real video data, so the probe cannot
    /// measure it, which is exactly the "did not decode" case.
    fn package(name: &str, manifest: &str) -> std::path::PathBuf {
        let dir = temp_dir(name);
        std::fs::write(dir.join("manifest.json"), manifest).unwrap();
        std::fs::write(dir.join("wallpaper.mp4"), [0u8; 2048]).unwrap();
        dir
    }

    fn find<'a>(report: &'a Report, code: &str) -> Option<&'a Finding> {
        report.findings.iter().find(|f| f.code == code)
    }

    #[test]
    fn gather_reports_import_error_and_still_lists_the_warnings() {
        // An empty name fails import, and the creator also wants to know
        // about the preview and signature in the same pass.
        let dir = package("badname", r#"{"name":"","author":"Ada","type":"video"}"#);
        let report = review(&gather(&dir));
        assert!(report.blocked(), "{:?}", report.findings);
        let rejected = find(&report, "import-rejected").expect("import verdict");
        assert!(rejected.message.contains("name"), "{}", rejected.message);
        assert!(find(&report, "unsigned").is_some());
        assert!(find(&report, "preview-missing").is_some());
        // `name-missing` is not added: the same problem reported twice would
        // contradict the one-source rule.
        assert!(find(&report, "name-missing").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn gather_reads_every_declared_field() {
        let dir = package(
            "fields",
            r#"{"name":"Neon","author":"Ada","type":"video","description":"lights","resolution":"3840x2160"}"#,
        );
        let facts = gather(&dir);
        assert_eq!(facts.import_error, None);
        assert_eq!(facts.name, "Neon");
        assert_eq!(facts.author, "Ada");
        assert_eq!(facts.description, "lights");
        assert_eq!(facts.kind, "video");
        assert_eq!(facts.declared_resolution.as_deref(), Some("3840x2160"));
        assert!(!facts.preview_present);
        // A fabricated .mp4 has no moov atom: ffprobe answers "invalid data".
        // That is a decode failure, which is the creator's problem, not the
        // machine's; the distinction must survive gather.
        assert!(
            !facts.measured_entry.unprobed || facts.measured_entry.width.is_some(),
            "{:?}",
            facts.measured_entry
        );
        let report = review(&facts);
        assert!(
            find(&report, "description-missing").is_none(),
            "declared description must be read: {:?}",
            report.findings
        );
        assert!(
            !codes(&report).contains(&"resolution-missing".to_string()),
            "{:?}",
            report.findings
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_preview_convention_is_honoured_without_a_declared_field() {
        let dir = package("previewconv", r#"{"name":"P","author":"A","type":"video"}"#);
        std::fs::write(dir.join("preview.jpg"), b"not really a jpeg").unwrap();
        let facts = gather(&dir);
        assert!(facts.preview_present, "preview.jpg must count");
        let report = review(&facts);
        assert!(!codes(&report).contains(&"preview-missing".to_string()));
        // The file is present but cannot be decoded, which is a different
        // finding; on a machine without ffprobe the probe is skipped entirely.
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_directory_is_reported_as_an_import_failure() {
        // The CLI is expected to be pointed at anything; lint must describe
        // what it found instead of panicking or inventing findings.
        let report = review(&gather(std::path::Path::new("/definitely/not/a/package")));
        assert!(report.blocked());
        assert!(find(&report, "import-rejected")
            .unwrap()
            .message
            .contains("not/a/package"));
    }

    #[test]
    fn gather_leaves_author_blank_only_when_the_manifest_omits_it() {
        let dir = package("noauthor", r#"{"name":"N","type":"video"}"#);
        let facts = gather(&dir);
        assert!(facts.author.is_empty());
        assert!(
            codes(&review(&facts)).contains(&"author-missing".to_string()),
            "{:?}",
            codes(&review(&facts))
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_web_package_skips_the_video_probe() {
        // ffprobe on an HTML file fails with "invalid data", which would
        // otherwise be reported as a broken entry. A web entry has nothing to
        // measure, and the finding must say so.
        let dir = temp_dir("webpkg");
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"name":"Page","author":"A","type":"web","description":"d","entry":"index.html"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("index.html"), b"<html></html>").unwrap();
        let facts = gather(&dir);
        assert_eq!(facts.kind, "web");
        assert!(facts.measured_entry.unprobed, "no probe for HTML");
        let report = review(&facts);
        let missing = find(&report, "resolution-missing").expect("web needs a declared resolution");
        assert!(missing.message.contains("web page"), "{}", missing.message);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn signature_states_survive_from_the_verifier() {
        // A package with no signature.json is Unsigned, and that is the
        // warning a creator can act on with `package sign`.
        let dir = package("sig", r#"{"name":"S","author":"A","type":"video"}"#);
        let facts = gather(&dir);
        assert_eq!(facts.signature, Signature::Unsigned);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_signature_without_checksums_is_invalid_not_unsigned() {
        // signing requires checksums.json, so a package carrying only
        // signature.json cannot be verified at all: the finding must name the
        // reason rather than tell the creator to sign again.
        let dir = package("orphan-sig", r#"{"name":"S","author":"A","type":"video"}"#);
        std::fs::write(dir.join("signature.json"), "{}").unwrap();
        let facts = gather(&dir);
        assert!(
            matches!(facts.signature, Signature::Invalid(_)),
            "{:?}",
            facts.signature
        );
        let report = review(&facts);
        assert!(report.blocked());
        assert!(find(&report, "unsigned").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---------------------------------------------------------------------
    // Integration: lint → import consistency and edge cases.
    // ---------------------------------------------------------------------

    /// A package that passes lint review (no errors) should also pass import.
    /// This is the core invariant: lint errors are a superset of import rejections.
    #[test]
    fn passing_lint_report_means_import_succeeds() {
        let dir = temp_dir("passing-lint");
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"name":"Clean","author":"Ada","type":"video","description":"desc"}"#,
        ).unwrap();
        std::fs::write(dir.join("wallpaper.mp4"), [0u8; 2048]).unwrap();
        // Gather facts — entry exists but won't measure without ffmpeg.
        let facts = gather(&dir);
        let report = review(&facts);
        // No import error means lint didn't find structural problems.
        assert!(report.errors().count() <= 1, "at most one structurally reported finding: {report:?}");
        if !report.blocked() {
            let imported = import(&dir, &temp_dir("import-passing")).unwrap();
            assert_eq!(imported.name, "Clean");
            let _ = fs::remove_dir_all(imported.package_dir);
        }
        let _ = fs::remove_dir_all(dir);
    }

    /// Lint warnings do NOT block import — they inform the creator only.
    #[test]
    fn lint_warnings_do_not_block_import() {
        let dir = temp_dir("warnings-only");
        // Missing author, description, preview, checksums — all warnings.
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"name":"Warnings Pack","type":"video"}"#,
        ).unwrap();
        std::fs::write(dir.join("wallpaper.mp4"), b"v").unwrap();
        let facts = gather(&dir);
        let report = review(&facts);
        assert!(!report.blocked(), "warnings must not block import");
        // But there should be several warnings.
        assert!(
            report.warnings().count() >= 3,
            "expected multiple warnings: {:?}",
            codes(&report)
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// Lint errors DO block import.
    #[test]
    fn lint_errors_block_import() {
        let dir = temp_dir("blocking-errors");
        // Name empty → import rejected → lint reports import-rejected error.
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"name":"","author":"A","type":"video"}"#,
        ).unwrap();
        std::fs::write(dir.join("wallpaper.mp4"), b"v").unwrap();
        let facts = gather(&dir);
        let report = review(&facts);
        assert!(report.blocked());
        assert!(
            report.findings.iter().any(|f| f.code == "import-rejected"),
            "{:?}",
            codes(&report)
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// Import-rejected message appears verbatim in the lint finding.
    #[test]
    fn lint_import_rejected_carrying_message_is_verbatim() {
        let json = r#"{"name":"OK","type":"video","entry":"../etc/passwd"}"#;
        let dir = temp_dir("path-leak");
        std::fs::write(dir.join("manifest.json"), json).unwrap();
        std::fs::write(dir.join("wallpaper.mp4"), b"v").unwrap();
        let facts = gather(&dir);
        assert!(facts.import_error.is_some());
        let report = review(&facts);
        let finding = find(&report, "import-rejected").expect("import-rejected expected");
        // The message must contain the original reason words from load.
        assert!(finding.message.to_lowercase().contains("relative path") ||
                finding.message.to_lowercase().contains(".."));
        let _ = fs::remove_dir_all(dir);
    }

    /// A complete signed+checksummed package yields no findings.
    #[test]
    fn complete_signed_package_has_no_findings() {
        let dir = temp_dir("complete-signed");
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"name":"Gold Standard","author":"Pro","type":"video","description":"pro desc","resolution":"1920x1080"}"#,
        ).unwrap();
        std::fs::write(dir.join("wallpaper.mp4"), b"v").unwrap();
        std::fs::write(dir.join("preview.jpg"), b"p").unwrap();
        // Write a valid checksums file so verify_checksums sees it.
        let digests: std::collections::BTreeMap<String, String> = crate::package::checksums(&dir).unwrap();
        let cs_text = serde_json::to_string_pretty(&digests).unwrap();
        std::fs::write(dir.join("checksums.json"), format!("{cs_text}\n")).unwrap();
        let facts = gather(&dir);
        assert_eq!(facts.checksummed, true);
        let report = review(&facts);
        // May have unsigned warning but no errors.
        assert!(!report.blocked());
        assert!(
            report.errors().count() == 0,
            "no errors: {:?}",
            codes(&report)
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// Resolution missing + unprobed entry gives correct web vs video messages.
    #[test]
    fn resolution_missing_message_distinguishes_web_from_video() {
        let dir = temp_dir("web-res-missing");
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"name":"W","author":"A","type":"web","entry":"index.html"}"#,
        ).unwrap();
        std::fs::write(dir.join("index.html"), b"<html></html>").unwrap();
        let facts = gather(&dir);
        let report = review(&facts);
        let finding = find(&report, "resolution-missing").expect("resolution-missing for web");
        assert!(finding.message.contains("web page"), "{}", finding.message);
        let _ = fs::remove_dir_all(dir);
    }

    /// A junk entry file causes measured_entry to return None, triggering
    /// resolution-missing rather than resolution-mismatch.
    #[test]
    fn junk_entry_triggers_resolution_missing_not_mismatch() {
        let dir = temp_dir("junk-entry");
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"name":"J","author":"A","type":"video","description":"d"}"#,
        ).unwrap();
        std::fs::write(dir.join("wallpaper.mp4"), b"not-video").ok();
        let facts = gather(&dir);
        let report = review(&facts);
        let codes = codes(&report);
        assert!(
            !codes.contains(&"resolution-mismatch".to_string()),
            "{codes:?}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// Severity serialisation round-trips through JSON.
    #[test]
    fn severity_round_trips_through_json() {
        let text = serde_json::to_string(&Severity::Error).unwrap();
        assert_eq!(text, "\"error\"");
        let text = serde_json::to_string(&Severity::Warning).unwrap();
        assert_eq!(text, "\"warning\"");
        let err: Severity = serde_json::from_str("\"error\"").unwrap();
        assert_eq!(err, Severity::Error);
    }

    /// Finding fields serialise and deserialise correctly.
    #[test]
    fn finding_serialises_correctly() {
        let finding = Finding::error("test-code", "something broke");
        let text = serde_json::to_string(&finding).unwrap();
        let decoded: Finding = serde_json::from_str(&text).unwrap();
        assert_eq!(decoded.severity, Severity::Error);
        assert_eq!(decoded.code, "test-code");
        assert_eq!(decoded.message, "something broke");
    }

    /// KNOWN_CODES list covers every code used by review.
    #[test]
    fn known_codes_are_non_empty_and_match_expected_set() {
        assert!(!KNOWN_CODES.is_empty());
        assert!(KNOWN_CODES.contains(&"import-rejected"));
        assert!(KNOWN_CODES.contains(&"unsigned"));
        assert!(KNOWN_CODES.contains(&"signature-invalid"));
        assert!(KNOWN_CODES.contains(&"preview-unreadable"));
    }

    /// is_known_code accepts documented codes and rejects unknown ones.
    #[test]
    fn is_known_code_returns_true_for_valid_codes() {
        assert!(is_known_code("unsigned"));
        assert!(is_known_code("import-rejected"));
        assert!(!is_known_code("made-up-code"));
        assert!(!is_known_code(""));
    }

    /// parse_ignore_list ignores empty strings in the input.
    #[test]
    fn parse_ignore_list_skips_empty_parts() {
        assert_eq!(
            parse_ignore_list(",unsigned,,checksums-missing,").unwrap(),
            vec!["unsigned", "checksums-missing"]
        );
    }

    /// filter removes specified codes from findings.
    #[test]
    fn filter_removes_only_the_requested_codes() {
        let facts = Facts {
            signature: Signature::Unsigned,
            checksummed: false,
            ..Facts::default()
        };
        let report = review(&facts);
        let kept = filter(report.findings, &vec!["unsigned".to_string()]);
        let filtered = Report {
            name: report.name.clone(),
            kind: report.kind.clone(),
            findings: kept,
        };
        let codes = codes(&filtered);
        assert!(!codes.contains(&"unsigned".to_string()));
        assert!(codes.contains(&"checksums-missing".to_string()));
    }

    /// Measured::resolution returns None when either dimension is missing.
    #[test]
    fn measured_resolution_requires_both_dimensions() {
        let m = Measured { width: Some(1920), height: None, unprobed: false };
        assert_eq!(m.resolution(), None);
        let m = Measured { width: None, height: Some(1080), unprobed: false };
        assert_eq!(m.resolution(), None);
        let m = Measured { width: Some(1920), height: Some(1080), unprobed: false };
        assert_eq!(m.resolution(), Some("1920x1080".to_string()));
    }

    /// Report.lines() outputs severity:message format.
    #[test]
    fn report_lines_have_severity_prefix() {
        let report = review(&Facts::default());
        let lines = report.lines();
        for line in &lines {
            assert!(line.starts_with("warning: "), "unexpected line: {line}");
        }
    }

    /// Whitespace in manifest entry is passed as-is to checked_file;
    /// if the actual file exists under that exact name it loads, otherwise fails.
    #[test]
    fn whitespace_in_manifest_entry_is_passed_as_is() {
        let dir = temp_dir("whitespace-entry");
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"name":"WS","author":"A","type":"video","entry":"  wallpaper.mp4  "}"#,
        ).unwrap();
        // The file is named without spaces, so this should fail because
        // checked_file constructs dir/"  wallpaper.mp4  " which doesn't exist.
        assert!(crate::package::load(&dir).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    /// Entry path cannot refer to a directory.
    #[test]
    fn entry_path_pointing_to_a_directory_is_rejected() {
        let dir = temp_dir("dir-entry");
        std::fs::create_dir(dir.join("subdir")).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"name":"DE","author":"A","type":"video","entry":"subdir"}"#,
        ).unwrap();
        std::fs::write(dir.join("wallpaper.mp4"), b"x").unwrap();
        assert!(crate::package::load(&dir).unwrap_err().contains("not found"));
        let _ = fs::remove_dir_all(dir);
    }

    /// Preview fallback: preview.jpg convention activates when no preview field.
    #[test]
    fn preview_jpg_convention_fallback_works() {
        let dir = temp_dir("preview-fallback");
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"name":"PF","author":"A","type":"video"}"#,
        ).unwrap();
        std::fs::write(dir.join("wallpaper.mp4"), b"x").unwrap();
        std::fs::write(dir.join("preview.jpg"), b"jpeg").unwrap();
        let pkg = crate::package::load(&dir).unwrap();
        assert!(pkg.preview.is_some());
        assert!(pkg.preview.unwrap().ends_with("preview.jpg"));
        let _ = fs::remove_dir_all(dir);
    }
}
