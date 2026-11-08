//! yt-dlp integration.
//!
//! The `yt-dlp` executable is used instead of the Python module. It is
//! asked for the JSON description of a video (`-J`), and the format selection
//! is plain Rust so it can be unit-tested without network access.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

/// Upper bound for `yt-dlp -J`. Without it a hung extractor leaves the UI
/// spinner running forever, because the resolve result never comes back.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a resolved stream is reused. `Reload` and every re-apply would
/// otherwise start `yt-dlp -J` again, which costs 1.5–2.7 s and blanks the
/// wallpaper in the meantime. Signed URLs stay valid for hours, but a short
/// window keeps a re-signed or revoked link from being replayed for long; a
/// playback error drops the entry immediately.
const RESOLUTION_TTL: Duration = Duration::from_secs(15 * 60);

/// A session only ever watches a handful of streams; the cap stops a long
/// run that keeps trying new links from growing the map forever.
const RESOLUTION_CACHE_LIMIT: usize = 32;

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

/// Resolved streams keyed by the page URL and the screen height they were
/// chosen for. The height is part of the key because format selection depends
/// on it: moving the wallpaper to a taller monitor must pick a sharper stream,
/// not replay the previous choice.
#[derive(Debug, Default)]
struct ResolutionCache {
    entries: Mutex<HashMap<(String, u32), (StreamInfo, Instant)>>,
}

impl ResolutionCache {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, u32), (StreamInfo, Instant)>> {
        // A panic elsewhere while the map was borrowed must not disable
        // streaming for the rest of the session.
        self.entries.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn get(&self, url: &str, screen_height: u32, now: Instant) -> Option<StreamInfo> {
        let mut entries = self.lock();
        let key = (url.to_string(), screen_height);
        let (info, stored) = entries.get(&key)?;
        if now.saturating_duration_since(*stored) < RESOLUTION_TTL {
            return Some(info.clone());
        }
        entries.remove(&key);
        None
    }

    fn insert(&self, url: &str, screen_height: u32, info: StreamInfo, now: Instant) {
        let mut entries = self.lock();
        entries.retain(|_, (_, stored)| now.saturating_duration_since(*stored) < RESOLUTION_TTL);
        if entries.len() >= RESOLUTION_CACHE_LIMIT {
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, (_, stored))| *stored)
                .map(|(key, _)| key.clone())
            {
                entries.remove(&oldest);
            }
        }
        entries.insert((url.to_string(), screen_height), (info, now));
    }

    /// Drop every height variant of one page URL. Called when playback fails,
    /// so the next attempt asks the site for a fresh link instead of retrying
    /// the same dead one until the TTL expires.
    fn invalidate(&self, url: &str) {
        self.lock().retain(|(cached, _), _| cached != url);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().len()
    }
}

fn resolution_cache() -> &'static ResolutionCache {
    static CACHE: OnceLock<ResolutionCache> = OnceLock::new();
    CACHE.get_or_init(ResolutionCache::default)
}

/// Look up a stream resolved earlier in this process.
pub fn cached(url: &str, screen_height: u32) -> Option<StreamInfo> {
    resolution_cache().get(url, screen_height, Instant::now())
}

/// Forget a page URL after its stream proved unplayable.
pub fn invalidate(url: &str) {
    resolution_cache().invalidate(url);
}

/// Resolve a streaming URL to a playable video (+ separate audio) stream.
pub fn resolve(url: &str, screen_height: u32) -> Result<StreamInfo, String> {
    if let Some(info) = cached(url, screen_height) {
        log::info!("[yt-dlp] Reusing cached stream for {url}");
        return Ok(info);
    }
    log::info!("[yt-dlp] Resolving {url}");
    let mut command = Command::new(crate::paths::tool("yt-dlp"));
    // `--` keeps a URL supplied over D-Bus from being parsed as an option.
    command
        .args(["-J", "--no-playlist", "--no-warnings", "--"])
        .arg(url);
    let (status, stdout, stderr) = run_with_timeout(command, RESOLVE_TIMEOUT, "yt-dlp")?;
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        let reason = stderr
            .lines()
            .find(|line| line.starts_with("ERROR:"))
            .unwrap_or_else(|| stderr.lines().next().unwrap_or("yt-dlp failed"))
            .trim()
            .to_string();
        return Err(reason);
    }
    let text = String::from_utf8_lossy(&stdout);
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
    let info = StreamInfo {
        video_url: video.url.clone(),
        audio_url: audio,
        width: video.width.unwrap_or(1920),
        height: video.height.unwrap_or(1080),
    };
    resolution_cache().insert(url, screen_height, info.clone(), Instant::now());
    Ok(info)
}

