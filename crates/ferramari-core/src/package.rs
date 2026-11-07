//! The `.ferramari` package format.
//!
//! A package is a directory with a `manifest.json`, one entry document/video
//! and optional `preview`, `assets/`, `scripts/` and `resources/`. Validation
//! is deliberately strict: no symlinks, no absolute paths, no `..`, a bounded
//! file count and a 2 GiB size limit. Import copies the package into the
//! user's data directory; export builds one from a local video.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

const FORMAT_VERSION: u32 = 1;
const MAX_PACKAGE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_FILES: usize = 10_000;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_CHECKSUMS_BYTES: u64 = 4 * 1024 * 1024;
const MAX_NAME_CHARS: usize = 100;
const MAX_AUTHOR_CHARS: usize = 120;
/// Name of the integrity manifest inside a package. It is never hashed itself.
pub const CHECKSUMS_FILE: &str = "checksums.json";
/// Package signature; like the checksums it is never hashed itself.
pub const SIGNATURE_FILE: &str = crate::signing::SIGNATURE_FILE;
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "webm", "gif", "mkv", "mov", "m4v"];
const WEB_EXTENSIONS: &[&str] = &["html", "htm"];
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageType {
    Video,
    Web,
}

impl PackageType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Web => "web",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requirements {
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub javascript: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default = "default_format")]
    pub format: u32,
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(rename = "type")]
    pub kind: PackageType,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub resolution: Option<String>,
    #[serde(default)]
    pub entry: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub requirements: Requirements,
    #[serde(default)]
    pub engine_min: Option<String>,
}

fn default_format() -> u32 {
    FORMAT_VERSION
}

fn default_version() -> String {
    "1.0".to_string()
}

#[derive(Debug, Clone)]
pub struct ValidatedPackage {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub entry: PathBuf,
    pub preview: Option<PathBuf>,
    pub size_bytes: u64,
    /// True when `checksums.json` covered every file in the package.
    pub checksummed: bool,
    /// Result of the Ed25519 signature check, if `signature.json` exists.
    pub signature: crate::signing::SignatureStatus,
}

#[derive(Debug, Clone)]
pub struct ImportedPackage {
    pub name: String,
    pub kind: PackageType,
    pub entry: PathBuf,
    pub preview: Option<PathBuf>,
    pub package_dir: PathBuf,
    pub size_bytes: u64,
    pub checksummed: bool,
    pub signature: crate::signing::SignatureStatus,
}

