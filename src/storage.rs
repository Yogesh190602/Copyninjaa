use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::content::{ext_for_mime, ClipContent};

static STORAGE: OnceLock<Storage> = OnceLock::new();

/// Characters kept in the stored one-line preview.
const PREVIEW_CHARS: usize = 100;

/// Clipboard history entry. Supports both the new `content` field and legacy `text`/`preview` fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipEntry {
    /// New format: tagged content (text or image)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<ClipContent>,
    /// Legacy text field. Still read for old files, no longer written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Legacy preview field
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    pub hash: String,
    pub time: f64,
    #[serde(default)]
    pub pinned: bool,
    /// When this entry last changed (copied again, pinned or unpinned), so sync
    /// can tell which device has the newer state. 0 in files from older versions.
    #[serde(default)]
    pub updated: f64,
}

impl ClipEntry {
    /// Get the resolved content, handling legacy entries that only have text/preview.
    pub fn resolved_content(&self) -> ClipContent {
        if let Some(ref content) = self.content {
            content.clone()
        } else if let Some(ref text) = self.text {
            ClipContent::Text {
                text: text.clone(),
                preview: self.preview.clone().unwrap_or_default(),
            }
        } else {
            ClipContent::Text {
                text: String::new(),
                preview: String::new(),
            }
        }
    }

    /// Create a new text entry.
    pub fn new_text(text: String, hash: String, time: f64) -> Self {
        let preview = make_preview(&text);
        Self {
            content: Some(ClipContent::Text { text, preview }),
            text: None,
            preview: None,
            hash,
            time,
            pinned: false,
            updated: time,
        }
    }

    /// Create a new image entry.
    pub fn new_image(path: PathBuf, mime: String, hash: String, time: f64) -> Self {
        Self {
            content: Some(ClipContent::Image { path, mime }),
            text: None,
            preview: None,
            hash,
            time,
            pinned: false,
            updated: time,
        }
    }

    /// The last time anything about this entry changed.
    pub fn modified(&self) -> f64 {
        self.time.max(self.updated)
    }
}

/// One-line preview: whitespace runs collapsed, first `PREVIEW_CHARS` characters.
pub fn make_preview(text: &str) -> String {
    let mut out = String::new();
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        if out.len() >= PREVIEW_CHARS * 4 {
            break;
        }
    }
    out.chars().take(PREVIEW_CHARS).collect()
}

#[derive(Debug, Clone)]
pub struct Storage {
    pub history_path: PathBuf,
    pub max_entries: usize,
    pub max_backups: usize,
    pub image_dir: PathBuf,
    pub max_image_bytes: usize,
    pub max_text_bytes: usize,
    /// Where to publish changes for cross-device sync, if enabled.
    pub sync_dir: Option<PathBuf>,
}

impl Storage {
    pub fn from_config(config: &Config) -> Self {
        let storage = Self {
            history_path: config.history_file.clone(),
            max_entries: config.max_entries,
            max_backups: config.max_backups,
            image_dir: config.image_dir.clone(),
            max_image_bytes: config.max_image_size_mb as usize * 1024 * 1024,
            max_text_bytes: config.max_text_size_kb as usize * 1024,
            sync_dir: config.sync_dir(),
        };
        storage.ensure_image_dir();
        storage
    }

