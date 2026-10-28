//! yt-dlp integration.
//!
//! Upstream used the `yt_dlp` Python module. Here the `yt-dlp` executable is
//! asked for the JSON description of a video (`-J`), and the format selection
//! is plain Rust so it can be unit-tested without network access.

use std::cmp::Ordering;
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Format {
    pub url: String,
    pub acodec: Option<String>,
    pub vcodec: Option<String>,
    /// yt-dlp has returned this as an integer, a double (`2.0`) and sometimes a
    /// string over time; keep it as a float and parse leniently.
    pub quality: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct Formats {
    pub formats: Vec<Format>,
}

#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub video_url: String,
    pub audio_url: Option<String>,
    pub width: u32,
    pub height: u32,
}

impl Formats {
    /// Parse `yt-dlp -J` output.
    ///
    /// Field-by-field parsing (instead of `#[derive(Deserialize)]` on the whole
    /// document) means one odd entry cannot reject an otherwise valid video.
    /// yt-dlp changes the type of `quality` between releases, and a single
    /// `2.0` where an integer was expected used to break streaming entirely.
    pub fn parse(json: &str) -> Result<Self, String> {
        let root: Value =
            serde_json::from_str(json).map_err(|err| format!("unexpected yt-dlp output: {err}"))?;
        let entries = root
            .get("formats")
            .and_then(Value::as_array)
            .ok_or_else(|| "unexpected yt-dlp output: missing formats array".to_string())?;
        let formats: Vec<Format> = entries.iter().filter_map(parse_format).collect();
        if formats.is_empty() {
            return Err("yt-dlp returned no playable formats".to_string());
        }
        Ok(Self { formats })
    }

    fn audio_only(&self) -> impl Iterator<Item = &Format> {
        self.formats
            .iter()
            .filter(|f| f.acodec.as_deref() != Some("none") && f.vcodec.as_deref() == Some("none"))
    }

    fn video_only(&self) -> impl Iterator<Item = &Format> {
        self.formats
            .iter()
            .filter(|f| f.vcodec.as_deref() != Some("none") && f.acodec.as_deref() == Some("none"))
    }

    fn audio_video(&self) -> impl Iterator<Item = &Format> {
        self.formats
            .iter()
            .filter(|f| f.vcodec.as_deref() != Some("none") && f.acodec.as_deref() != Some("none"))
    }

    /// Highest-quality audio stream, falling back to a muxed stream.
    pub fn best_audio(&self) -> Option<&Format> {
        let audio: Vec<&Format> = self.audio_only().collect();
        let candidates: Vec<&Format> = if audio.is_empty() {
            self.audio_video().collect()
        } else {
            audio
        };
        candidates.into_iter().max_by(|left, right| {
            left.quality
                .unwrap_or(-1.0)
                .partial_cmp(&right.quality.unwrap_or(-1.0))
                .unwrap_or(Ordering::Equal)
        })
    }

    /// Video stream whose height is closest to the given screen height.
    pub fn optimal_video(&self, height: u32) -> Option<&Format> {
        let video: Vec<&Format> = self.video_only().collect();
        let candidates: Vec<&Format> = if video.is_empty() {
            self.audio_video().collect()
        } else {
            video
        };
        candidates.into_iter().min_by_key(|format| {
            let format_height = format.height.unwrap_or(0) as i64;
            // Closest height, and prefer the sharper stream on a tie.
            ((format_height - height as i64).abs(), -format_height)
        })
    }
}