pub fn load(root: &Path) -> Result<ValidatedPackage, String> {
    let root =
        fs::canonicalize(root).map_err(|err| format!("cannot open {}: {err}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }

    let mut files = Vec::new();
    let mut total = 0u64;
    walk(&root, &root, 0, &mut files, &mut total)?;

    let manifest_path = root.join("manifest.json");
    let metadata =
        fs::metadata(&manifest_path).map_err(|_| "manifest.json is missing".to_string())?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err("manifest.json is too large".to_string());
    }
    let text = fs::read_to_string(&manifest_path)
        .map_err(|err| format!("cannot read manifest.json: {err}"))?;
    let manifest: Manifest =
        serde_json::from_str(&text).map_err(|err| format!("invalid manifest.json: {err}"))?;
    validate_manifest(&root, &manifest, total)?;

    let entry_name = manifest
        .entry
        .clone()
        .unwrap_or_else(|| default_entry(manifest.kind));
    let entry = checked_file(&root, &entry_name, "entry")?;
    ensure_extension(&entry, allowed_extensions(manifest.kind), "entry")?;

    let preview = match manifest.preview.as_deref() {
        Some(name) => {
            let preview = checked_file(&root, name, "preview")?;
            ensure_extension(&preview, IMAGE_EXTENSIONS, "preview")?;
            Some(preview)
        }
        None => {
            let candidate = root.join("preview.jpg");
            (candidate.is_file()).then_some(candidate)
        }
    };

    // Integrity is checked last: a tampered package must not be applied even
    // when every structural rule above still passes.
    let checksummed = verify_checksums(&root)?;
    let signature = crate::signing::verify_package(&root)?;

    Ok(ValidatedPackage {
        root,
        manifest,
        entry,
        preview,
        size_bytes: total,
        checksummed,
        signature,
    })
}

/// SHA-256 of every regular file in the package, keyed by its relative path.
/// `checksums.json` itself is never hashed.
pub fn checksums(root: &Path) -> Result<BTreeMap<String, String>, String> {
    let root =
        fs::canonicalize(root).map_err(|err| format!("cannot open {}: {err}", root.display()))?;
    let mut files = Vec::new();
    let mut total = 0u64;
    walk(&root, &root, 0, &mut files, &mut total)?;
    let mut digests = BTreeMap::new();
    for path in files {
        let relative = path
            .strip_prefix(&root)
            .map_err(|_| "file outside the package".to_string())?
            .to_string_lossy()
            .into_owned();
        // The integrity files describe the package; hashing them would make
        // signing impossible.
        if relative == CHECKSUMS_FILE || relative == SIGNATURE_FILE {
            continue;
        }
        digests.insert(relative, file_digest(&path)?);
    }
    Ok(digests)
}

/// Write `checksums.json` for the package. Export calls this after the manifest
/// exists, so the manifest itself is covered.
pub fn write_checksums(root: &Path) -> Result<(), String> {
    let digests = checksums(root)?;
    let text = serde_json::to_string_pretty(&digests)
        .map_err(|err| format!("cannot serialize checksums: {err}"))?;
    write_text(&root.join(CHECKSUMS_FILE), &text)
}

/// Compare `checksums.json` with the files on disk. `Ok(false)` means the
/// package carries no checksums; a mismatch or a missing file is an error.
fn verify_checksums(root: &Path) -> Result<bool, String> {
    let path = root.join(CHECKSUMS_FILE);
    if !path.is_file() {
        return Ok(false);
    }
    let metadata =
        fs::metadata(&path).map_err(|err| format!("cannot stat {}: {err}", path.display()))?;
    if metadata.len() > MAX_CHECKSUMS_BYTES {
        return Err("checksums.json is too large".to_string());
    }
    let text = fs::read_to_string(&path)
        .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let expected: BTreeMap<String, String> =
        serde_json::from_str(&text).map_err(|err| format!("invalid checksums.json: {err}"))?;
    let actual = checksums(root)?;
    if expected.len() != actual.len() {
        return Err(format!(
            "checksums.json lists {} files, the package has {}",
            expected.len(),
            actual.len()
        ));
    }
    for (name, digest) in &expected {
        match actual.get(name) {
            Some(found) if found == digest => {}
            Some(_) => return Err(format!("{name} does not match its recorded SHA-256")),
            None => return Err(format!("{name} is listed in checksums.json but missing")),
        }
    }
    Ok(true)
}

fn file_digest(path: &Path) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|err| format!("cannot open {}: {err}", path.display()))?;
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{:02x}", b)).collect())
}

fn validate_manifest(root: &Path, manifest: &Manifest, total: u64) -> Result<(), String> {
    if manifest.format > FORMAT_VERSION {
        return Err(format!(
            "package format {} is newer than supported format {FORMAT_VERSION}",
            manifest.format
        ));
    }
    if manifest.format == 0 {
        return Err("package format must be 1".to_string());
    }
    let name = manifest.name.trim();
    if name.is_empty() {
        return Err("name must not be empty".to_string());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err("name is too long".to_string());
    }
    if manifest.author.chars().count() > MAX_AUTHOR_CHARS {
        return Err("author is too long".to_string());
    }
    if manifest.version.chars().count() > 32 {
        return Err("version is too long".to_string());
    }
    if let Some(resolution) = &manifest.resolution {
        let (width, height) = resolution
            .split_once('x')
            .ok_or_else(|| "resolution must look like 1920x1080".to_string())?;
        width
            .parse::<u32>()
            .ok()
            .zip(height.parse::<u32>().ok())
            .filter(|(width, height)| *width > 0 && *height > 0)
            .ok_or_else(|| "resolution must look like 1920x1080".to_string())?;
    }
    if let Some(engine_min) = &manifest.engine_min {
        if version_tuple(engine_min) > version_tuple(env!("CARGO_PKG_VERSION")) {
            return Err(format!(
                "package requires Ferramari {engine_min} or newer (current {})",
                env!("CARGO_PKG_VERSION")
            ));
        }
    }
    if total > MAX_PACKAGE_BYTES {
        return Err("package is larger than 2 GiB".to_string());
    }
    // Manifest and entry are checked by the caller; make sure the root itself
    // is not a symlink (canonicalize above already resolves it, but a caller
    // could pass a symlinked path intentionally).
    let _ = root;
    Ok(())
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    files: &mut Vec<PathBuf>,
    total: &mut u64,
) -> Result<(), String> {
    if depth > 32 {
        return Err("package directory tree is too deep".to_string());
    }
    let entries =
        fs::read_dir(dir).map_err(|err| format!("cannot read {}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("cannot read package: {err}"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|err| format!("cannot stat {}: {err}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "symlinks are not allowed in packages: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            walk(root, &path, depth + 1, files, total)?;
        } else if metadata.is_file() {
            if files.len() >= MAX_FILES {
                return Err(format!("package contains more than {MAX_FILES} files"));
            }
            *total = total
                .checked_add(metadata.len())
                .ok_or_else(|| "package size overflow".to_string())?;
            if *total > MAX_PACKAGE_BYTES {
                return Err("package is larger than 2 GiB".to_string());
            }
            files.push(path);
        } else {
            return Err(format!("unsupported file type: {}", path.display()));
        }
    }
    let _ = root;
    Ok(())
}