    /// Create the image directory, private to the user: clipboard images are
    /// often screenshots of anything on screen.
    pub(crate) fn ensure_image_dir(&self) {
        if let Err(e) = fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.image_dir)
        {
            log::warn!("Cannot create {}: {}", self.image_dir.display(), e);
            return;
        }
        let _ = fs::set_permissions(&self.image_dir, fs::Permissions::from_mode(0o700));
    }

    /// Where the image with this hash is stored.
    pub fn image_path(&self, hash: &str, mime: &str) -> PathBuf {
        self.image_dir
            .join(format!("{}.{}", hash, ext_for_mime(mime)))
    }

    /// Where the picker caches the small preview of an image.
    pub fn thumbnail_path(&self, hash: &str) -> PathBuf {
        self.image_dir.join("thumbs").join(format!("{}.png", hash))
    }

    /// Read the history without locking (for display).
    pub fn load_history(&self) -> Vec<ClipEntry> {
        load_history_from(&self.history_path, self.max_backups).0
    }

    /// Replace the whole history.
    #[cfg(test)]
    pub fn save_history(&self, history: &[ClipEntry]) -> Result<()> {
        self.update(|h| *h = history.to_vec())
    }

    fn lock(&self) -> Result<File> {
        let path = self.history_path.with_extension("lock");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&path)?;
        file.lock()?;
        Ok(file)
    }

    /// Read-modify-write the history while holding an exclusive lock, so the
    /// daemon, the picker and the sync thread never overwrite each other's
    /// changes. `f` returns its result and whether it changed anything; the
    /// file is only rewritten if it did.
    pub fn update_if<R>(&self, f: impl FnOnce(&mut Vec<ClipEntry>) -> (R, bool)) -> Result<R> {
        let _lock = self.lock()?;
        let (mut history, main_ok) = load_history_from(&self.history_path, self.max_backups);
        let (result, changed) = f(&mut history);
        if changed {
            // Don't rotate a corrupt main file into the backups we just recovered from.
            if main_ok {
                rotate_backups(&self.history_path, self.max_backups);
            }
            save_history_to(&history, &self.history_path)?;
        }
        Ok(result)
    }

    /// Like `update_if`, for changes that always modify the history.
    pub fn update<R>(&self, f: impl FnOnce(&mut Vec<ClipEntry>) -> R) -> Result<R> {
        self.update_if(|history| (f(history), true))
    }

    pub fn process_text(&self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if text.len() > self.max_text_bytes {
            log::info!(
                "Not saving {} KB of text: larger than max_text_size_kb",
                text.len() / 1024
            );
            return;
        }

        // Store the text exactly as copied; trimming would break pasted code indentation.
        let hash = get_hash(text);
        let result = self.update(|history| {
            let now = now();
            match history.iter().position(|e| e.hash == hash) {
                // Dedup: if entry exists, move it to top
                Some(pos) => {
                    let mut entry = history.remove(pos);
                    entry.time = now;
                    entry.updated = now;
                    history.insert(0, entry);
                }
                None => history.insert(0, ClipEntry::new_text(text.to_string(), hash.clone(), now)),
            }
            self.prune(history);
            history[0].clone()
        });
        match result {
            Ok(entry) => self.publish(&entry),
            Err(e) => log::error!("Failed to save history: {}", e),
        }
    }

    /// Process image data from clipboard — save to file, add to history.
    pub fn process_image(&self, data: &[u8], mime: &str) {
        if data.is_empty() {
            return;
        }
        if data.len() > self.max_image_bytes {
            log::info!(
                "Not saving {} KB image: larger than max_image_size_mb",
                data.len() / 1024
            );
            return;
        }

        let hash = get_hash_bytes(data);
        let image_path = self.image_path(&hash, mime);
        let result = self.update_if(|history| {
            let now = now();
            if let Some(pos) = history.iter().position(|e| e.hash == hash) {
                let mut entry = history.remove(pos);
                entry.time = now;
                entry.updated = now;
                // Re-create the file if it went missing.
                if let Some(ClipContent::Image { path, .. }) = &entry.content {
                    if !path.exists() && self.write_image(&image_path, data) {
                        entry.content = Some(ClipContent::Image {
                            path: image_path.clone(),
                            mime: mime.to_string(),
                        });
                    }
                }
                history.insert(0, entry);
            } else {
                if !self.write_image(&image_path, data) {
                    return (None, false);
                }
                let entry =
                    ClipEntry::new_image(image_path.clone(), mime.to_string(), hash.clone(), now);
                history.insert(0, entry);
            }
            self.prune(history);
            (Some(history[0].clone()), true)
        });
        match result {
            Ok(Some(entry)) => self.publish(&entry),
            Ok(None) => {}
            Err(e) => log::error!("Failed to save history: {}", e),
        }
    }

    fn write_image(&self, path: &Path, data: &[u8]) -> bool {
        self.ensure_image_dir();
        match write_private(path, data) {
            Ok(()) => true,
            Err(e) => {
                log::error!("Failed to save image {}: {}", path.display(), e);
                false
            }
        }
    }

    /// Flip an entry's pinned state. Returns the new state, or None if the
    /// entry no longer exists.
    pub fn toggle_pin(&self, hash: &str) -> Option<bool> {
        let result = self.update_if(
            |history| match history.iter_mut().find(|e| e.hash == hash) {
                Some(entry) => {
                    entry.pinned = !entry.pinned;
                    entry.updated = now();
                    (Some(entry.clone()), true)
                }
                None => (None, false),
            },
        );
        match result {
            Ok(Some(entry)) => {
                self.publish(&entry);
                Some(entry.pinned)
            }
            Ok(None) => None,
            Err(e) => {
                log::error!("Failed to save history: {}", e);
                None
            }
        }
    }

    /// Delete one entry (and its image), on every synced device.
    pub fn delete(&self, hash: &str) {
        let result = self.update_if(|history| {
            let before = history.len();
            history.retain(|e| {
                let keep = e.hash != hash;
                if !keep {
                    self.remove_entry_files(e);
                }
                keep
            });
            ((), history.len() != before)
        });
        if let Err(e) = result {
            log::error!("Failed to save history: {}", e);
        }
        if let Some(dir) = &self.sync_dir {
            crate::sync::write_tombstone(dir, hash);
        }
    }

    /// Delete every unpinned entry, on every synced device. Returns how many were removed.
    pub fn clear_unpinned(&self) -> usize {
        let result = self.update_if(|history| {
            let mut removed = Vec::new();
            history.retain(|e| {
                if !e.pinned {
                    self.remove_entry_files(e);
                    removed.push(e.hash.clone());
                }
                e.pinned
            });
            let changed = !removed.is_empty();
            (removed, changed)
        });
        match result {
            Ok(removed) => {
                if let Some(dir) = &self.sync_dir {
                    for hash in &removed {
                        crate::sync::write_tombstone(dir, hash);
                    }
                }
                removed.len()
            }
            Err(e) => {
                log::error!("Failed to save history: {}", e);
                0
            }
        }
    }

    /// Drop the oldest unpinned entries beyond `max_entries`, deleting their
    /// image files. Pinned entries are never dropped, and the newest entry
    /// always stays even when pins fill the whole limit.
    pub(crate) fn prune(&self, history: &mut Vec<ClipEntry>) -> Vec<ClipEntry> {
        let pinned = history.iter().filter(|e| e.pinned).count();
        let limit = self.max_entries.saturating_sub(pinned).max(1);
        let mut kept = 0;
        let mut evicted = Vec::new();
        history.retain(|e| {
            if e.pinned {
                return true;
            }
            kept += 1;
            if kept > limit {
                evicted.push(e.clone());
            }
            kept <= limit
        });
        for entry in &evicted {
            self.remove_entry_files(entry);
        }
        evicted
    }

    /// Delete the image and thumbnail belonging to an entry, if any.
    pub(crate) fn remove_entry_files(&self, entry: &ClipEntry) {
        if let Some(ClipContent::Image { path, .. }) = &entry.content {
            self.remove_image_file(path);
            let _ = fs::remove_file(self.thumbnail_path(&entry.hash));
        }
    }

    /// Delete an image file, but only if it sits directly in our image
    /// directory. Entry paths can come from a shared sync folder or an old
    /// file, so they're never trusted to point somewhere safe.
    fn remove_image_file(&self, path: &Path) {
        let (Ok(file), Ok(dir)) = (path.canonicalize(), self.image_dir.canonicalize()) else {
            return;
        };
        if file.parent() == Some(dir.as_path()) {
            if let Err(e) = fs::remove_file(&file) {
                log::debug!("Failed to remove {}: {}", file.display(), e);
            }
        } else {
            log::warn!(
                "Not deleting {}: it is outside the image directory",
                path.display()
            );
        }
    }

    fn publish(&self, entry: &ClipEntry) {
        if let Some(dir) = &self.sync_dir {
            crate::sync::publish(dir, entry);
        }
    }
}

