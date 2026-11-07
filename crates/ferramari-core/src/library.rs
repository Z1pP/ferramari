//! User library: favourites, recently used wallpapers and collections.
//!
//! This is deliberately separate from `config.json`: playback settings stay
//! schema-compatible while library data can grow without touching them. The
//! file is written atomically and keeps one previous copy.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::paths;

const LIBRARY_VERSION: u32 = 1;
const HISTORY_LIMIT: usize = 50;

static COLLECTION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub path: String,
    pub used_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub items: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    favorites: Vec<String>,
    #[serde(default)]
    history: Vec<HistoryEntry>,
    #[serde(default)]
    collections: Vec<Collection>,
}

fn default_version() -> u32 {
    LIBRARY_VERSION
}

impl Default for Library {
    fn default() -> Self {
        Self {
            version: LIBRARY_VERSION,
            favorites: Vec::new(),
            history: Vec::new(),
            collections: Vec::new(),
        }
    }
}

impl Library {
    pub fn load() -> Self {
        Self::load_from(&paths::library_path())
    }

    fn load_from(path: &Path) -> Self {
        let mut last_error = None;
        for candidate in [path.to_path_buf(), backup_path(path)] {
            let Ok(text) = fs::read_to_string(&candidate) else {
                continue;
            };
            match serde_json::from_str::<Library>(&text) {
                Ok(mut library) => {
                    if library.version != LIBRARY_VERSION {
                        library.version = LIBRARY_VERSION;
                    }
                    return library;
                }
                Err(err) => last_error = Some((candidate, err)),
            }
        }
        if let Some((candidate, err)) = last_error {
            log::warn!("[Library] Could not read {}: {err}", candidate.display());
        }
        Self::default()
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&paths::library_path())
    }

    fn save_to(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| format!("could not serialize the library: {err}"))?;
        write_atomic(path, format!("{text}\n").as_bytes())
            .map_err(|err| format!("could not write {}: {err}", path.display()))
    }

    pub fn is_favorite(&self, path: &str) -> bool {
        self.favorites.iter().any(|favorite| favorite == path)
    }

    /// Toggle a favourite and return its new state.
    pub fn toggle_favorite(&mut self, path: &str) -> bool {
        if let Some(index) = self.favorites.iter().position(|favorite| favorite == path) {
            self.favorites.remove(index);
            false
        } else {
            self.favorites.push(path.to_string());
            true
        }
    }

    pub fn history(&self) -> &[HistoryEntry] {
        &self.history
    }

    pub fn record_use(&mut self, path: &str) {
        self.history.retain(|entry| entry.path != path);
        self.history.insert(
            0,
            HistoryEntry {
                path: path.to_string(),
                used_at: now_seconds(),
            },
        );
        self.history.truncate(HISTORY_LIMIT);
    }

    pub fn collections(&self) -> &[Collection] {
        &self.collections
    }

    pub fn collection(&self, id: &str) -> Option<&Collection> {
        self.collections
            .iter()
            .find(|collection| collection.id == id)
    }

    pub fn collection_contains(&self, id: &str, path: &str) -> bool {
        self.collection(id)
            .is_some_and(|collection| collection.items.iter().any(|item| item == path))
    }

    pub fn create_collection(&mut self, name: &str) -> Option<String> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let id = format!(
            "c{}-{}",
            now_seconds(),
            COLLECTION_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        self.collections.push(Collection {
            id: id.clone(),
            name: name.to_string(),
            items: Vec::new(),
        });
        Some(id)
    }

    pub fn rename_collection(&mut self, id: &str, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        match self
            .collections
            .iter_mut()
            .find(|collection| collection.id == id)
        {
            Some(collection) => {
                collection.name = name.to_string();
                true
            }
            None => false,
        }
    }

    pub fn delete_collection(&mut self, id: &str) -> bool {
        let before = self.collections.len();
        self.collections.retain(|collection| collection.id != id);
        self.collections.len() != before
    }

    pub fn add_to_collection(&mut self, id: &str, path: &str) -> bool {
        match self
            .collections
            .iter_mut()
            .find(|collection| collection.id == id)
        {
            Some(collection) => {
                if collection.items.iter().any(|item| item == path) {
                    return false;
                }
                collection.items.push(path.to_string());
                true
            }
            None => false,
        }
    }

    pub fn remove_from_collection(&mut self, id: &str, path: &str) -> bool {
        match self
            .collections
            .iter_mut()
            .find(|collection| collection.id == id)
        {
            Some(collection) => {
                let before = collection.items.len();
                collection.items.retain(|item| item != path);
                collection.items.len() != before
            }
            None => false,
        }
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("json.bak")
}

fn temp_path(path: &Path) -> PathBuf {
    path.with_extension("json.tmp")
}

fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = temp_path(path);
    {
        let mut file = fs::File::create(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    if path.is_file() {
        fs::copy(path, backup_path(path))?;
    }
    fs::rename(&temp, path)?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        let _ = fs::File::open(parent).and_then(|dir| dir.sync_all());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ferramari-library-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn favourites_toggle_both_ways() {
        let mut library = Library::default();
        assert!(!library.is_favorite("/a.mp4"));
        assert!(library.toggle_favorite("/a.mp4"));
        assert!(library.is_favorite("/a.mp4"));
        assert!(!library.toggle_favorite("/a.mp4"));
        assert!(!library.is_favorite("/a.mp4"));
    }

    #[test]
    fn history_is_most_recent_first_and_deduplicated() {
        let mut library = Library::default();
        library.record_use("/a.mp4");
        library.record_use("/b.mp4");
        library.record_use("/a.mp4");
        let paths: Vec<&str> = library
            .history()
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(paths, vec!["/a.mp4", "/b.mp4"]);
    }

    #[test]
    fn collections_support_create_add_remove_rename_delete() {
        let mut library = Library::default();
        let id = library.create_collection(" Anime ").unwrap();
        assert_eq!(library.collection(&id).unwrap().name, "Anime");
        assert!(library.add_to_collection(&id, "/a.mp4"));
        assert!(!library.add_to_collection(&id, "/a.mp4"));
        assert!(library.collection_contains(&id, "/a.mp4"));
        assert!(library.remove_from_collection(&id, "/a.mp4"));
        assert!(!library.collection_contains(&id, "/a.mp4"));
        assert!(library.rename_collection(&id, "Favourites"));
        assert_eq!(library.collection(&id).unwrap().name, "Favourites");
        assert!(library.delete_collection(&id));
        assert!(library.collection(&id).is_none());
    }

    #[test]
    fn saves_and_reloads_atomically() {
        let dir = temp_dir("save");
        let path = dir.join("library.json");
        let mut library = Library::default();
        library.toggle_favorite("/a.mp4");
        library.record_use("/a.mp4");
        library.create_collection("Anime");
        library.save_to(&path).unwrap();
        // The second save creates the .bak copy used for crash recovery.
        library.record_use("/b.mp4");
        library.save_to(&path).unwrap();

        let loaded = Library::load_from(&path);
        assert!(loaded.is_favorite("/a.mp4"));
        assert_eq!(loaded.history().len(), 2);
        assert_eq!(loaded.collections().len(), 1);

        fs::write(&path, b"{broken").unwrap();
        let recovered = Library::load_from(&path);
        assert!(recovered.is_favorite("/a.mp4"));

        let _ = fs::remove_dir_all(dir);
    }
}