fn checked_file(root: &Path, value: &str, label: &str) -> Result<PathBuf, String> {
    if value.trim().is_empty() {
        return Err(format!("{label} path must not be empty"));
    }
    let relative = Path::new(value);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("{label} path must be a relative path without '..'"));
    }
    let path = root.join(relative);
    if !path.is_file() {
        return Err(format!("{label} file not found: {value}"));
    }
    // Resolve and ensure it still lives inside the package. This also catches
    // a missing component or a path that escapes through an unusual spelling.
    let canonical =
        fs::canonicalize(&path).map_err(|err| format!("cannot resolve {label} file: {err}"))?;
    if !canonical.starts_with(root) {
        return Err(format!("{label} path escapes the package"));
    }
    Ok(canonical)
}

fn ensure_extension(path: &Path, allowed: &[&str], label: &str) -> Result<(), String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase());
    if extension
        .as_deref()
        .is_some_and(|extension| allowed.contains(&extension))
    {
        Ok(())
    } else {
        Err(format!(
            "{label} must have one of these extensions: {}",
            allowed.join(", ")
        ))
    }
}

fn allowed_extensions(kind: PackageType) -> &'static [&'static str] {
    match kind {
        PackageType::Video => VIDEO_EXTENSIONS,
        PackageType::Web => WEB_EXTENSIONS,
    }
}

fn default_entry(kind: PackageType) -> String {
    match kind {
        PackageType::Video => "wallpaper.mp4".to_string(),
        PackageType::Web => "index.html".to_string(),
    }
}

/// Import a validated package into `destination` (usually
/// `~/.local/share/ferramari/packages`) and return its installed paths.
pub fn import(source: &Path, destination: &Path) -> Result<ImportedPackage, String> {
    let package = load(source)?;
    fs::create_dir_all(destination)
        .map_err(|err| format!("cannot create {}: {err}", destination.display()))?;
    let base = slug(&package.manifest.name);
    let mut target = destination.join(&base);
    let mut suffix = 2;
    while target.exists() {
        target = destination.join(format!("{base}-{suffix}"));
        suffix += 1;
        if suffix > 999 {
            return Err("too many packages with the same name".to_string());
        }
    }
    if let Err(err) = copy_tree(&package.root, &target) {
        let _ = fs::remove_dir_all(&target);
        return Err(err);
    }
    match load(&target) {
        Ok(installed) => Ok(ImportedPackage {
            name: installed.manifest.name,
            kind: installed.manifest.kind,
            entry: installed.entry,
            preview: installed.preview,
            package_dir: target,
            size_bytes: installed.size_bytes,
            checksummed: installed.checksummed,
            signature: installed.signature,
        }),
        Err(err) => {
            let _ = fs::remove_dir_all(&target);
            Err(format!("imported package failed validation: {err}"))
        }
    }
}