/// Initialize the global storage from config. Call once at startup.
pub fn init(config: &Config) {
    let _ = STORAGE.set(Storage::from_config(config));
}

/// Get the global storage instance.
pub fn global() -> &'static Storage {
    STORAGE
        .get()
        .expect("Storage not initialized — call storage::init() first")
}

pub fn get_hash(text: &str) -> String {
    get_hash_bytes(text.as_bytes())
}

pub fn get_hash_bytes(data: &[u8]) -> String {
    let digest = md5::compute(data);
    format!("{:x}", digest)[..12].to_string()
}

pub(crate) fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

/// Atomically replace `path` with `data`, readable only by the user.
pub(crate) fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(
        "{}.tmp.{}.{}",
        name,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn backup_path(path: &Path, n: usize) -> PathBuf {
    let name = format!("{}.bak.{}", path.file_name().unwrap().to_string_lossy(), n);
    path.with_file_name(name)
}

/// Load the history, falling back to backups if the main file is corrupt or
/// missing. The flag is false when the main file exists but is corrupt.
fn load_history_from(path: &Path, max_backups: usize) -> (Vec<ClipEntry>, bool) {
    let main_ok = match fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(entries) => return (entries, true),
            Err(e) => {
                log::warn!("Corrupt history file {}: {}", path.display(), e);
                false
            }
        },
        Err(_) => true,
    };

    // Try backups in order (newest first)
    for n in 1..=max_backups {
        let bak = backup_path(path, n);
        if let Ok(content) = fs::read_to_string(&bak) {
            match serde_json::from_str::<Vec<ClipEntry>>(&content) {
                Ok(entries) => {
                    log::warn!("Recovered history from backup {}", bak.display());
                    return (entries, main_ok);
                }
                Err(e) => {
                    log::warn!("Backup {} also corrupt: {}", bak.display(), e);
                }
            }
        }
    }

    if !main_ok {
        log::warn!("All history files corrupt, starting fresh");
    }
    (Vec::new(), main_ok)
}