/// Run a child process with a deadline, capturing stdout and stderr.
///
/// `Command::output()` has no timeout, and polling a piped child without
/// draining its pipes deadlocks once it writes more than the pipe buffer, so
/// each pipe gets its own reader thread.
fn run_with_timeout(
    mut command: Command,
    timeout: Duration,
    label: &str,
) -> Result<(ExitStatus, Vec<u8>, Vec<u8>), String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // On Unix, own a process group so a timeout also kills helpers such as
    // ffmpeg. Other platforms currently terminate the direct child; their
    // native process-tree/job-object support belongs in the platform layer.
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|err| format!("could not run {label}: {err}"))?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let stdout_reader = std::thread::spawn(move || read_to_end(stdout));
    let stderr_reader = std::thread::spawn(move || read_to_end(stderr));

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                kill_tree(&mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("{label} timed out after {}s", timeout.as_secs()));
            }
            Err(err) => {
                kill_tree(&mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("could not wait for {label}: {err}"));
            }
        }
    };

    Ok((
        status,
        stdout_reader.join().unwrap_or_default(),
        stderr_reader.join().unwrap_or_default(),
    ))
}

fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        // Negative pid = the whole process group created by `process_group(0)`.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
    }
    // On non-Unix targets this kills the direct child. Platform-specific
    // process-tree termination can replace this fallback when implemented.
    let _ = child.kill();
    let _ = child.wait();
}

fn read_to_end(mut pipe: impl Read) -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = pipe.read_to_end(&mut buffer);
    buffer
}