/// Video entries from installed packages, for the local library grid. Invalid
/// packages are ignored here; they still fail with a clear message on import.
pub fn installed_videos() -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(crate::paths::packages_dir()) else {
        return Vec::new();
    };
    let mut videos = Vec::new();
    for entry in entries.flatten() {
        let root = entry.path();
        // A listing does not need the full size/symlink walk. Read the manifest,
        // then let `checked_file` enforce that the entry stays inside the
        // package; validation runs again when the package is applied.
        let Some(manifest) = read_manifest(&root) else {
            continue;
        };
        if manifest.kind != PackageType::Video {
            continue;
        }
        let entry_name = manifest
            .entry
            .unwrap_or_else(|| default_entry(PackageType::Video));
        let Ok(root) = fs::canonicalize(&root) else {
            continue;
        };
        if let Ok(entry) = checked_file(&root, &entry_name, "entry") {
            videos.push(entry);
        }
    }
    videos.sort();
    videos
}

fn read_manifest(root: &Path) -> Option<Manifest> {
    let path = root.join("manifest.json");
    let metadata = fs::metadata(&path).ok()?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// The package directory a video entry came from, when it came from one. Used
/// by the library UI to offer authorship actions on installed packages.
pub fn package_dir_for_entry(entry: &Path) -> Option<PathBuf> {
    let entry = fs::canonicalize(entry).ok()?;
    let dir = entry.parent()?.to_path_buf();
    if !dir.join("manifest.json").is_file() {
        return None;
    }
    Some(dir)
}

/// Build a video package from a local file. `parent` is the directory that
/// will contain `<name>.ferramari`; it must already exist.
pub fn export_video(video: &Path, parent: &Path) -> Result<PathBuf, String> {
    let video =
        fs::canonicalize(video).map_err(|err| format!("cannot open {}: {err}", video.display()))?;
    if !video.is_file() {
        return Err(format!("{} is not a file", video.display()));
    }
    let size = fs::metadata(&video)
        .map_err(|err| format!("cannot stat {}: {err}", video.display()))?
        .len();
    if size > MAX_PACKAGE_BYTES {
        return Err("video is larger than 2 GiB".to_string());
    }
    let extension = video
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .ok_or_else(|| "video has no extension".to_string())?;
    if !VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        return Err("unsupported video extension".to_string());
    }
    let name = video
        .file_stem()
        .map(|stem| stem.to_string_lossy().trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Wallpaper".to_string());
    let target = parent.join(format!("{}.ferramari", slug(&name)));
    if target.exists() {
        return Err(format!("{} already exists", target.display()));
    }
    fs::create_dir_all(&target)
        .map_err(|err| format!("cannot create {}: {err}", target.display()))?;

    let entry_name = format!("wallpaper.{extension}");
    let entry = target.join(&entry_name);
    if let Err(err) = fs::copy(&video, &entry) {
        let _ = fs::remove_dir_all(&target);
        return Err(format!("cannot copy video: {err}"));
    }

    // A preview is optional; a thumbnail failure must not reject the package.
    let preview_name = crate::thumbnails::generate(&video).and_then(|thumbnail| {
        let destination = target.join("preview.jpg");
        fs::copy(&thumbnail, &destination)
            .ok()
            .map(|_| "preview.jpg".to_string())
    });

    let resolution =
        crate::catalogue::probe(&video).map(|probe| format!("{}x{}", probe.width, probe.height));
    let manifest = Manifest {
        format: FORMAT_VERSION,
        name,
        author: std::env::var("USER").unwrap_or_else(|_| "Ferramari user".to_string()),
        version: "1.0".to_string(),
        kind: PackageType::Video,
        description: String::new(),
        resolution,
        entry: Some(entry_name),
        preview: preview_name,
        requirements: Requirements::default(),
        engine_min: Some(env!("CARGO_PKG_VERSION").to_string()),
    };
    let text = serde_json::to_string_pretty(&manifest)
        .map_err(|err| format!("cannot serialize manifest: {err}"))?;
    let manifest_path = target.join("manifest.json");
    if let Err(err) = write_manifest(&manifest_path, &text) {
        let _ = fs::remove_dir_all(&target);
        return Err(err);
    }
    if let Err(err) = write_checksums(&target) {
        let _ = fs::remove_dir_all(&target);
        return Err(err);
    }
    // Sign with the user's key when they have one; an unsigned export is still
    // a valid package, just without authorship.
    match crate::signing::sign_if_available(&target) {
        Ok(true) => log::info!("[Package] Signed {}", target.display()),
        Ok(false) => log::debug!("[Package] No signing key; exported unsigned"),
        Err(err) => {
            let _ = fs::remove_dir_all(&target);
            return Err(err);
        }
    }
    if let Err(err) = load(&target) {
        let _ = fs::remove_dir_all(&target);
        return Err(format!("exported package failed validation: {err}"));
    }
    Ok(target)
}

fn write_manifest(path: &Path, text: &str) -> Result<(), String> {
    write_text(path, text)
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    let mut file =
        fs::File::create(path).map_err(|err| format!("cannot write {}: {err}", path.display()))?;
    file.write_all(format!("{text}\n").as_bytes())
        .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
    file.sync_all()
        .map_err(|err| format!("cannot flush {}: {err}", path.display()))
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target)
        .map_err(|err| format!("cannot create {}: {err}", target.display()))?;
    copy_tree_inner(source, target)?;
    Ok(())
}