fn rotate_backups(path: &Path, max_backups: usize) {
    if max_backups == 0 {
        return;
    }
    // Shift .bak.2 → .bak.3, .bak.1 → .bak.2, etc.
    for n in (1..max_backups).rev() {
        let from = backup_path(path, n);
        let to = backup_path(path, n + 1);
        if from.exists() {
            let _ = fs::rename(&from, &to);
        }
    }
    // Copy current main file to .bak.1
    if path.exists() {
        let _ = fs::copy(path, backup_path(path, 1));
    }
    // Backups made by older versions were world-readable.
    for n in 1..=max_backups {
        let _ = fs::set_permissions(backup_path(path, n), fs::Permissions::from_mode(0o600));
    }
}

fn save_history_to(history: &[ClipEntry], path: &Path) -> Result<()> {
    let json = serde_json::to_vec_pretty(history)?;
    write_private(path, &json)
}

// --- Public API using the global storage ---

pub fn load_history() -> Vec<ClipEntry> {
    global().load_history()
}

pub fn process_text(text: &str) {
    global().process_text(text);
}

pub fn process_image(data: &[u8], mime: &str) {
    global().process_image(data, mime);
}

#[cfg(test)]
pub(crate) fn test_storage(dir: &Path) -> Storage {
    let storage = Storage {
        history_path: dir.join("history.json"),
        max_entries: 5,
        max_backups: 3,
        image_dir: dir.join("images"),
        max_image_bytes: 1024 * 1024,
        max_text_bytes: 64 * 1024,
        sync_dir: None,
    };
    storage.ensure_image_dir();
    storage
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Helper to get the text from a ClipEntry for test assertions.
    fn entry_text(entry: &ClipEntry) -> String {
        match entry.resolved_content() {
            ClipContent::Text { text, .. } => text,
            ClipContent::Image { .. } => String::new(),
        }
    }

    fn entry_preview(entry: &ClipEntry) -> String {
        match entry.resolved_content() {
            ClipContent::Text { preview, .. } => preview,
            ClipContent::Image { mime, .. } => mime,
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn test_hash_deterministic() {
        assert_eq!(get_hash("hello"), get_hash("hello"));
    }

    #[test]
    fn test_hash_length_12() {
        assert_eq!(get_hash("test").len(), 12);
    }

    #[test]
    fn test_hash_different_inputs() {
        assert_ne!(get_hash("hello"), get_hash("world"));
    }

    #[test]
    fn test_process_text_adds_entry() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path());
        s.process_text("hello world");
        let h = s.load_history();
        assert_eq!(h.len(), 1);
        assert_eq!(entry_text(&h[0]), "hello world");
    }

    #[test]
    fn test_process_text_ignores_empty() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path());
        s.process_text("");
        s.process_text("   ");
        assert_eq!(s.load_history().len(), 0);
    }

    #[test]
    fn test_text_kept_exactly_as_copied() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path());
        s.process_text("    fn main() {}\n");
        assert_eq!(entry_text(&s.load_history()[0]), "    fn main() {}\n");
    }

    #[test]
    fn test_text_over_limit_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path()); // 64 KB limit
        s.process_text(&"x".repeat(65 * 1024));
        assert!(s.load_history().is_empty());
    }

    #[test]
    fn test_new_entries_dont_duplicate_text_in_legacy_fields() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path());
        s.process_text("only once");
        let raw = fs::read_to_string(&s.history_path).unwrap();
        assert_eq!(
            raw.matches("only once").count(),
            2,
            "text + preview, no legacy copies"
        );
    }

    #[test]
    fn test_legacy_entries_still_load() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path());
        fs::write(
            &s.history_path,
            r#"[{"text":"old","preview":"old","hash":"abcdefabcdef","time":1.0}]"#,
        )
        .unwrap();
        let h = s.load_history();
        assert_eq!(entry_text(&h[0]), "old");
        assert_eq!(h[0].modified(), 1.0);
    }

    #[test]
    fn test_dedup_moves_to_top() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path());
        s.process_text("first");
        s.process_text("second");
        s.process_text("first"); // duplicate
        let h = s.load_history();
        assert_eq!(h.len(), 2);
        assert_eq!(entry_text(&h[0]), "first");
        assert_eq!(entry_text(&h[1]), "second");
    }

    #[test]
    fn test_prune_at_max_entries() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path()); // max_entries = 5
        for i in 0..7 {
            s.process_text(&format!("entry {}", i));
        }
        let h = s.load_history();
        assert_eq!(h.len(), 5);
        // Newest should be first
        assert_eq!(entry_text(&h[0]), "entry 6");
    }

    #[test]
    fn test_prune_protects_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path()); // max_entries = 5
                                          // Add 5 entries
        for i in 0..5 {
            s.process_text(&format!("entry {}", i));
        }
        // Pin the oldest entry (entry 0, which is at position 4)
        let mut h = s.load_history();
        h[4].pinned = true;
        s.save_history(&h).unwrap();

        // Add 2 more — should evict unpinned, keep pinned
        s.process_text("new 1");
        s.process_text("new 2");
        let h = s.load_history();
        // Pinned entry must still be present
        assert!(h.iter().any(|e| entry_text(e) == "entry 0" && e.pinned));
        assert!(h.len() <= 5);
    }

    #[test]
    fn test_new_entry_kept_when_pins_fill_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path()); // max_entries = 5
        for i in 0..5 {
            s.process_text(&format!("pin {}", i));
        }
        for e in s.load_history() {
            s.toggle_pin(&e.hash);
        }
        s.process_text("fresh copy");
        assert!(s
            .load_history()
            .iter()
            .any(|e| entry_text(e) == "fresh copy"));
    }

    #[test]
    fn test_preview_collapses_newlines() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path());
        s.process_text("line1\nline2\nline3");
        let h = s.load_history();
        assert_eq!(entry_preview(&h[0]), "line1 line2 line3");
    }

    #[test]
    fn test_save_creates_backup() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path());
        s.process_text("first save");
        s.process_text("second save");
        let bak1 = backup_path(&s.history_path, 1);
        assert!(
            bak1.exists(),
            "backup .bak.1 should exist after second save"
        );
    }

    #[test]
    fn test_corrupt_json_loads_from_backup() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path());
        // Two saves to create a backup (.bak.1 is made on the second save)
        s.process_text("good data");
        s.process_text("more data");

        // Corrupt the main file
        fs::write(&s.history_path, "NOT VALID JSON").unwrap();

        // Load should recover from backup
        let h = s.load_history();
        assert!(!h.is_empty(), "should recover from backup");
        assert!(h.iter().any(|e| entry_text(e) == "good data"));
    }

    #[test]
    fn test_corrupt_main_is_not_rotated_into_backups() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path());
        s.process_text("good data");
        s.process_text("more data");
        fs::write(&s.history_path, "NOT VALID JSON").unwrap();

        s.process_text("after corruption");
        let bak1 = fs::read_to_string(backup_path(&s.history_path, 1)).unwrap();
        assert!(serde_json::from_str::<Vec<ClipEntry>>(&bak1).is_ok());
        let h = s.load_history();
        assert!(h.iter().any(|e| entry_text(e) == "good data"));
        assert!(h.iter().any(|e| entry_text(e) == "after corruption"));
    }

    #[test]
    fn test_all_corrupt_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path());

        // Write corrupt main file and corrupt backups
        fs::write(&s.history_path, "BAD").unwrap();
        for n in 1..=3 {
            fs::write(backup_path(&s.history_path, n), "BAD").unwrap();
        }

        let h = s.load_history();
        assert!(h.is_empty());
    }

    #[test]
    fn test_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path());
        assert!(s.load_history().is_empty());
    }

    #[test]
    fn test_concurrent_writers_lose_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = test_storage(dir.path());
        s.max_entries = 10_000;
        let s = Arc::new(s);
        let threads: Vec<_> = (0..4)
            .map(|t| {
                let s = s.clone();
                std::thread::spawn(move || {
                    for i in 0..25 {
                        s.process_text(&format!("thread {} copy {}", t, i));
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(s.load_history().len(), 100);
    }

    #[test]
    fn test_files_are_private() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path());
        s.process_text("secret");
        s.process_text("secret 2");
        s.process_image(b"fake png bytes", "image/png");
        assert_eq!(mode(&s.history_path), 0o600);
        assert_eq!(mode(&backup_path(&s.history_path, 1)), 0o600);
        assert_eq!(mode(&s.image_dir), 0o700);
        let img = s.image_path(&get_hash_bytes(b"fake png bytes"), "image/png");
        assert_eq!(mode(&img), 0o600);
    }

    #[test]
    fn test_image_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path()); // 1 MB limit
        s.process_image(&vec![1u8; 2 * 1024 * 1024], "image/png");
        assert!(s.load_history().is_empty());
        s.process_image(&[1u8; 10], "image/png");
        assert_eq!(s.load_history().len(), 1);
    }

    #[test]
    fn test_pruned_image_files_are_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path()); // max_entries = 5
        s.process_image(b"image one", "image/png");
        let img = s.image_path(&get_hash_bytes(b"image one"), "image/png");
        assert!(img.exists());
        for i in 0..5 {
            s.process_text(&format!("text {}", i));
        }
        assert!(!img.exists());
    }

    #[test]
    fn test_never_deletes_files_outside_image_dir() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path());
        let victim = dir.path().join("precious.txt");
        fs::write(&victim, "keep me").unwrap();
        let evil = ClipEntry::new_image(
            victim.clone(),
            "image/png".into(),
            "aaaaaaaaaaaa".into(),
            1.0,
        );
        s.save_history(&[evil]).unwrap();
        s.delete("aaaaaaaaaaaa");
        s.process_image(b"x", "image/png");
        assert!(victim.exists());
        assert!(s.load_history().iter().all(|e| e.hash != "aaaaaaaaaaaa"));
    }

    #[test]
    fn test_toggle_pin_delete_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let s = test_storage(dir.path());
        s.process_text("a");
        s.process_text("b");
        s.process_image(b"img", "image/png");
        let hash_a = get_hash("a");
        let hash_b = get_hash("b");

        assert_eq!(s.toggle_pin(&hash_a), Some(true));
        assert!(
            s.load_history()
                .iter()
                .find(|e| e.hash == hash_a)
                .unwrap()
                .updated
                > 0.0
        );
        assert_eq!(s.toggle_pin("doesnotexist"), None);

        s.delete(&hash_b);
        assert!(s.load_history().iter().all(|e| e.hash != hash_b));

        let img = s.image_path(&get_hash_bytes(b"img"), "image/png");
        assert_eq!(s.clear_unpinned(), 1);
        assert!(!img.exists());
        let h = s.load_history();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].hash, hash_a);
    }

    #[test]
    fn test_make_preview() {
        assert_eq!(make_preview("  a\n\n\tb  c "), "a b c");
        assert_eq!(
            make_preview(&"y".repeat(500)).chars().count(),
            PREVIEW_CHARS
        );
    }
}
