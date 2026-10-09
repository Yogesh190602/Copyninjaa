//! File-based cross-device sync through a shared folder (Syncthing, Nextcloud, …).
//!
//! Layout of the sync folder:
//!   entries/<hash>.json   one file per clipboard entry
//!   images/<hash>.<ext>   image bytes for image entries
//!   deleted/<hash>        tombstone holding the deletion time
//!
//! Anything in the folder may have been written by another machine, so it is
//! treated as untrusted: hashes are validated before they are used in paths,
//! and image paths are always rebuilt locally.

use crate::content::{ext_for_mime, ClipContent, IMAGE_MIMES};
use crate::storage::{self, get_hash_bytes, write_private, ClipEntry, Storage};
use log::{debug, error, info, warn};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, UNIX_EPOCH};

/// Tombstones older than this are removed from the sync folder.
const TOMBSTONE_TTL_SECS: f64 = 30.0 * 24.0 * 3600.0;

/// Hashes are the first 12 hex digits of an MD5 sum.
pub fn is_valid_hash(s: &str) -> bool {
    s.len() == 12 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn entry_file(sync_dir: &Path, hash: &str) -> PathBuf {
    sync_dir.join("entries").join(format!("{}.json", hash))
}

fn image_file(sync_dir: &Path, hash: &str, mime: &str) -> PathBuf {
    sync_dir
        .join("images")
        .join(format!("{}.{}", hash, ext_for_mime(mime)))
}

fn tombstone_file(sync_dir: &Path, hash: &str) -> PathBuf {
    sync_dir.join("deleted").join(hash)
}

fn read_entry(path: &Path) -> Option<ClipEntry> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn write_file(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_private(path, data)
}

/// Publish a local entry (new copy, copied again, pinned/unpinned) to the sync folder.
pub fn publish(sync_dir: &Path, entry: &ClipEntry) {
    if !is_valid_hash(&entry.hash) {
        return;
    }
    let path = entry_file(sync_dir, &entry.hash);
    if read_entry(&path).is_some_and(|existing| existing.modified() >= entry.modified()) {
        return;
    }

    let mut shared = entry.clone();
    shared.text = None;
    shared.preview = None;
    shared.content = Some(match entry.resolved_content() {
        ClipContent::Image { path: local, mime } => {
            let dest = image_file(sync_dir, &entry.hash, &mime);
            if !dest.exists() {
                let copied = fs::read(&local)
                    .map_err(anyhow::Error::from)
                    .and_then(|data| write_file(&dest, &data));
                if let Err(e) = copied {
                    warn!("Sync: cannot export image {}: {}", local.display(), e);
                    return;
                }
            }
            // Don't leak this machine's file layout into the shared folder.
            let name = format!("{}.{}", entry.hash, ext_for_mime(&mime));
            ClipContent::Image {
                path: PathBuf::from(name),
                mime,
            }
        }
        text => text,
    });

    // Copying something again after it was deleted brings it back.
    let tombstone = tombstone_file(sync_dir, &entry.hash);
    if read_tombstone_time(&tombstone).is_some_and(|t| t < entry.modified()) {
        let _ = fs::remove_file(&tombstone);
    }

    match serde_json::to_vec_pretty(&shared) {
        Ok(json) => {
            if let Err(e) = write_file(&path, &json) {
                warn!("Sync: cannot export entry {}: {}", entry.hash, e);
            }
        }
        Err(e) => warn!("Sync: cannot serialize entry {}: {}", entry.hash, e),
    }
}

/// Write a tombstone so other devices delete this entry too.
pub fn write_tombstone(sync_dir: &Path, hash: &str) {
    if !is_valid_hash(hash) {
        warn!("Sync: refusing tombstone for invalid hash {:?}", hash);
        return;
    }
    let time = storage::now().to_string();
    if let Err(e) = write_file(&tombstone_file(sync_dir, hash), time.as_bytes()) {
        warn!("Sync: cannot write tombstone for {}: {}", hash, e);
    }
    remove_shared_entry(sync_dir, hash);
}

fn remove_shared_entry(sync_dir: &Path, hash: &str) {
    let _ = fs::remove_file(entry_file(sync_dir, hash));
    for mime in IMAGE_MIMES {
        let _ = fs::remove_file(image_file(sync_dir, hash, mime));
    }
}

fn read_tombstone_time(path: &Path) -> Option<f64> {
    let content = fs::read_to_string(path).ok()?;
    content.trim().parse::<f64>().ok().or_else(|| {
        // Tombstones from older versions are empty: use the file's mtime.
        let modified = fs::metadata(path).ok()?.modified().ok()?;
        Some(modified.duration_since(UNIX_EPOCH).ok()?.as_secs_f64())
    })
}

fn read_tombstones(sync_dir: &Path) -> HashMap<String, f64> {
    let mut tombstones = HashMap::new();
    if let Ok(dir) = fs::read_dir(sync_dir.join("deleted")) {
        for item in dir.flatten() {
            let Some(name) = item.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !is_valid_hash(&name) {
                continue;
            }
            if let Some(time) = read_tombstone_time(&item.path()) {
                tombstones.insert(name, time);
            }
        }
    }
    tombstones
}

/// Read and validate every entry in the sync folder.
fn read_remote_entries(storage: &Storage, sync_dir: &Path) -> Vec<ClipEntry> {
    let Ok(dir) = fs::read_dir(sync_dir.join("entries")) else {
        return Vec::new();
    };
    // JSON escaping can blow text up several times; this only guards against absurd files.
    let max_file_bytes = storage.max_text_bytes as u64 * 6 + 64 * 1024;

    let mut entries = Vec::new();
    for item in dir.flatten() {
        let path = item.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !is_valid_hash(stem) {
            debug!("Sync: skipping {} (not an entry name)", path.display());
            continue;
        }
        if item.metadata().map_or(true, |m| m.len() > max_file_bytes) {
            debug!("Sync: skipping {} (too large)", path.display());
            continue;
        }
        let Some(entry) = read_entry(&path) else {
            debug!("Sync: skipping unreadable entry {}", path.display());
            continue;
        };
        if entry.hash != stem {
            debug!(
                "Sync: skipping {} (hash doesn't match file name)",
                path.display()
            );
            continue;
        }
        let usable = match entry.resolved_content() {
            ClipContent::Text { text, .. } => {
                !text.trim().is_empty() && text.len() <= storage.max_text_bytes
            }
            ClipContent::Image { mime, .. } => IMAGE_MIMES.contains(&mime.as_str()),
        };
        if usable {
            entries.push(entry);
        }
    }
    entries
}

/// Copy the bytes of a newly imported image entry into the local image
/// directory. Returns false if they aren't available (yet) or don't match.
fn fetch_image(storage: &Storage, sync_dir: &Path, entry: &ClipEntry) -> bool {
    let Some(ClipContent::Image { path, mime }) = &entry.content else {
        return true;
    };
    if path.exists() {
        return true;
    }
    let src = image_file(sync_dir, &entry.hash, mime);
    match fs::metadata(&src) {
        Ok(meta) if meta.len() as usize <= storage.max_image_bytes => {}
        Ok(_) => return false,
        Err(_) => {
            debug!("Sync: image for {} hasn't arrived yet", entry.hash);
            return false;
        }
    }
    let Ok(data) = fs::read(&src) else {
        return false;
    };
    if get_hash_bytes(&data) != entry.hash {
        warn!(
            "Sync: image {} doesn't match its hash, ignoring",
            src.display()
        );
        return false;
    }
    storage.ensure_image_dir();
    match write_private(path, &data) {
        Ok(()) => true,
        Err(e) => {
            warn!("Sync: cannot store image {}: {}", path.display(), e);
            false
        }
    }
}

/// Merge entries and deletions from the sync folder into local history.
pub fn import_from_sync_dir(storage: &Storage, sync_dir: &Path) {
    let tombstones = read_tombstones(sync_dir);
    let remote = read_remote_entries(storage, sync_dir);
    let is_deleted =
        |hash: &str, modified: f64| tombstones.get(hash).is_some_and(|&t| t >= modified);

    let result = storage.update_if(|history| {
        let before = fingerprint(history);

        // Deletions made on other devices.
        history.retain(|e| {
            let deleted = is_deleted(&e.hash, e.modified());
            if deleted {
                storage.remove_entry_files(e);
            }
            !deleted
        });

        let mut added = HashSet::new();
        for r in &remote {
            if is_deleted(&r.hash, r.modified()) {
                continue;
            }
            match history.iter_mut().find(|e| e.hash == r.hash) {
                // The newer state wins (copied again, pinned, unpinned).
                Some(local) => {
                    if r.modified() > local.modified() {
                        local.time = r.time;
                        local.updated = r.updated;
                        local.pinned = r.pinned;
                    }
                }
                None => {
                    let mut entry = r.clone();
                    if let ClipContent::Image { mime, .. } = r.resolved_content() {
                        entry.content = Some(ClipContent::Image {
                            path: storage.image_path(&r.hash, &mime),
                            mime,
                        });
                    }
                    added.insert(r.hash.clone());
                    history.push(entry);
                }
            }
        }

        history.sort_by(|a, b| b.time.total_cmp(&a.time));
        storage.prune(history);
        // Only fetch image bytes for new entries that survived pruning.
        history.retain(|e| !added.contains(&e.hash) || fetch_image(storage, sync_dir, e));

        let imported = history.iter().filter(|e| added.contains(&e.hash)).count();
        (imported, fingerprint(history) != before)
    });

    match result {
        Ok(imported) if imported > 0 => info!("Imported {} entries from sync dir", imported),
        Ok(_) => {}
        Err(e) => error!("Failed to save after sync import: {}", e),
    }

    collect_garbage(storage, sync_dir, &remote, &tombstones);
}

/// What sync can change about the history, to tell whether a save is needed.
fn fingerprint(history: &[ClipEntry]) -> Vec<(String, u64, u64, bool)> {
    history
        .iter()
        .map(|e| {
            (
                e.hash.clone(),
                e.time.to_bits(),
                e.updated.to_bits(),
                e.pinned,
            )
        })
        .collect()
}

/// Keep the sync folder from growing forever: drop deleted entries, entries
/// older than everything we keep while our history is full (every device with
/// the same limit would prune them too), and tombstones older than 30 days.
fn collect_garbage(
    storage: &Storage,
    sync_dir: &Path,
    remote: &[ClipEntry],
    tombstones: &HashMap<String, f64>,
) {
    let history = storage.load_history();
    let local: HashSet<&str> = history.iter().map(|e| e.hash.as_str()).collect();
    let unpinned: Vec<&ClipEntry> = history.iter().filter(|e| !e.pinned).collect();
    let limit = storage
        .max_entries
        .saturating_sub(history.len() - unpinned.len())
        .max(1);
    let oldest_kept = if unpinned.len() >= limit {
        unpinned
            .iter()
            .map(|e| e.time)
            .fold(f64::INFINITY, f64::min)
    } else {
        f64::NEG_INFINITY
    };

    for r in remote {
        let deleted = tombstones.get(&r.hash).is_some_and(|&t| t >= r.modified());
        let expired = !r.pinned && !local.contains(r.hash.as_str()) && r.modified() < oldest_kept;
        if deleted || expired {
            remove_shared_entry(sync_dir, &r.hash);
        }
    }

    let cutoff = storage::now() - TOMBSTONE_TTL_SECS;
    for (hash, &time) in tombstones {
        if time < cutoff {
            let _ = fs::remove_file(tombstone_file(sync_dir, hash));
        }
    }
}

/// Publish every local entry that isn't in the sync folder yet.
pub fn export_to_sync_dir(storage: &Storage, sync_dir: &Path) {
    let history = storage.load_history();
    for entry in &history {
        publish(sync_dir, entry);
    }
    debug!("Exported {} entries to sync dir", history.len());
}

/// Start watching the sync directory for changes from other devices.
/// Runs in a background thread, calls import when changes detected.
pub fn start_watcher(storage: &'static Storage) {
    let Some(sync_dir) = storage.sync_dir.clone() else {
        return;
    };

    std::thread::spawn(move || {
        let dirs = ["entries", "images", "deleted"].map(|d| sync_dir.join(d));
        for dir in &dirs {
            if let Err(e) = fs::create_dir_all(dir) {
                error!("Cannot create sync dir {}: {}", dir.display(), e);
                return;
            }
        }

        // Catch up with changes made while we weren't running.
        import_from_sync_dir(storage, &sync_dir);
        export_to_sync_dir(storage, &sync_dir);

        let (tx, rx) = mpsc::channel();
        let mut watcher = match RecommendedWatcher::new(
            move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                        let _ = tx.send(());
                    }
                }
            },
            notify::Config::default(),
        ) {
            Ok(w) => w,
            Err(e) => {
                error!("Failed to create sync watcher: {}", e);
                return;
            }
        };

        for dir in &dirs {
            if let Err(e) = watcher.watch(dir, RecursiveMode::NonRecursive) {
                error!("Failed to watch sync dir {}: {}", dir.display(), e);
                return;
            }
        }
        info!("Sync watcher active on {}", sync_dir.display());

        // Debounce: wait for changes, then import after a short delay
        while rx.recv().is_ok() {
            std::thread::sleep(Duration::from_millis(500));
            while rx.try_recv().is_ok() {}

            debug!("Sync dir changed, importing");
            import_from_sync_dir(storage, &sync_dir);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{get_hash, test_storage};

    struct TwoDevices {
        _tmp: tempfile::TempDir,
        a: Storage,
        b: Storage,
        sync: PathBuf,
    }

    fn two_devices(max_entries: usize) -> TwoDevices {
        let tmp = tempfile::tempdir().unwrap();
        let sync = tmp.path().join("sync");
        let mut a = test_storage(&tmp.path().join("a"));
        let mut b = test_storage(&tmp.path().join("b"));
        for s in [&mut a, &mut b] {
            s.sync_dir = Some(sync.clone());
            s.max_entries = max_entries;
        }
        TwoDevices {
            _tmp: tmp,
            a,
            b,
            sync,
        }
    }

    fn texts(s: &Storage) -> Vec<String> {
        s.load_history()
            .iter()
            .map(|e| match e.resolved_content() {
                ClipContent::Text { text, .. } => text,
                ClipContent::Image { mime, .. } => mime,
            })
            .collect()
    }

    fn count(dir: &Path) -> usize {
        fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
    }

    #[test]
    fn new_copies_are_published_immediately() {
        let d = two_devices(5);
        d.a.process_text("hello");
        assert!(entry_file(&d.sync, &get_hash("hello")).exists());
        import_from_sync_dir(&d.b, &d.sync);
        assert_eq!(texts(&d.b), vec!["hello"]);
    }

    #[test]
    fn import_respects_max_entries_and_converges() {
        let mut d = two_devices(3);
        d.a.max_entries = 100;
        for i in 0..6 {
            d.a.process_text(&format!("remote {}", i));
        }
        d.b.process_text("local");
        import_from_sync_dir(&d.b, &d.sync);
        assert_eq!(d.b.load_history().len(), 3);
        // Importing again changes nothing and old entries don't come back.
        let before = texts(&d.b);
        import_from_sync_dir(&d.b, &d.sync);
        assert_eq!(texts(&d.b), before);
        // The folder is trimmed to what devices keep.
        assert!(
            count(&d.sync.join("entries")) <= 3,
            "{}",
            count(&d.sync.join("entries"))
        );
    }

    #[test]
    fn deletions_reach_other_devices_and_stay_deleted() {
        let d = two_devices(5);
        d.a.process_text("doomed");
        d.a.process_text("kept");
        import_from_sync_dir(&d.b, &d.sync);
        assert_eq!(d.b.load_history().len(), 2);

        d.a.delete(&get_hash("doomed"));
        import_from_sync_dir(&d.b, &d.sync);
        assert_eq!(texts(&d.b), vec!["kept"]);

        // B's next startup export must not resurrect it.
        export_to_sync_dir(&d.b, &d.sync);
        assert!(!entry_file(&d.sync, &get_hash("doomed")).exists());
    }

    #[test]
    fn copying_again_after_delete_brings_it_back() {
        let d = two_devices(5);
        d.a.process_text("again");
        d.a.delete(&get_hash("again"));
        std::thread::sleep(Duration::from_millis(5));
        d.b.process_text("again");
        import_from_sync_dir(&d.b, &d.sync);
        assert_eq!(texts(&d.b), vec!["again"]);
        assert!(!tombstone_file(&d.sync, &get_hash("again")).exists());
        import_from_sync_dir(&d.a, &d.sync);
        assert_eq!(texts(&d.a), vec!["again"]);
    }

    #[test]
    fn clear_all_reaches_other_devices() {
        let d = two_devices(5);
        d.a.process_text("one");
        d.a.process_text("two");
        import_from_sync_dir(&d.b, &d.sync);
        d.a.clear_unpinned();
        import_from_sync_dir(&d.b, &d.sync);
        assert!(d.b.load_history().is_empty());
    }

    #[test]
    fn pins_sync_both_ways() {
        let d = two_devices(5);
        d.a.process_text("pin me");
        import_from_sync_dir(&d.b, &d.sync);
        let hash = get_hash("pin me");

        std::thread::sleep(Duration::from_millis(5));
        d.a.toggle_pin(&hash);
        import_from_sync_dir(&d.b, &d.sync);
        assert!(d.b.load_history()[0].pinned);

        std::thread::sleep(Duration::from_millis(5));
        d.b.toggle_pin(&hash);
        import_from_sync_dir(&d.a, &d.sync);
        assert!(!d.a.load_history()[0].pinned);
    }

    #[test]
    fn images_are_synced_with_their_bytes() {
        let d = two_devices(5);
        d.a.process_image(b"png bytes", "image/png");
        let hash = get_hash_bytes(b"png bytes");
        let shared = read_entry(&entry_file(&d.sync, &hash)).unwrap();
        match shared.content.unwrap() {
            ClipContent::Image { path, .. } => assert!(path.is_relative(), "{:?}", path),
            _ => panic!("expected image"),
        }

        import_from_sync_dir(&d.b, &d.sync);
        let local = d.b.image_path(&hash, "image/png");
        assert_eq!(fs::read(&local).unwrap(), b"png bytes");
        match d.b.load_history()[0].resolved_content() {
            ClipContent::Image { path, .. } => assert_eq!(path, local),
            _ => panic!("expected image"),
        }
    }

    #[test]
    fn image_entry_waits_for_its_bytes() {
        let d = two_devices(5);
        d.a.process_image(b"late image", "image/png");
        let hash = get_hash_bytes(b"late image");
        let img = image_file(&d.sync, &hash, "image/png");
        let parked = d.sync.join("parked");
        fs::rename(&img, &parked).unwrap();

        import_from_sync_dir(&d.b, &d.sync);
        assert!(d.b.load_history().is_empty());

        fs::rename(&parked, &img).unwrap();
        import_from_sync_dir(&d.b, &d.sync);
        assert_eq!(d.b.load_history().len(), 1);
    }

    #[test]
    fn hostile_sync_folder_cannot_touch_other_files() {
        let d = two_devices(5);
        let victim = d._tmp.path().join("victim.txt");
        fs::write(&victim, "important").unwrap();

        // Path traversal through the hash.
        write_tombstone(&d.sync, "../../victim.txt");
        assert_eq!(fs::read_to_string(&victim).unwrap(), "important");

        // An image entry pointing at an arbitrary file.
        let evil_hash = "0123456789ab";
        let evil = ClipEntry::new_image(
            victim.clone(),
            "image/png".into(),
            evil_hash.into(),
            storage::now(),
        );
        write_file(
            &entry_file(&d.sync, evil_hash),
            &serde_json::to_vec(&evil).unwrap(),
        )
        .unwrap();
        write_file(
            &image_file(&d.sync, evil_hash, "image/png"),
            b"not matching",
        )
        .unwrap();
        // An entry whose hash isn't its file name.
        let mut sneaky = ClipEntry::new_text("x".into(), "../../../x".into(), storage::now());
        sneaky.pinned = true;
        write_file(
            &d.sync.join("entries/aaaaaaaaaaaa.json"),
            &serde_json::to_vec(&sneaky).unwrap(),
        )
        .unwrap();

        import_from_sync_dir(&d.b, &d.sync);
        assert!(d.b.load_history().is_empty());
        d.b.clear_unpinned();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "important");
    }

    #[test]
    fn legacy_tombstones_still_work() {
        let d = two_devices(5);
        d.b.process_text("old delete");
        let hash = get_hash("old delete");
        std::thread::sleep(Duration::from_millis(20));
        // Empty file, as written by older versions: its mtime is the deletion time.
        write_file(&tombstone_file(&d.sync, &hash), b"").unwrap();
        import_from_sync_dir(&d.b, &d.sync);
        assert!(d.b.load_history().is_empty());
    }

    #[test]
    fn valid_hashes() {
        assert!(is_valid_hash(&get_hash("x")));
        assert!(!is_valid_hash("../../etc/pw"));
        assert!(!is_valid_hash("ABCDEF123456"));
        assert!(!is_valid_hash("abc"));
    }
}
