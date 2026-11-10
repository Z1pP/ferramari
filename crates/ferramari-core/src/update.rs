//! Release check for the control surface's "Check for updates" setting.
//!
//! The project publishes releases on GitHub, and that is the only place a
//! new version can come from, so the check is one GET against the releases
//! API and nothing else. Two rules shape the rest of the module:
//!
//! * **A check is a network call with a deadline.** `ureq` is configured with
//!   explicit timeouts, and every failure path returns a [`Status`] that says
//!   what happened rather than an error the UI has to interpret.
//! * **A result is remembered, not re-fetched.** The answer is cached in
//!   `update.json` with the time it was taken, so an automatic check on
//!   startup costs one file read, and the user can see how old the answer is.
//!   Staleness is stated, never used to invent a newer version.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::util::tr;

/// The repository releases come from. Kept here rather than in a build-time
/// constant so a downstream packager can point it at their own.
pub const REPOSITORY: &str = "Z1pP/ferramari";

/// How long a cached answer stays fresh enough to show without re-checking.
/// A week matches how often the project releases and how often a desktop
/// should ask a public API about it.
pub const FRESH_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The version this binary was built as, which is the version being compared.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// What a check can answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// This build is the published release, or newer than it.
    UpToDate,
    /// A release exists that this build is not.
    Available {
        /// The release tag, `v2.1.0`.
        version: String,
        /// The release page, for the link the settings row offers.
        url: String,
        /// The release title, when the API supplied one.
        title: String,
    },
    /// The check has never run, or its result could not be read.
    Unknown,
    /// The check ran and could not finish. The reason is the sentence the UI
    /// shows: an offline machine and a moved repository are different
    /// problems and should not read the same.
    Failed(String),
}

impl Status {
    /// One line for the settings row.
    pub fn text(&self) -> String {
        match self {
            Status::UpToDate => tr("You are up to date"),
            Status::Available { version, .. } => {
                format!("{} {version}", tr("Update available:"))
            }
            Status::Unknown => tr("Never checked"),
            Status::Failed(reason) => reason.clone(),
        }
    }

    /// Whether anything newer than this build is known.
    pub fn is_available(&self) -> bool {
        matches!(self, Status::Available { .. })
    }
}

/// The cached answer, on disk.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Whether the control surface checks by itself when it starts.
    #[serde(default)]
    pub automatic: bool,
    /// Unix seconds of the last completed check, `0` when there was none.
    #[serde(default)]
    pub checked_at: u64,
    /// The tag the last check saw, kept so a fresh record can be answered
    /// without another request.
    #[serde(default)]
    pub version: String,
    /// The release page of that tag.
    #[serde(default)]
    pub url: String,
    /// The release title of that tag.
    #[serde(default)]
    pub title: String,
    /// Why the last check failed, when it did.
    #[serde(default)]
    pub error: String,
}

pub fn record_path() -> PathBuf {
    paths::config_dir().join("update.json")
}

impl Record {
    pub fn load() -> Self {
        Self::load_from(&record_path())
    }

    /// A record that cannot be read is a record that was never written: the
    /// settings page shows "never checked" rather than guessing.
    fn load_from(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn store(&self) -> Result<(), String> {
        self.store_to(&record_path())
    }

    /// Written the same way as the configuration: a temp file and a rename,
    /// so an interrupted write cannot leave the settings page reading a
    /// half-written file.
    fn store_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| format!("could not serialize the update record: {error}"))?;
        let temp = path.with_extension("json.tmp");
        fs::write(&temp, format!("{text}\n")).map_err(|error| error.to_string())?;
        fs::rename(&temp, path).map_err(|error| error.to_string())
    }

    pub fn status(&self) -> Status {
        if self.checked_at == 0 {
            return Status::Unknown;
        }
        if !self.error.is_empty() {
            return Status::Failed(self.error.clone());
        }
        if self.version.is_empty() {
            return Status::Unknown;
        }
        if is_newer(&self.version, current_version()) {
            Status::Available {
                version: self.version.clone(),
                url: self.url.clone(),
                title: self.title.clone(),
            }
        } else {
            Status::UpToDate
        }
    }

    /// Whether the cached answer is recent enough to show as the truth.
    pub fn is_fresh(&self) -> bool {
        self.checked_at != 0
            && now().is_some_and(|now| now.saturating_sub(self.checked_at) < FRESH_FOR.as_secs())
    }

    /// When the check ran, as a date. `None` when the clock said nothing or
    /// the record has no check in it.
    pub fn checked_on(&self) -> Option<String> {
        if self.checked_at == 0 {
            return None;
        }
        // No date library in the tree, and none is worth adding for one
        // label: the epoch day is enough to say "this answer is from".
        let days = self.checked_at / 86_400;
        let (y, m, d) = civil_from_days(days as i64);
        Some(format!("{y:04}-{m:02}-{d:02}"))
    }
}