fn copy_tree_inner(source: &Path, target: &Path) -> Result<(), String> {
    let entries =
        fs::read_dir(source).map_err(|err| format!("cannot read {}: {err}", source.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("cannot read package: {err}"))?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        let metadata = fs::symlink_metadata(&from)
            .map_err(|err| format!("cannot stat {}: {err}", from.display()))?;
        if metadata.file_type().is_symlink() {
            return Err("symlinks are not allowed in packages".to_string());
        }
        if metadata.is_dir() {
            fs::create_dir_all(&to)
                .map_err(|err| format!("cannot create {}: {err}", to.display()))?;
            copy_tree_inner(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|err| format!("cannot copy {}: {err}", from.display()))?;
        }
    }
    Ok(())
}

fn slug(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "wallpaper".to_string()
    } else {
        slug
    }
}

fn version_tuple(version: &str) -> (u64, u64, u64) {
    let mut parts = version
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ferramari-package-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_package(dir: &Path, manifest: &str, entry: &str, bytes: &[u8]) {
        fs::write(dir.join("manifest.json"), manifest).unwrap();
        fs::write(dir.join(entry), bytes).unwrap();
    }

    #[test]
    fn valid_video_package_loads() {
        let dir = temp_dir("valid");
        write_package(
            &dir,
            r#"{"name":"Neon","author":"A","version":"1.0","type":"video"}"#,
            "wallpaper.mp4",
            b"x",
        );
        let package = load(&dir).unwrap();
        assert_eq!(package.manifest.name, "Neon");
        assert_eq!(package.manifest.kind, PackageType::Video);
        assert_eq!(
            package.entry,
            fs::canonicalize(dir.join("wallpaper.mp4")).unwrap()
        );
        assert!(package.size_bytes > 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn web_package_uses_index_default_and_accepts_resolution() {
        let dir = temp_dir("web");
        write_package(
            &dir,
            r#"{"name":"Scene","author":"A","version":"2.0","type":"web","resolution":"1920x1080"}"#,
            "index.html",
            b"<html></html>",
        );
        let package = load(&dir).unwrap();
        assert_eq!(package.manifest.kind, PackageType::Web);
        assert_eq!(package.manifest.resolution.as_deref(), Some("1920x1080"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn path_traversal_and_symlinks_are_rejected() {
        let dir = temp_dir("escape");
        write_package(
            &dir,
            r#"{"name":"Bad","author":"A","version":"1.0","type":"video","entry":"../outside.mp4"}"#,
            "wallpaper.mp4",
            b"x",
        );
        assert!(load(&dir).unwrap_err().contains("relative path"));
        let _ = fs::remove_dir_all(dir);

        let dir = temp_dir("symlink");
        write_package(
            &dir,
            r#"{"name":"Link","author":"A","version":"1.0","type":"video"}"#,
            "wallpaper.mp4",
            b"x",
        );
        std::os::unix::fs::symlink("/etc/passwd", dir.join("leak")).unwrap();
        assert!(load(&dir).unwrap_err().contains("symlinks"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn newer_format_and_engine_are_rejected() {
        let dir = temp_dir("newer");
        write_package(
            &dir,
            r#"{"format":99,"name":"Future","author":"A","version":"1.0","type":"video"}"#,
            "wallpaper.mp4",
            b"x",
        );
        assert!(load(&dir).unwrap_err().contains("newer"));
        let _ = fs::remove_dir_all(dir);

        let dir = temp_dir("engine");
        write_package(
            &dir,
            r#"{"name":"Future","author":"A","version":"1.0","type":"video","engine_min":"99.0.0"}"#,
            "wallpaper.mp4",
            b"x",
        );
        assert!(load(&dir).unwrap_err().contains("requires"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn import_copies_and_revalidates_the_package() {
        let source = temp_dir("import-src");
        write_package(
            &source,
            r#"{"name":"My Wallpaper","author":"A","version":"1.0","type":"video"}"#,
            "wallpaper.mp4",
            b"video",
        );
        let destination = temp_dir("import-dst");
        let imported = import(&source, &destination).unwrap();
        assert_eq!(imported.name, "My Wallpaper");
        assert_eq!(imported.kind, PackageType::Video);
        assert!(imported.entry.is_file());
        assert!(imported.entry.starts_with(&destination));
        assert!(imported.package_dir.ends_with("my-wallpaper"));
        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(destination);
    }

    #[test]
    fn export_builds_a_reloadable_video_package() {
        let source = temp_dir("export-src");
        let video = source.join("Sample Clip.mp4");
        fs::write(&video, b"not a real video").unwrap();
        let parent = temp_dir("export-dst");
        let package = export_video(&video, &parent).unwrap();
        assert!(package.ends_with("sample-clip.ferramari"));
        let loaded = load(&package).unwrap();
        assert_eq!(loaded.manifest.name, "Sample Clip");
        assert_eq!(loaded.manifest.kind, PackageType::Video);
        assert!(loaded.entry.ends_with("wallpaper.mp4"));
        assert!(loaded.checksummed);
        assert!(package.join(CHECKSUMS_FILE).is_file());
        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn a_tampered_file_is_rejected() {
        let source = temp_dir("tamper-src");
        let video = source.join("Clip.mp4");
        fs::write(&video, b"original bytes").unwrap();
        let parent = temp_dir("tamper-dst");
        let package = export_video(&video, &parent).unwrap();
        fs::write(package.join("wallpaper.mp4"), b"replaced bytes").unwrap();
        let error = load(&package).unwrap_err();
        assert!(error.contains("SHA-256"), "unexpected error: {error}");
        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn an_unlisted_file_is_rejected_and_import_keeps_checksums() {
        let source = temp_dir("extra-src");
        write_package(
            &source,
            r#"{"name":"My Wallpaper","author":"A","version":"1.0","type":"video"}"#,
            "wallpaper.mp4",
            b"video",
        );
        write_checksums(&source).unwrap();
        fs::write(source.join("surprise.txt"), b"payload").unwrap();
        assert!(load(&source).unwrap_err().contains("checksums.json"));

        let _ = fs::remove_file(source.join("surprise.txt"));
        let destination = temp_dir("extra-dst");
        let imported = import(&source, &destination).unwrap();
        assert!(imported.checksummed);
        assert!(imported.package_dir.join(CHECKSUMS_FILE).is_file());
        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(destination);
    }

    #[test]
    fn a_package_without_checksums_still_loads() {
        let dir = temp_dir("unsigned");
        write_package(
            &dir,
            r#"{"name":"Old","author":"A","version":"1.0","type":"video"}"#,
            "wallpaper.mp4",
            b"x",
        );
        let package = load(&dir).unwrap();
        assert!(!package.checksummed);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_package_entry_reports_its_package_dir_and_a_plain_video_does_not() {
        let dir = temp_dir("entry");
        write_package(
            &dir,
            r#"{"name":"My Wallpaper","author":"A","version":"1.0","type":"video"}"#,
            "wallpaper.mp4",
            b"video",
        );
        let entry = dir.join("wallpaper.mp4");
        let package = package_dir_for_entry(&entry).unwrap();
        assert!(package.join("manifest.json").is_file());

        let plain = temp_dir("plain");
        let video = plain.join("clip.mp4");
        fs::write(&video, b"video").unwrap();
        assert!(package_dir_for_entry(&video).is_none());
        assert!(package_dir_for_entry(Path::new("/does/not/exist.mp4")).is_none());

        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(plain);
    }

    #[test]
    fn slugs_and_versions_are_stable() {
        assert_eq!(slug("My Wallpaper"), "my-wallpaper");
        assert_eq!(slug("***"), "wallpaper");
        assert!(version_tuple("1.10.0") > version_tuple("1.9.9"));
        assert_eq!(version_tuple("1.2"), (1, 2, 0));
    }
}