fn parse_format(value: &Value) -> Option<Format> {
    let url = value.get("url").and_then(Value::as_str)?.to_string();
    if url.is_empty() {
        return None;
    }
    Some(Format {
        url,
        acodec: string_field(value, "acodec"),
        vcodec: string_field(value, "vcodec"),
        quality: number_field(value, "quality"),
        width: u32_field(value, "width"),
        height: u32_field(value, "height"),
    })
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn number_field(value: &Value, key: &str) -> Option<f64> {
    let value = value.get(key)?;
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

fn u32_field(value: &Value, key: &str) -> Option<u32> {
    let value = value.get(key)?;
    value
        .as_u64()
        .map(|number| number as u32)
        .or_else(|| value.as_f64().map(|number| number.max(0.0) as u32))
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

/// Resolve a streaming URL to a playable video (+ separate audio) stream.
pub fn resolve(url: &str, screen_height: u32) -> Result<StreamInfo, String> {
    log::info!("[yt-dlp] Resolving {url}");
    let output = Command::new(crate::paths::tool("yt-dlp"))
        .args(["-J", "--no-playlist", "--no-warnings", url])
        .output()
        .map_err(|err| format!("could not run yt-dlp: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr
            .lines()
            .find(|line| line.starts_with("ERROR:"))
            .unwrap_or_else(|| stderr.lines().next().unwrap_or("yt-dlp failed"))
            .trim()
            .to_string();
        return Err(reason);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let formats = Formats::parse(&text)?;
    let video = formats
        .optimal_video(screen_height)
        .ok_or_else(|| "no video stream found".to_string())?;
    // A muxed stream already carries its audio; only attach a separate audio
    // track when yt-dlp handed us a video-only format.
    let audio = if video.acodec.as_deref() == Some("none") {
        formats.best_audio().map(|format| format.url.clone())
    } else {
        None
    };
    Ok(StreamInfo {
        video_url: video.url.clone(),
        audio_url: audio,
        width: video.width.unwrap_or(1920),
        height: video.height.unwrap_or(1080),
    })
}

/// Lightweight reachability check, matching upstream's pre-flight request.
pub fn check_url(url: &str) -> Result<(), String> {
    match ureq::get(url).timeout(Duration::from_secs(20)).call() {
        Ok(response) if response.status() < 400 => Ok(()),
        Ok(response) => Err(format!(
            "Failed to access {url}. Error code: {}",
            response.status()
        )),
        Err(err) => Err(format!("Failed to access {url}. Error:\n{err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> &'static str {
        r#"{
          "formats": [
            {"url": "https://x/av-low",  "acodec": "mp4a", "vcodec": "avc1", "quality": 1, "width": 640,  "height": 360},
            {"url": "https://x/av-high", "acodec": "mp4a", "vcodec": "avc1", "quality": 2, "width": 1920, "height": 1080},
            {"url": "https://x/v-720",   "acodec": "none", "vcodec": "vp9",  "quality": 3, "width": 1280, "height": 720},
            {"url": "https://x/v-1440",  "acodec": "none", "vcodec": "vp9",  "quality": 4, "width": 2560, "height": 1440},
            {"url": "https://x/a-128",   "acodec": "opus", "vcodec": "none", "quality": 5},
            {"url": "",                  "acodec": "opus", "vcodec": "none", "quality": 9}
          ]
        }"#
    }

    #[test]
    fn parses_and_drops_empty_urls() {
        let formats = Formats::parse(sample()).unwrap();
        assert_eq!(formats.formats.len(), 5);
    }

    #[test]
    fn picks_highest_quality_audio() {
        let formats = Formats::parse(sample()).unwrap();
        assert_eq!(formats.best_audio().unwrap().url, "https://x/a-128");
    }

    #[test]
    fn picks_video_closest_to_screen_height() {
        let formats = Formats::parse(sample()).unwrap();
        assert_eq!(formats.optimal_video(1080).unwrap().url, "https://x/v-1440");
        assert_eq!(formats.optimal_video(720).unwrap().url, "https://x/v-720");
        assert_eq!(formats.optimal_video(4000).unwrap().url, "https://x/v-1440");
    }

    #[test]
    fn falls_back_to_muxed_streams() {
        let json = r#"{"formats": [
            {"url": "https://x/av", "acodec": "mp4a", "vcodec": "avc1", "quality": 1, "width": 800, "height": 600}
        ]}"#;
        let formats = Formats::parse(json).unwrap();
        let video = formats.optimal_video(600).unwrap();
        assert_eq!(video.url, "https://x/av");
        assert_eq!(formats.best_audio().unwrap().url, "https://x/av");
    }

    #[test]
    fn accepts_float_quality_and_string_numbers() {
        // GNOME 50's yt-dlp returns "quality": 2.0 for some YouTube formats;
        // the old derived deserializer rejected the whole response.
        let json = r#"{
          "formats": [
            {"url": "https://x/a", "acodec": "opus", "vcodec": "none", "quality": 2.0},
            {"url": "https://x/b", "acodec": "opus", "vcodec": "none", "quality": "3.5"},
            {"url": "https://x/v", "acodec": "none", "vcodec": "vp9", "width": 1920.0, "height": "1080", "quality": null},
            {"url": "", "acodec": "opus", "vcodec": "none"},
            null,
            42
          ]
        }"#;
        let formats = Formats::parse(json).unwrap();
        assert_eq!(formats.formats.len(), 3);
        assert_eq!(formats.best_audio().unwrap().url, "https://x/b");
        assert_eq!(formats.optimal_video(1080).unwrap().url, "https://x/v");
        assert_eq!(formats.formats[2].width, Some(1920));
        assert_eq!(formats.formats[2].height, Some(1080));
    }

    #[test]
    fn rejects_empty_format_list() {
        assert!(Formats::parse(r#"{"formats": []}"#).is_err());
        assert!(Formats::parse("garbage").is_err());
    }
}