/// Unix seconds, or `None` when the clock is before the epoch.
fn now() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|since| since.as_secs())
}

/// Howard Hinnant's `civil_from_days`, the standard algorithm: the date a
/// Unix timestamp falls on, without a date crate.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Whether `candidate` is a later release than `current`.
///
/// Compares the numeric components of dotted versions and ignores a leading
/// `v` and any `-rc1`-style suffix: a pre-release is not an update to offer
/// an ordinary user, and neither is a tag whose numbers we cannot read.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

fn parse_version(text: &str) -> Option<Vec<u64>> {
    let text = text.trim().trim_start_matches('v');
    // Drop a pre-release or build suffix before the numbers are read.
    let core = text
        .split(['-', '+'])
        .next()
        .filter(|core| !core.is_empty())?;
    let parts: Vec<u64> = core
        .split('.')
        .map(|part| part.parse().ok())
        .collect::<Option<Vec<u64>>>()?;
    (!parts.is_empty()).then_some(parts)
}

/// The published release, straight from the API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub title: String,
}

/// Ask the releases API for the latest release and record the answer.
///
/// Blocking by design: the caller is a worker thread, never the UI event
/// loop. The timeouts are short because the check is a courtesy, not a
/// dependency — a slow API should read as "could not check", not as a
/// settings page that hangs.
/// A release record is a few hundred bytes; the cap is what turns a hostile
/// or misconfigured endpoint into a failure rather than a memory problem. The
/// same bound the catalogue client uses, for the same reason.
const MAX_BODY_BYTES: u64 = 64 * 1024;

pub fn check() -> Status {
    check_from(&format!(
        "https://api.github.com/repos/{REPOSITORY}/releases/latest"
    ))
}

/// The check with its endpoint named, so a test can point it at something
/// that is not the live API.
fn check_from(url: &str) -> Status {
    let request = ureq::get(url)
        .timeout(Duration::from_secs(15))
        .set("User-Agent", &format!("ferramari/{}", current_version()))
        .set("Accept", "application/vnd.github+json")
        .call();

    let outcome: Result<Release, String> = match request {
        Ok(response) => read_release(response),
        Err(ureq::Error::Status(code, response)) => Err(unfriendly(code, &response)),
        Err(error) => Err(format!("{}: {error}", tr("Could not reach GitHub"))),
    };

    // The switch is the user's, not the check's: a failed request must not
    // turn automatic checking off, and a successful one must not turn it on.
    let mut record = Record {
        automatic: Record::load().automatic,
        checked_at: now().unwrap_or_default(),
        version: String::new(),
        url: String::new(),
        title: String::new(),
        error: String::new(),
    };
    match outcome {
        Ok(release) if release.version.is_empty() => {
            record.error = tr("The release listing had no version");
        }
        Ok(release) => {
            record.version = release.version;
            record.url = release.url;
            record.title = release.title;
        }
        Err(reason) => record.error = reason,
    }
    // A record that cannot be written is still a check that happened: the
    // answer is returned either way and the next start simply asks again.
    let _ = record.store();
    record.status()
}

fn read_release(response: ureq::Response) -> Result<Release, String> {
    use std::io::Read;
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_BODY_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|error| format!("{}: {error}", tr("Could not read the release listing")))?;
    if body.len() as u64 > MAX_BODY_BYTES {
        return Err(tr("The release listing was unexpectedly large"));
    }
    let value: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|error| format!("{}: {error}", tr("The release listing was not valid JSON")))?;
    let field = |name: &str| {
        value
            .get(name)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Ok(Release {
        version: field("tag_name"),
        url: field("html_url"),
        title: field("name"),
    })
}

