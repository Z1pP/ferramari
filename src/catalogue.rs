//! Client for the remote live-wallpaper catalogue.
//!
//! Listing pages contain everything needed to show a card (title, category,
//! preview, post id), and `/download/id<post>` returns an HTML page with a
//! short-lived signed link to a ZIP that holds one video. Nothing here talks
//! to the GUI; the online page drives these functions from worker threads.
//!
//! Network access is deliberately conservative: a browser User-Agent, hard
//! size limits, no cookies and no crawling beyond what the user asks for.

use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;

use crate::paths;
use crate::util;

pub const SITE: &str = "https://winzoro.net";
pub const SOURCE_URL: &str = "https://winzoro.net/wallpapers_all/";
pub const CANCELLED: &str = "cancelled";

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/130.0 Safari/537.36";
const HTML_LIMIT: u64 = 5 * 1024 * 1024;
const PREVIEW_LIMIT: u64 = 8 * 1024 * 1024;
const ZIP_LIMIT: u64 = 512 * 1024 * 1024;
const VIDEO_LIMIT: u64 = 1024 * 1024 * 1024;
const PAGE_CACHE_AGE: Duration = Duration::from_secs(5 * 60);
const CACHE_VIDEO_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const CACHE_VIDEO_TOTAL: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Category {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wallpaper {
    pub id: String,
    pub title: String,
    pub category: String,
    pub thumb_url: String,
    pub preview_url: String,
}

#[derive(Debug, Clone, Default)]
pub struct Listing {
    pub items: Vec<Wallpaper>,
    pub categories: Vec<Category>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DownloadInfo {
    pub url: String,
    pub file_name: String,
    pub size_text: String,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Probe {
    pub width: u32,
    pub height: u32,
    pub fps: Option<f64>,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Downloaded {
    pub video: PathBuf,
    /// Original name inside the archive, e.g. `ruins.mp4`.
    pub original_name: String,
}

/// Build a listing URL. `category` is the absolute category URL from the
/// catalogue; `None` means the whole "live wallpapers" section.
pub fn listing_url(category: Option<&str>, page: u32) -> String {
    let base = category.unwrap_or(SOURCE_URL).trim_end_matches('/');
    if page <= 1 {
        format!("{base}/")
    } else {
        format!("{base}/page/{page}/")
    }
}

pub fn download_page_url(id: &str) -> String {
    format!("{SITE}/download/id{id}")
}

pub fn cache_dir() -> PathBuf {
    paths::catalogue_cache_dir()
}

fn video_cache_dir() -> PathBuf {
    cache_dir().join("videos")
}

fn preview_cache_dir() -> PathBuf {
    cache_dir().join("previews")
}

fn page_cache_dir() -> PathBuf {
    cache_dir().join("pages")
}

fn sheet_cache_dir() -> PathBuf {
    cache_dir().join("sheets")
}

/// Fetch and parse one listing page (whole catalogue or a category).
pub fn load_listing(url: &str) -> Result<Listing> {
    let html = fetch_page_cached(url)?;
    Ok(parse_listing(&html))
}

/// Search the site. Results include themes, cursors and other content, so
/// only cards whose category is a live wallpaper are kept.
pub fn search(query: &str) -> Result<Listing> {
    let agent = agent(Duration::from_secs(30));
    let response = agent
        .post(&format!("{SITE}/?do=search"))
        .set("Accept-Language", "ru,en;q=0.9")
        .set(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .send_form(&[
            ("do", "search"),
            ("subaction", "search"),
            ("story", query.trim()),
        ])
        .map_err(http_error)?;
    let html = read_text(response, HTML_LIMIT)?;
    let mut listing = parse_listing(&html);
    listing
        .items
        .retain(|item| item.category.starts_with("Живые обои"));
    // The navigation is independent of the search; the page also offers a
    // generic "next" link when there are many results.
    Ok(listing)
}

fn fetch_page_cached(url: &str) -> Result<String> {
    let path = page_cache_path(url);
    if let Some(html) = fresh_cache_file(&path, PAGE_CACHE_AGE).and_then(read_text_file) {
        return Ok(html);
    }
    let agent = agent(Duration::from_secs(30));
    let response = agent
        .get(url)
        .set("Accept-Language", "ru,en;q=0.9")
        .set(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .call()
        .map_err(http_error)?;
    let html = read_text(response, HTML_LIMIT)?;
    let _ = write_cache(&path, html.as_bytes());
    Ok(html)
}

/// Parse a listing/search page. Cards are returned in document order.
pub fn parse_listing(html: &str) -> Listing {
    Listing {
        items: parse_cards(html),
        categories: parse_categories(html),
        next: parse_next(html),
    }
}

/// Parse the download interstitial page and return its signed ZIP link.
pub fn parse_download(html: &str) -> Result<DownloadInfo> {
    static SIGNED: OnceLock<Regex> = OnceLock::new();
    static FILE: OnceLock<Regex> = OnceLock::new();
    let signed = SIGNED.get_or_init(|| {
        Regex::new(r"window\.location\s*=\s*'([^']+)'").expect("signed URL pattern")
    });
    let file = FILE.get_or_init(|| {
        Regex::new(r#"(?s)<div class="dpbName[^"]*">\s*<strong>(.*?)</strong>"#)
            .expect("download name pattern")
    });

    let url = signed
        .captures(html)
        .map(|captures| decode_entities(&captures[1]))
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
        .ok_or_else(|| anyhow!("the catalogue did not return a download link"))?;
    let file_name = file
        .captures(html)
        .map(|captures| clean_text(&captures[1]))
        .unwrap_or_default();
    let size_text = parse_size_text(html).unwrap_or_default();
    let size = parse_size(&size_text);
    Ok(DownloadInfo {
        url,
        file_name,
        size_text,
        size,
    })
}

pub fn fetch_download_info(id: &str) -> Result<DownloadInfo> {
    let html = fetch_html_uncached(&download_page_url(id))?;
    parse_download(&html)
}

fn fetch_html_uncached(url: &str) -> Result<String> {
    let agent = agent(Duration::from_secs(30));
    let response = agent
        .get(url)
        .set("Accept-Language", "ru,en;q=0.9")
        .set(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .call()
        .map_err(http_error)?;
    read_text(response, HTML_LIMIT)
}

fn agent(read_timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(USER_AGENT)
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(read_timeout)
        .timeout_write(Duration::from_secs(30))
        .build()
}

fn http_error(err: ureq::Error) -> anyhow::Error {
    match err {
        ureq::Error::Status(code, response) => {
            let mut body = String::new();
            let _ = response
                .into_reader()
                .take(64 * 1024)
                .read_to_string(&mut body);
            match page_error_message(&body) {
                Some(message) => anyhow!("{message} (HTTP {code})"),
                None => anyhow!("the wallpaper server returned HTTP {code}"),
            }
        }
        other => anyhow!("network error: {other}"),
    }
}

/// Site errors such as the per-IP download limit arrive as an HTML page with
/// a descriptive `<title>`; XML errors use `<Message>`. Show that text instead
/// of a bare status code.
fn page_error_message(html: &str) -> Option<String> {
    static TITLE: OnceLock<Regex> = OnceLock::new();
    static MESSAGE: OnceLock<Regex> = OnceLock::new();
    let title = TITLE.get_or_init(|| Regex::new(r"(?is)<title>(.*?)</title>").unwrap());
    let message = MESSAGE.get_or_init(|| Regex::new(r"(?is)<Message>(.*?)</Message>").unwrap());
    [title, message]
        .iter()
        .filter_map(|pattern| pattern.captures(html))
        .map(|captures| {
            let text = clean_text(&strip_tags(&captures[1]));
            text.trim_end_matches(" (ID:)").trim().to_string()
        })
        .find(|text| !text.is_empty())
}

fn read_text(response: ureq::Response, limit: u64) -> Result<String> {
    let bytes = read_limited(response.into_reader(), limit)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn read_limited(mut reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        if buffer.len() as u64 + read as u64 > limit {
            bail!("the response is larger than {} MB", limit / (1024 * 1024));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Ok(buffer)
}

fn parse_cards(html: &str) -> Vec<Wallpaper> {
    static HREF: OnceLock<Regex> = OnceLock::new();
    static IMG: OnceLock<Regex> = OnceLock::new();
    static CATEGORY: OnceLock<Regex> = OnceLock::new();
    static TITLE: OnceLock<Regex> = OnceLock::new();
    static RATE: OnceLock<Regex> = OnceLock::new();

    let href = HREF.get_or_init(|| Regex::new(r#"(?s)<a\s+href="([^"]+)""#).unwrap());
    let img = IMG.get_or_init(|| Regex::new(r#"(?s)<img\s+src="([^"]+)""#).unwrap());
    let category = CATEGORY
        .get_or_init(|| Regex::new(r#"(?s)fiveShortyCat[^>]*>\s*<span>(.*?)</span>"#).unwrap());
    let title =
        TITLE.get_or_init(|| Regex::new(r#"(?s)fiveShortyTitle[^>]*>(.*?)</div>"#).unwrap());
    let rate = RATE.get_or_init(|| Regex::new(r"doRate\(\s*'plus'\s*,\s*'(\d+)'").unwrap());

    let mut cards = Vec::new();
    for chunk in html.split(r#"class="fiveShort short"#).skip(1) {
        let page_url = href.captures(chunk).map(|captures| captures[1].to_string());
        let id = rate.captures(chunk).map(|captures| captures[1].to_string());
        let (Some(_), Some(id)) = (page_url, id) else {
            continue;
        };
        let title = title
            .captures(chunk)
            .map(|captures| clean_text(&captures[1]))
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "Live wallpaper".to_string());
        let category = category
            .captures(chunk)
            .map(|captures| clean_text(&captures[1]))
            .unwrap_or_default();
        let thumb_url = img
            .captures(chunk)
            .map(|captures| absolute_url(&captures[1]))
            .unwrap_or_default();
        cards.push(Wallpaper {
            id,
            title,
            category,
            preview_url: large_preview(&thumb_url),
            thumb_url,
        });
    }
    cards
}

fn parse_categories(html: &str) -> Vec<Category> {
    static CATEGORY: OnceLock<Regex> = OnceLock::new();
    let pattern = CATEGORY.get_or_init(|| {
        Regex::new(r#"(?s)<a href="([^"]+)"[^>]*data-category-id="(\d+)"[^>]*>(.*?)</a>"#).unwrap()
    });

    let mut categories: Vec<Category> = Vec::new();
    for captures in pattern.captures_iter(html) {
        let url = absolute_url(&captures[1]);
        let id = &captures[2];
        let name = clean_text(&strip_tags(&captures[3]));
        if !url.contains("/wallpapers_all/")
            || url.trim_end_matches('/') == SOURCE_URL.trim_end_matches('/')
            || matches!(id, "40" | "42" | "158")
            || name.is_empty()
        {
            continue;
        }
        if categories.iter().any(|category| category.url == url) {
            continue;
        }
        categories.push(Category { name, url });
    }
    categories
}

fn parse_next(html: &str) -> Option<String> {
    static NEXT: OnceLock<Regex> = OnceLock::new();
    let next =
        NEXT.get_or_init(|| Regex::new(r#"(?s)class="showMore[^"]*".*?href="([^"]+)""#).unwrap());
    next.captures(html)
        .map(|captures| absolute_url(&captures[1]))
        .filter(|url| url.starts_with("https://winzoro.net/"))
}

fn parse_size_text(html: &str) -> Option<String> {
    static SIZE: OnceLock<Regex> = OnceLock::new();
    let size = SIZE.get_or_init(|| {
        Regex::new("(?s)<div class=\"dpbName[^\"]*\">.*?<span>([^<]+)</span>").unwrap()
    });
    size.captures(html).map(|captures| clean_text(&captures[1]))
}

fn parse_size(text: &str) -> Option<u64> {
    let mut parts = text.split_whitespace();
    let number: f64 = parts.next()?.replace(',', ".").parse().ok()?;
    let unit = parts.next().unwrap_or("b").to_lowercase();
    let multiplier = if unit.starts_with("gb") || unit.starts_with("гб") {
        1024.0 * 1024.0 * 1024.0
    } else if unit.starts_with("mb") || unit.starts_with("мб") {
        1024.0 * 1024.0
    } else if unit.starts_with("kb") || unit.starts_with("кб") {
        1024.0
    } else {
        1.0
    };
    Some((number * multiplier) as u64)
}

fn absolute_url(url: &str) -> String {
    let url = decode_entities(url.trim());
    if url.starts_with("http://") || url.starts_with("https://") {
        url
    } else if let Some(rest) = url.strip_prefix("//") {
        format!("https://{rest}")
    } else if url.starts_with('/') {
        format!("{SITE}{url}")
    } else {
        format!("{SITE}/{url}")
    }
}

/// The full-size still is the thumbnail path without its `thumbs` segment.
fn large_preview(thumb: &str) -> String {
    if thumb.contains("/thumbs/") {
        thumb.replacen("/thumbs/", "/", 1)
    } else {
        thumb.to_string()
    }
}

/// Remove markup so link labels that wrap their text in an icon and a
/// `<span>` do not leak HTML into the UI.
fn strip_tags(html: &str) -> String {
    let mut output = String::with_capacity(html.len());
    let mut inside = false;
    for character in html.chars() {
        match character {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => output.push(character),
            _ => {}
        }
    }
    output
}

fn clean_text(html: &str) -> String {
    let text = decode_entities(html);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(index) = rest.find('&') {
        output.push_str(&rest[..index]);
        let after = &rest[index..];
        let entity = after.find(';').filter(|end| *end <= 12).and_then(|end| {
            let name = &after[1..end];
            named_entity(name)
                .or_else(|| numeric_entity(name))
                .map(|character| (character, end))
        });
        match entity {
            Some((character, end)) => {
                output.push(character);
                rest = &after[end + 1..];
            }
            None => {
                output.push('&');
                rest = &after[1..];
            }
        }
    }
    output.push_str(rest);
    output
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" | "#039" => '\'',
        "nbsp" => ' ',
        "laquo" => '«',
        "raquo" => '»',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "copy" => '©',
        "middot" => '·',
        "bull" => '•',
        "rsquo" => '’',
        "lsquo" => '‘',
        "ldquo" => '“',
        "rdquo" => '”',
        _ => return None,
    })
}

fn numeric_entity(name: &str) -> Option<char> {
    let value = name.strip_prefix('#')?;
    let code = if let Some(hex) = value.strip_prefix(['x', 'X']) {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        value.parse().ok()?
    };
    char::from_u32(code)
}

fn hash_key(value: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn page_cache_path(url: &str) -> PathBuf {
    page_cache_dir().join(format!("{}.html", hash_key(url)))
}

fn preview_cache_path(url: &str) -> PathBuf {
    let extension = url
        .split('?')
        .next()
        .and_then(|path| Path::new(path).extension())
        .and_then(|extension| extension.to_str())
        .filter(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif"
            )
        })
        .unwrap_or("img");
    preview_cache_dir().join(format!("{}.{}", hash_key(url), extension))
}

fn fresh_cache_file(path: &Path, max_age: Duration) -> Option<PathBuf> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    (age < max_age).then(|| path.to_path_buf())
}

fn read_text_file(path: PathBuf) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() as u64 > HTML_LIMIT {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn write_cache(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("cache path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!(
        "part-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

/// Download the thumbnail for `url`, using the on-disk cache when possible.
pub fn ensure_preview(url: &str) -> Result<PathBuf> {
    let path = preview_cache_path(url);
    if fs::metadata(&path)
        .map(|metadata| metadata.len() > 0)
        .unwrap_or(false)
    {
        return Ok(path);
    }
    let agent = agent(Duration::from_secs(30));
    let response = agent
        .get(url)
        .set(
            "Accept",
            "image/avif,image/webp,image/png,image/*;q=0.8,*/*;q=0.5",
        )
        .call()
        .map_err(http_error)?;
    let content_type = response
        .header("Content-Type")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !content_type.starts_with("image/") {
        bail!("the server did not return an image");
    }
    let bytes = read_limited(response.into_reader(), PREVIEW_LIMIT)?;
    if bytes.is_empty() {
        bail!("empty preview image");
    }
    write_cache(&path, &bytes)?;
    Ok(path)
}

pub fn cached_video(id: &str) -> Option<PathBuf> {
    video_files()
        .into_iter()
        .find(|path| path.file_stem().and_then(|stem| stem.to_str()) == Some(id))
}

fn video_files() -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(video_cache_dir()) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(is_video_extension)
        })
        .collect();
    files.sort();
    files
}

pub fn cached_original_name(id: &str) -> Option<String> {
    let path = video_cache_dir().join(format!("{id}.name"));
    let name = fs::read_to_string(path).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn sheet_path(id: &str) -> PathBuf {
    sheet_cache_dir().join(format!("{id}.png"))
}

pub fn cached_sheet(id: &str) -> Option<PathBuf> {
    let path = sheet_path(id);
    fs::metadata(&path)
        .ok()
        .filter(|metadata| metadata.len() > 0)
        .map(|_| path)
}

/// Download and unpack a wallpaper. The archive is removed afterwards; the
/// extracted video and its original name stay in the cache for the preview
/// dialog and repeated installs.
pub fn download_video(
    id: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<Downloaded> {
    if let Some(video) = cached_video(id) {
        return Ok(Downloaded {
            video,
            original_name: cached_original_name(id)
                .unwrap_or_else(|| format!("wallpaper-{id}.mp4")),
        });
    }
    fs::create_dir_all(video_cache_dir())?;

    let mut last_error = None;
    for attempt in 0..2 {
        if cancel.load(Ordering::Relaxed) {
            bail!(CANCELLED);
        }
        // The signed link lives for ten minutes; when it expired (HTTP 403)
        // the interstitial page has to be fetched again.
        let info = fetch_download_info(id)?;
        let zip = video_cache_dir().join(format!("{id}.zip"));
        match download_zip(&info, &zip, cancel, progress) {
            Ok(()) => {}
            Err(err) => {
                if cancel.load(Ordering::Relaxed) || err.to_string().contains(CANCELLED) {
                    bail!(CANCELLED);
                }
                last_error = Some(err);
                if attempt == 0 {
                    continue;
                }
                break;
            }
        }

        let extracted = extract_video(&zip, &video_cache_dir(), id, VIDEO_LIMIT);
        let _ = fs::remove_file(&zip);
        match extracted {
            Ok((video, original_name)) => {
                let _ = fs::write(video_cache_dir().join(format!("{id}.name")), &original_name);
                return Ok(Downloaded {
                    video,
                    original_name,
                });
            }
            Err(err) => {
                last_error = Some(err);
                if attempt == 0 {
                    continue;
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("could not download the wallpaper")))
}

fn download_zip(
    info: &DownloadInfo,
    target: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!(CANCELLED);
    }
    let agent = agent(Duration::from_secs(60));
    let response = agent
        .get(&info.url)
        .set("Accept", "application/zip,application/octet-stream,*/*")
        .call()
        .map_err(http_error)?;
    let total = response
        .header("Content-Length")
        .and_then(|length| length.parse::<u64>().ok())
        .filter(|length| *length > 0);
    if total.is_some_and(|total| total > ZIP_LIMIT) {
        bail!("the archive is larger than 512 MB");
    }

    let parent = target
        .parent()
        .ok_or_else(|| anyhow!("download target has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = target.with_extension("part");
    let result = (|| -> Result<()> {
        let mut file = fs::File::create(&temporary)?;
        let mut reader = response.into_reader();
        let mut buffer = [0u8; 64 * 1024];
        let mut done = 0u64;
        progress(0, total);
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!(CANCELLED);
            }
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            done += read as u64;
            if done > ZIP_LIMIT {
                bail!("the archive is larger than 512 MB");
            }
            file.write_all(&buffer[..read])?;
            progress(done, total);
        }
        file.flush()?;
        if done == 0 {
            bail!("the server returned an empty archive");
        }
        fs::rename(&temporary, target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Extract the first video entry. The archive's path is never used: the file
/// is written under our own `<id>.<ext>` name, which also makes zip-slip
/// impossible.
fn extract_video(
    zip: &Path,
    directory: &Path,
    id: &str,
    size_limit: u64,
) -> Result<(PathBuf, String)> {
    let file = fs::File::open(zip).with_context(|| format!("open {}", zip.display()))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .with_context(|| format!("read {}", zip.display()))?;
    fs::create_dir_all(directory)?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry
            .name()
            .map_err(|err| anyhow!("invalid archive entry name: {err}"))?
            .to_string();
        let extension = Path::new(&name)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .unwrap_or_default();
        if !is_video_extension(&extension) {
            continue;
        }
        if entry.encrypted() {
            bail!("the archive is encrypted");
        }
        if entry.size() > size_limit {
            bail!("the video is larger than {} MB", size_limit / (1024 * 1024));
        }
        let destination = directory.join(format!("{id}.{extension}"));
        let temporary = destination.with_extension(format!("{extension}.part"));
        let result = (|| -> Result<()> {
            let mut output = fs::File::create(&temporary)?;
            let copied = std::io::copy(&mut (&mut entry).take(size_limit + 1), &mut output)?;
            output.flush()?;
            if copied > size_limit {
                bail!("the video is larger than {} MB", size_limit / (1024 * 1024));
            }
            if copied == 0 {
                bail!("the archive contains an empty video");
            }
            fs::rename(&temporary, &destination)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        return Ok((destination, name));
    }
    bail!("the archive does not contain a video")
}

fn is_video_extension(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "mp4" | "webm" | "mkv" | "mov" | "m4v"
    )
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const SHEET_TIMEOUT: Duration = Duration::from_secs(60);

/// Read video metadata with `ffprobe`. Failures are not fatal: the download
/// is still usable without the extra line of information.
pub fn probe(video: &Path) -> Option<Probe> {
    let mut child = Command::new(paths::tool("ffprobe"))
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate:format=duration",
            "-of",
            "json",
        ])
        .arg(video)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let status = util::wait_with_timeout(&mut child, PROBE_TIMEOUT)?;
    if !status.success() {
        return None;
    }
    let mut stdout = Vec::new();
    child.stdout.take()?.read_to_end(&mut stdout).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&stdout).ok()?;
    let stream = json.get("streams")?.get(0)?;
    let width = stream.get("width")?.as_u64()? as u32;
    let height = stream.get("height")?.as_u64()? as u32;
    if width == 0 || height == 0 {
        return None;
    }
    let fps = stream
        .get("r_frame_rate")
        .and_then(|value| value.as_str())
        .and_then(parse_fraction);
    let duration = json
        .get("format")
        .and_then(|format| format.get("duration"))
        .and_then(|value| value.as_str())
        .and_then(|value| value.parse().ok());
    Some(Probe {
        width,
        height,
        fps,
        duration,
    })
}

fn parse_fraction(value: &str) -> Option<f64> {
    let (numerator, denominator) = value.split_once('/')?;
    let numerator: f64 = numerator.parse().ok()?;
    let denominator: f64 = denominator.parse().ok()?;
    (denominator > 0.0).then(|| numerator / denominator)
}

/// Build a 3×2 contact sheet with six frames from the video. Falls back to a
/// single large frame when the tile filter cannot run.
pub fn ensure_sheet(video: &Path, id: &str) -> Option<PathBuf> {
    if let Some(sheet) = cached_sheet(id) {
        return Some(sheet);
    }
    let target = sheet_path(id);
    fs::create_dir_all(sheet_cache_dir()).ok()?;
    let temporary = target.with_extension("part.png");
    let duration = probe(video).and_then(|probe| probe.duration);

    let made = duration
        .filter(|duration| *duration > 0.05)
        .is_some_and(|duration| {
            let filter = format!(
                "fps=6/{duration:.6},scale=480:270:force_original_aspect_ratio=increase,\
                 crop=480:270,tile=3x2"
            );
            run_ffmpeg(video, &temporary, &filter)
        })
        || run_ffmpeg(
            video,
            &temporary,
            "thumbnail,scale=1440:810:force_original_aspect_ratio=increase,crop=1440:810",
        );
    if !made {
        let _ = fs::remove_file(&temporary);
        return None;
    }
    if fs::rename(&temporary, &target).is_err() {
        let _ = fs::remove_file(&temporary);
        return None;
    }
    Some(target)
}

fn run_ffmpeg(video: &Path, output: &Path, filter: &str) -> bool {
    let Ok(mut child) = Command::new(paths::tool("ffmpeg"))
        .args(["-y", "-v", "error", "-i"])
        .arg(video)
        .args(["-vf", filter, "-frames:v", "1"])
        .arg(output)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    util::wait_with_timeout(&mut child, SHEET_TIMEOUT).is_some_and(|status| status.success())
}

/// File name to use when copying a cached video into the local library.
pub fn library_name(original_name: &str, id: &str, extension: &str) -> String {
    let stem = Path::new(original_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    let mut clean = String::new();
    let mut last_dash = false;
    for character in stem.chars() {
        if character.is_alphanumeric() {
            clean.push(character);
            last_dash = false;
        } else if !last_dash && matches!(character, ' ' | '-' | '_' | '.' | '(' | ')' | '[' | ']') {
            clean.push('-');
            last_dash = true;
        }
        if clean.chars().count() >= 96 {
            break;
        }
    }
    let clean = clean.trim_matches('-').trim_matches('.');
    let clean = if clean.is_empty() {
        format!("wallpaper-{id}")
    } else {
        clean.to_string()
    };
    format!("{clean}.{extension}")
}

/// Copy a cached video into the wallpaper folder. An existing file with the
/// same name is reused instead of overwritten.
pub fn install(video: &Path, name: &str) -> Result<PathBuf> {
    let directory = paths::video_wallpaper_dir();
    fs::create_dir_all(&directory)?;
    let target = directory.join(name);
    if target.is_file() {
        return Ok(target);
    }
    let temporary = target.with_extension(format!(
        "{}.part",
        target
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("mp4")
    ));
    let result = (|| -> Result<()> {
        fs::copy(video, &temporary).with_context(|| format!("copy {}", video.display()))?;
        fs::rename(&temporary, &target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(target)
}

/// Remove a downloaded video and its contact sheet from the cache. Installed
/// files in the wallpaper folder are never touched.
pub fn clear_cached(id: &str) {
    if let Some(video) = cached_video(id) {
        let _ = fs::remove_file(video);
    }
    let _ = fs::remove_file(sheet_path(id));
    let _ = fs::remove_file(video_cache_dir().join(format!("{id}.name")));
}

/// Drop old page/preview entries and keep the video cache under 2 GB.
pub fn cleanup_cache() {
    let now = SystemTime::now();
    let too_old = |path: &Path, age: Duration| {
        fs::metadata(path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|elapsed| elapsed > age)
    };

    for directory in [page_cache_dir(), preview_cache_dir()] {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let age = if directory.ends_with("pages") {
                Duration::from_secs(24 * 60 * 60)
            } else {
                Duration::from_secs(30 * 24 * 60 * 60)
            };
            if too_old(&path, age) {
                let _ = fs::remove_file(path);
            }
        }
    }

    let videos_dir = video_cache_dir();
    let Ok(entries) = fs::read_dir(&videos_dir) else {
        return;
    };
    let mut videos = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_partial = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.ends_with("part"));
        if too_old(
            &path,
            if is_partial {
                Duration::from_secs(3600)
            } else {
                CACHE_VIDEO_AGE
            },
        ) {
            let _ = fs::remove_file(&path);
            continue;
        }
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(is_video_extension)
        {
            if let Ok(metadata) = fs::metadata(&path) {
                videos.push((
                    path,
                    metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    metadata.len(),
                ));
            }
        }
    }

    let total: u64 = videos.iter().map(|(_, _, size)| size).sum();
    if total > CACHE_VIDEO_TOTAL {
        videos.sort_by_key(|(_, modified, _)| *modified);
        let mut remaining = total;
        for (path, _, size) in videos {
            if remaining <= CACHE_VIDEO_TOTAL {
                break;
            }
            let id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_string();
            let _ = fs::remove_file(&path);
            let _ = fs::remove_file(sheet_path(&id));
            let _ = fs::remove_file(videos_dir.join(format!("{id}.name")));
            remaining = remaining.saturating_sub(size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const LISTING: &str = r##"
        <a href="https://winzoro.net/wallpapers_all/" data-category-id="40">Живые обои</a>
        <a href="https://winzoro.net/wallpapers_all/zhivye-oboi-anime/" data-category-id="153"><i class="/templates/wnz/dleimages/no_icon.gif"></i> <span>Аниме &bull; Манга</span></a>
        <a href="https://winzoro.net/themes/" data-category-id="2">Темы Windows</a>
        <div class="w33 left inline"><div class="fiveShort short block hidden br2">
        <div class="fiveShortyHeader sHeader">
        <a href="https://winzoro.net/2026/10/29/ruins.html">
        <img src="/uploads/posts/2026-10/thumbs/ruins_preview.png" alt="Ruins &amp; lake" />
        </a>
        <div class="fiveShortyCat"><span>Живые обои - Игры</span></div>
        <div class="fiveShortyTitle">Руины &laquo;Обливион&raquo;</div>
        </div>
        <div class="fiveShortyFooter"><a href="#" onclick="doRate('plus', '13514'); return false;">like</a></div>
        </div>
        <div class="w33 left inline"><div class="fiveShort short block hidden br2">
        <div class="fiveShortyHeader sHeader">
        <a href="/2026/10/29/witch.html"><img src="/uploads/posts/2026-10/thumbs/witch.png"></a>
        <div class="fiveShortyCat"><span>Живые обои - Аниме &bull; Манга</span></div>
        <div class="fiveShortyTitle">Ведьма</div>
        </div>
        <div class="fiveShortyFooter" onclick="doRate('plus', '13492')">x</div>
        </div>
        <div class="showMore block clear">
        <a href="https://winzoro.net/wallpapers_all/page/2/" class="btn smBtn fiveShowAll">Показать еще</a>
        </div>
    "##;

    #[test]
    fn parses_cards_categories_and_next_page() {
        let listing = parse_listing(LISTING);
        assert_eq!(listing.items.len(), 2);
        assert_eq!(listing.categories.len(), 1);
        assert_eq!(listing.categories[0].name, "Аниме • Манга");
        assert_eq!(
            listing.categories[0].url,
            "https://winzoro.net/wallpapers_all/zhivye-oboi-anime/"
        );
        assert_eq!(
            listing.next.as_deref(),
            Some("https://winzoro.net/wallpapers_all/page/2/")
        );

        let first = &listing.items[0];
        assert_eq!(first.id, "13514");
        assert_eq!(first.title, "Руины «Обливион»");
        assert_eq!(first.category, "Живые обои - Игры");
        assert_eq!(
            first.thumb_url,
            "https://winzoro.net/uploads/posts/2026-10/thumbs/ruins_preview.png"
        );
        assert_eq!(
            first.preview_url,
            "https://winzoro.net/uploads/posts/2026-10/ruins_preview.png"
        );
    }

    #[test]
    fn extracts_site_error_messages() {
        let limit = r#"<html><head><title>Превышен лимит на скачивание файлов! (ID:)</title></head></html>"#;
        assert_eq!(
            page_error_message(limit).as_deref(),
            Some("Превышен лимит на скачивание файлов!")
        );
        let s3 = "<Error><Code>AccessDenied</Code><Message>Request has expired</Message></Error>";
        assert_eq!(
            page_error_message(s3).as_deref(),
            Some("Request has expired")
        );
        assert!(page_error_message("<html>ok</html>").is_none());
    }

    #[test]
    fn external_commands_are_killed_at_the_deadline() {
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        let started = Instant::now();
        assert!(util::wait_with_timeout(&mut child, Duration::from_millis(100)).is_none());
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn short_external_commands_finish_normally() {
        let mut child = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
        assert!(util::wait_with_timeout(&mut child, Duration::from_secs(2))
            .is_some_and(|status| status.success()));
    }

    #[test]
    fn builds_listing_and_download_urls() {
        assert_eq!(listing_url(None, 1), "https://winzoro.net/wallpapers_all/");
        assert_eq!(
            listing_url(None, 3),
            "https://winzoro.net/wallpapers_all/page/3/"
        );
        assert_eq!(
            listing_url(
                Some("https://winzoro.net/wallpapers_all/zhivye-oboi-anime/"),
                2
            ),
            "https://winzoro.net/wallpapers_all/zhivye-oboi-anime/page/2/"
        );
        assert_eq!(
            download_page_url("13514"),
            "https://winzoro.net/download/id13514"
        );
    }

    #[test]
    fn parses_the_signed_download_link() {
        let html = r#"
            <div class="page-download download-btn"
                 onclick="window.location='https://winzoro.s3.regru.cloud/files/a.zip?X-Amz-Expires=600&amp;X-Amz-Signature=abc'">
                 <span>Скачать файл</span></div>
            <div class="dpbName inline">
                <strong>ruins &amp; lake.zip</strong>
                <span>11.4 Mb / 2 скачиваний / MD5: abc</span>
            </div>
        "#;
        let info = parse_download(html).unwrap();
        assert_eq!(
            info.url,
            "https://winzoro.s3.regru.cloud/files/a.zip?X-Amz-Expires=600&X-Amz-Signature=abc"
        );
        assert_eq!(info.file_name, "ruins & lake.zip");
        assert_eq!(info.size_text, "11.4 Mb / 2 скачиваний / MD5: abc");
        assert_eq!(info.size, Some((11.4 * 1024.0 * 1024.0) as u64));
    }

    #[test]
    fn a_page_without_a_link_is_an_error() {
        assert!(parse_download("<html>nothing</html>").is_err());
    }

    #[test]
    fn decodes_named_and_numeric_entities() {
        assert_eq!(
            decode_entities("a &amp; b &laquo;c&raquo; &#039;d&#x27;"),
            "a & b «c» 'd'"
        );
        assert_eq!(decode_entities("100% &unknown;"), "100% &unknown;");
    }

    #[test]
    fn library_names_are_safe() {
        assert_eq!(
            library_name("../../ayleid ruins (4K).zip", "1", "mp4"),
            "ayleid-ruins-4K.mp4"
        );
        assert_eq!(library_name("", "42", "mp4"), "wallpaper-42.mp4");
    }

    #[test]
    fn extracts_only_the_video_and_ignores_archive_paths() {
        use zip::write::SimpleFileOptions;

        let directory = std::env::temp_dir().join(format!(
            "ferramari-zip-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let archive_path = directory.join("test.zip");
        {
            let file = fs::File::create(&archive_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("../../evil.txt", options).unwrap();
            writer.write_all(b"not a video").unwrap();
            writer.start_file("nested/video.mp4", options).unwrap();
            writer.write_all(b"fake video bytes").unwrap();
            writer.finish().unwrap();
        }
        let (video, name) = extract_video(&archive_path, &directory, "7", VIDEO_LIMIT).unwrap();
        assert_eq!(video, directory.join("7.mp4"));
        assert_eq!(name, "nested/video.mp4");
        assert!(video.is_file());
        assert!(!directory.join("evil.txt").exists());
        assert!(!directory.join("nested").exists());

        let too_small = extract_video(&archive_path, &directory, "8", 4);
        assert!(too_small.is_err());
        assert!(!directory.join("8.mp4").exists());

        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn parses_frame_rates() {
        assert_eq!(parse_fraction("60/1"), Some(60.0));
        assert_eq!(parse_fraction("30000/1001"), Some(30000.0 / 1001.0));
        assert_eq!(parse_fraction("nonsense"), None);
    }
}
