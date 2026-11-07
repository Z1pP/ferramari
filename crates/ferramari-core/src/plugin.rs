//! Declarative plugin discovery.
//!
//! The first plugin kind is a catalogue source: a plugin is a small
//! `plugin.json` manifest pointing at a Ferramari Catalog API. Plugins are
//! never executed in-process (or spawned), so a plugin cannot read the user's
//! files or run commands; it can only provide an HTTP endpoint for the
//! catalogue client.
//!
//! Discovery lives under `$XDG_DATA_HOME/ferramari/plugins/<id>/plugin.json`.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_ID_CHARS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    /// Adds a Ferramari Catalog API source.
    Catalog,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub kind: PluginKind,
    /// Base URL of the catalogue API, for example `https://example.org/v1`.
    pub api: String,
    #[serde(default)]
    pub engine_min: Option<String>,
}

fn default_version() -> String {
    "1.0".to_string()
}

#[derive(Debug, Clone)]
pub struct Plugin {
    pub manifest: PluginManifest,
}

pub fn list() -> Vec<Plugin> {
    discover(&crate::paths::plugins_dir())
}

pub fn discover(root: &Path) -> Vec<Plugin> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut plugins = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        let manifest_path = path.join("plugin.json");
        let Ok(manifest_metadata) = fs::metadata(&manifest_path) else {
            continue;
        };
        if manifest_metadata.len() > MAX_MANIFEST_BYTES {
            log::warn!("[Plugin] {} is too large", manifest_path.display());
            continue;
        }
        let Ok(text) = fs::read_to_string(&manifest_path) else {
            continue;
        };
        let manifest: PluginManifest = match serde_json::from_str(&text) {
            Ok(manifest) => manifest,
            Err(err) => {
                log::warn!("[Plugin] {} is invalid: {err}", manifest_path.display());
                continue;
            }
        };
        if let Err(err) = validate(&manifest) {
            log::warn!("[Plugin] {}: {err}", manifest_path.display());
            continue;
        }
        plugins.push(Plugin { manifest });
    }
    plugins.sort_by(|left, right| left.manifest.id.cmp(&right.manifest.id));
    plugins
}

fn validate(manifest: &PluginManifest) -> Result<(), String> {
    let id = manifest.id.trim();
    if id.is_empty()
        || id.chars().count() > MAX_ID_CHARS
        || !id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
    {
        return Err("id must be 1–64 ASCII letters, digits, '.', '_' or '-'".to_string());
    }
    if manifest.name.trim().is_empty() {
        return Err("name must not be empty".to_string());
    }
    if manifest.version.chars().count() > 32 {
        return Err("version is too long".to_string());
    }
    if !(manifest.api.starts_with("https://") || manifest.api.starts_with("http://")) {
        return Err("api must be an http:// or https:// URL".to_string());
    }
    if manifest.api.chars().count() > 512 {
        return Err("api URL is too long".to_string());
    }
    if let Some(engine_min) = &manifest.engine_min {
        if version_tuple(engine_min) > version_tuple(env!("CARGO_PKG_VERSION")) {
            return Err(format!(
                "plugin requires Ferramari {engine_min} or newer (current {})",
                env!("CARGO_PKG_VERSION")
            ));
        }
    }
    Ok(())
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
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ferramari-plugin-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_plugin(root: &Path, folder: &str, manifest: &str) {
        let dir = root.join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("plugin.json"), manifest).unwrap();
    }

    #[test]
    fn discovers_valid_catalog_plugin() {
        let root = temp_dir("valid");
        write_plugin(
            &root,
            "example",
            r#"{
                "id": "example.catalog",
                "name": "Example",
                "version": "1.0",
                "kind": "catalog",
                "api": "https://example.org/v1"
            }"#,
        );
        let plugins = discover(&root);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].manifest.id, "example.catalog");
        assert_eq!(plugins[0].manifest.kind, PluginKind::Catalog);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_ids_urls_and_future_versions_are_ignored() {
        let root = temp_dir("invalid");
        write_plugin(
            &root,
            "bad-id",
            r#"{"id":"bad id","name":"X","kind":"catalog","api":"https://example.org"}"#,
        );
        write_plugin(
            &root,
            "file-url",
            r#"{"id":"file","name":"X","kind":"catalog","api":"file:///etc/passwd"}"#,
        );
        write_plugin(
            &root,
            "future",
            r#"{"id":"future","name":"X","kind":"catalog","api":"https://example.org","engine_min":"99.0"}"#,
        );
        assert!(discover(&root).is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn plugin_manifests_are_not_executed_or_symlinked() {
        let root = temp_dir("symlink");
        let outside = temp_dir("symlink-outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(
            outside.join("plugin.json"),
            r#"{"id":"outside","name":"X","kind":"catalog","api":"https://example.org"}"#,
        )
        .unwrap();
        std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
        assert!(discover(&root).is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