/// What an HTTP error means to the user. GitHub answers a rate limit with
/// 403 and a moved repository with 404, and both read as "could not check"
/// with a reason that points at the real problem.
fn unfriendly(code: u16, response: &ureq::Response) -> String {
    match code {
        403 if response
            .header("x-ratelimit-remaining")
            .is_some_and(|value| value == "0") =>
        {
            tr("GitHub rate limit reached; try again later")
        }
        404 => tr("The release listing was not found (is the project still at the same address?)"),
        other => format!("{}: HTTP {other}", tr("Could not reach GitHub")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn a_later_release_is_newer_and_an_earlier_one_is_not() {
        assert!(is_newer("v2.1.0", "2.0.0"));
        assert!(is_newer("2.0.1", "2.0.0"));
        assert!(is_newer("10.0.0", "9.9.9"));
        assert!(!is_newer("2.0.0", "2.0.0"));
        assert!(!is_newer("v1.9.0", "2.0.0"));
    }

    #[test]
    fn a_version_we_cannot_read_is_never_an_offer() {
        // The danger here is the false positive: telling a user an update is
        // available because the tag was `nightly-2026` would be a lie with a
        // link attached.
        assert!(!is_newer("nightly", "2.0.0"));
        assert!(!is_newer("", "2.0.0"));
        assert!(!is_newer("2.x.0", "2.0.0"));
        assert!(!is_newer("2.0.0", "not a version"));
    }

    #[test]
    fn a_pre_release_is_not_offered_as_an_update() {
        assert!(!is_newer("v2.0.0-rc1", "2.0.0"));
        // The numbers still compare correctly once the suffix is set aside.
        assert!(is_newer("v2.1.0-rc1", "2.0.0"));
    }

    #[test]
    fn an_empty_record_has_never_been_checked() {
        assert_eq!(Record::default().status(), Status::Unknown);
        assert_eq!(Record::default().checked_on(), None);
        assert!(!Record::default().is_fresh());
    }

    #[test]
    fn a_record_with_an_error_reports_the_error() {
        let record = Record {
            checked_at: 1_700_000_000,
            error: "no network".to_string(),
            ..Default::default()
        };
        assert_eq!(record.status(), Status::Failed("no network".to_string()));
    }

    #[test]
    fn a_record_with_a_newer_tag_offers_it() {
        let record = Record {
            checked_at: 1_700_000_000,
            version: "99.0.0".to_string(),
            url: "https://example.invalid".to_string(),
            title: "Test".to_string(),
            error: String::new(),
            automatic: false,
        };
        assert!(record.status().is_available());
        assert_eq!(
            record
                .checked_on()
                .unwrap()
                .split('-')
                .next()
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn a_record_with_the_current_tag_is_up_to_date() {
        let record = Record {
            checked_at: 1_700_000_000,
            version: current_version().to_string(),
            ..Default::default()
        };
        assert_eq!(record.status(), Status::UpToDate);
    }

    #[test]
    fn the_record_round_trips_through_the_file() {
        // The file is the only thing between a check and the settings row, so
        // a field that does not survive `store`/`load` silently resets the
        // user's choice. Written to a scratch path: a test must not touch the
        // configuration the user is running with.
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("ferramari-update-{}-{unique}", std::process::id()));
        let path = dir.join("update.json");

        let original = Record {
            automatic: true,
            checked_at: 1_700_000_000,
            version: "3.0.0".to_string(),
            url: "https://example.invalid".to_string(),
            title: "Third".to_string(),
            error: String::new(),
        };
        original.store_to(&path).unwrap();
        assert_eq!(Record::load_from(&path), original);

        // A file that is not JSON reads as "never checked", not as a panic.
        fs::write(&path, "this is not json").unwrap();
        assert_eq!(Record::load_from(&path), Record::default());

        let _ = fs::remove_dir_all(dir);
    }
}