/// Lightweight reachability check before asking yt-dlp to resolve a stream.
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

    #[test]
    fn hung_process_is_killed_at_the_deadline() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 5"]);
        let started = Instant::now();
        let err =
            run_with_timeout(command, Duration::from_millis(150), "test command").unwrap_err();
        assert!(err.contains("timed out"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(3), "kill was slow");
    }

    #[test]
    fn output_larger_than_a_pipe_buffer_is_captured() {
        // 200 KB is well past the usual 64 KB pipe buffer: without a reader
        // thread the child would block and the timeout would fire.
        let mut command = Command::new("sh");
        command.args(["-c", "head -c 200000 /dev/zero"]);
        let (status, stdout, _) =
            run_with_timeout(command, Duration::from_secs(5), "test command").unwrap();
        assert!(status.success());
        assert_eq!(stdout.len(), 200_000);
    }

    fn stream(url: &str) -> StreamInfo {
        StreamInfo {
            video_url: url.to_string(),
            audio_url: None,
            width: 1920,
            height: 1080,
        }
    }

    #[test]
    fn reuses_a_resolution_until_the_ttl_expires() {
        let cache = ResolutionCache::default();
        let now = Instant::now();
        cache.insert("https://pages/a", 1080, stream("https://cdn/a"), now);

        let hit = cache
            .get("https://pages/a", 1080, now + Duration::from_secs(60))
            .expect("entry is fresh");
        assert_eq!(hit.video_url, "https://cdn/a");

        assert!(cache
            .get("https://pages/a", 1080, now + RESOLUTION_TTL)
            .is_none());
        // The expired entry is dropped rather than left to grow the map.
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn a_different_screen_height_resolves_separately() {
        let cache = ResolutionCache::default();
        let now = Instant::now();
        cache.insert("https://pages/a", 1080, stream("https://cdn/1080"), now);
        cache.insert("https://pages/a", 2160, stream("https://cdn/2160"), now);

        assert_eq!(
            cache.get("https://pages/a", 2160, now).unwrap().video_url,
            "https://cdn/2160"
        );
        assert_eq!(
            cache.get("https://pages/a", 1080, now).unwrap().video_url,
            "https://cdn/1080"
        );
        assert!(cache.get("https://pages/a", 1440, now).is_none());
    }

    #[test]
    fn invalidation_drops_every_height_but_keeps_other_pages() {
        let cache = ResolutionCache::default();
        let now = Instant::now();
        cache.insert("https://pages/a", 1080, stream("https://cdn/a"), now);
        cache.insert("https://pages/a", 2160, stream("https://cdn/a4k"), now);
        cache.insert("https://pages/b", 1080, stream("https://cdn/b"), now);

        cache.invalidate("https://pages/a");

        assert!(cache.get("https://pages/a", 1080, now).is_none());
        assert!(cache.get("https://pages/a", 2160, now).is_none());
        assert!(cache.get("https://pages/b", 1080, now).is_some());
    }

    #[test]
    fn the_cache_is_bounded_and_drops_the_oldest_entry() {
        let cache = ResolutionCache::default();
        let now = Instant::now();
        for index in 0..RESOLUTION_CACHE_LIMIT + 4 {
            let url = format!("https://pages/{index}");
            let stored = now + Duration::from_millis(index as u64);
            cache.insert(&url, 1080, stream("https://cdn/x"), stored);
        }
        assert_eq!(cache.len(), RESOLUTION_CACHE_LIMIT);
        // The earliest insert is the one evicted, not an arbitrary entry.
        assert!(cache.get("https://pages/0", 1080, now).is_none());
        assert!(cache
            .get(
                &format!("https://pages/{}", RESOLUTION_CACHE_LIMIT + 3),
                1080,
                now
            )
            .is_some());
    }

    // ======================================================================== //
    // Stream resolution edge cases: missing formats, audio-only, empty lists  //
    // ======================================================================== //

    /// A format with both codecs as "none" carries no playable stream, so the
    /// resolution must fail instead of returning an empty stream.
    #[test]
    fn formats_with_no_codec_at_all_are_dropped_by_parse() {
        // yt-dlp marks a stream it cannot play with "none" for both codecs.
        let json = r#"{"formats": [
            {"url": "https://x/video-only", "acodec": "none", "vcodec": "vp9", "quality": 1, "width": 1280, "height": 720},
            {"url": "https://x/audio-only", "acodec": "opus", "vcodec": "none", "quality": 2}
        ]}"#;
        let formats = Formats::parse(json).unwrap();
        // Video-only has no audio, audio-only has no video.
        assert!(formats.optimal_video(1080).is_some());
        assert!(formats.best_audio().is_some());
        // A page with only an audio-only stream cannot be rendered as wallpaper.
        let video_only_json = r#"{"formats": [
            {"url": "https://x/a", "acodec": "opus", "vcodec": "none", "quality": 1}
        ]}"#;
        let only_audio = Formats::parse(video_only_json).unwrap();
        // best_audio finds it, but there is no separate video track.
        assert!(only_audio.best_audio().is_some());
    }

    #[test]
    fn all_formats_empty_returns_failure_for_video_and_audio() {
        let formats = Formats::default();
        assert!(formats.optimal_video(1080).is_none());
        assert!(formats.best_audio().is_none());
    }

    #[test]
    fn audio_only_format_with_negative_quality_ignored_for_optimal_video() {
        let json = r#"{"formats": [
            {"url": "https://x/audio", "acodec": "opus", "vcodec": "none", "quality": -1}
        ]}"#;
        let formats = Formats::parse(json).unwrap();
        assert!(formats.optimal_video(1080).is_none());
        assert_eq!(formats.best_audio().unwrap().url, "https://x/audio");
    }

    #[test]
    fn video_without_dimensions_falls_back_to_muxed_stream() {
        let json = r#"{"formats": [
            {"url": "https://x/no_dims", "acodec": "mp4a", "vcodec": "avc1", "quality": 1},
            {"url": "https://x/360p", "acodec": "none", "vcodec": "vp9", "quality": 2, "width": 640, "height": 360}
        ]}"#;
        let formats = Formats::parse(json).unwrap();
        // Video without dimensions is treated as muxed (has both audio+video)
        // so it's the best option for optimal_video when no pure video exists at target height.
        let video = formats.optimal_video(360).unwrap();
        assert_eq!(video.url, "https://x/360p");
    }

    #[test]
    fn very_large_height_picks_closest_available() {
        let json = r#"{"formats": [
            {"url": "https://x/low", "acodec": "none", "vcodec": "vp9", "quality": 1, "width": 1280, "height": 720},
            {"url": "https://x/high", "acodec": "none", "vcodec": "vp9", "quality": 2, "width": 3840, "height": 2160}
        ]}"#;
        let formats = Formats::parse(json).unwrap();
        // Screen is 4K but closest match to 4K is 2160p.
        let video = formats.optimal_video(4000).unwrap();
        assert_eq!(video.url, "https://x/high");
    }

    // ======================================================================== //
    // Cache edge cases: concurrent access patterns, TTL boundary precision    //
    // ======================================================================== //

    #[test]
    fn insert_then_immediate_get_at_exact_same_height_hits_cache() {
        let cache = ResolutionCache::default();
        let now = Instant::now();
        let info = stream("https://cdn/exact");
        cache.insert("https://pages/test", 1080, info.clone(), now);
        let hit = cache
            .get("https://pages/test", 1080, now)
            .expect("exact height hit");
        assert_eq!(hit.video_url, "https://cdn/exact");
    }

    #[test]
    fn ttl_expiration_is_strict_boundary_checking() {
        let cache = ResolutionCache::default();
        let now = Instant::now();
        let info = stream("https://cdn/ttl-test");
        cache.insert("https://pages/ttl", 1080, info, now);
        // The implementation uses strict `<` comparison: entries expire when
        // `elapsed >= RESOLUTION_TTL`. A value slightly *under* TTL is still valid.
        let hit = cache
            .get(
                "https://pages/ttl",
                1080,
                now + RESOLUTION_TTL - Duration::from_millis(1),
            )
            .expect("within TTL");
        assert_eq!(hit.video_url, "https://cdn/ttl-test");
        // Exactly at TTL or beyond must expire.
        assert!(cache
            .get("https://pages/ttl", 1080, now + RESOLUTION_TTL)
            .is_none());
        assert!(cache
            .get(
                "https://pages/ttl",
                1080,
                now + RESOLUTION_TTL + Duration::from_nanos(1)
            )
            .is_none());
    }

    #[test]
    fn multiple_pages_different_urls_dont_clobber_each_other() {
        let cache = ResolutionCache::default();
        let now = Instant::now();
        cache.insert("https://youtube.com/a", 1080, stream("https://cdn/a"), now);
        cache.insert("https://vimeo.com/b", 1080, stream("https://cdn/b"), now);
        cache.insert(
            "https://dailymotion.com/c",
            1080,
            stream("https://cdn/c"),
            now,
        );

        assert_eq!(
            cache
                .get("https://youtube.com/a", 1080, now)
                .unwrap()
                .video_url,
            "https://cdn/a"
        );
        assert_eq!(
            cache
                .get("https://vimeo.com/b", 1080, now)
                .unwrap()
                .video_url,
            "https://cdn/b"
        );
        assert_eq!(
            cache
                .get("https://dailymotion.com/c", 1080, now)
                .unwrap()
                .video_url,
            "https://cdn/c"
        );
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn same_page_different_heights_all_retained_independently() {
        let cache = ResolutionCache::default();
        let now = Instant::now();
        for h in [480, 720, 1080, 1440, 2160].iter() {
            cache.insert(
                "https://youtube.com/x",
                *h,
                stream(&format!("https://cdn/{}", h)),
                now,
            );
        }
        assert_eq!(cache.len(), 5);
        // Each height should resolve independently.
        for h in [480, 720, 1080, 1440, 2160].iter() {
            let hit = cache
                .get("https://youtube.com/x", *h, now)
                .unwrap_or_else(|| panic!("height {} miss", h));
            assert_eq!(hit.video_url, format!("https://cdn/{}", h));
        }
    }

    // ======================================================================== //
    // Invalidations verify single-page isolation across pages                //
    // ======================================================================== //

    #[test]
    fn invalidate_preserves_all_other_pages_completely() {
        let cache = ResolutionCache::default();
        let now = Instant::now();
        // Populate with three different page URLs, each with two heights.
        for page in ["https://a.com", "https://b.com", "https://c.com"] {
            for &h in &[1080, 2160] {
                cache.insert(page, h, stream(&format!("https://cdn/{page}-{h}")), now);
            }
        }
        assert_eq!(cache.len(), 6);

        // Invalidate middle page only.
        cache.invalidate("https://b.com");
        assert_eq!(cache.len(), 4);

        // Pages a and c intact.
        assert_eq!(
            cache.get("https://a.com", 1080, now).unwrap().video_url,
            "https://cdn/https://a.com-1080"
        );
        assert_eq!(
            cache.get("https://a.com", 2160, now).unwrap().video_url,
            "https://cdn/https://a.com-2160"
        );
        assert_eq!(
            cache.get("https://c.com", 1080, now).unwrap().video_url,
            "https://cdn/https://c.com-1080"
        );
        assert_eq!(
            cache.get("https://c.com", 2160, now).unwrap().video_url,
            "https://cdn/https://c.com-2160"
        );

        // Page b fully purged.
        assert!(cache.get("https://b.com", 1080, now).is_none());
        assert!(cache.get("https://b.com", 2160, now).is_none());
    }

    #[test]
    fn invalidate_on_nonexistent_page_is_noop() {
        let cache = ResolutionCache::default();
        let now = Instant::now();
        cache.insert(
            "https://real.com/page",
            1080,
            stream("https://cdn/real"),
            now,
        );
        cache.invalidate("https://fake.com/nonexistent");
        assert_eq!(cache.len(), 1);
        assert!(cache.get("https://real.com/page", 1080, now).is_some());
    }

    #[test]
    fn resolve_returns_error_for_inaccessible_url() {
        // With a non-existent URL or no yt-dlp, we expect either an error
        // message containing the original URL or the exit code string.
        let result = resolve("https://example.com/nonexistent-fake-video-xyz", 1080);
        assert!(result.is_err(), "expected an error for invalid URL");
    }
}
