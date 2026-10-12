//! Retention uses the real local footprint, including SQLite's WAL and caches.
use crate::{
    clipboard,
    db::{now_ms, Db, Item},
    settings::{Retention, SettingsStore},
};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};

const MB: u64 = 1024 * 1024;
const DAY: i64 = 86_400_000;

#[derive(Debug, Serialize)]
pub struct Usage {
    pub bytes: u64,
    pub count: u64,
}

#[derive(Serialize)]
pub struct CleanupResult {
    pub removed: u64,
    pub usage: Usage,
}

fn root(conn: &Connection) -> Result<PathBuf, String> {
    conn.path()
        .and_then(|p| Path::new(p).parent())
        .map(Path::to_path_buf)
        .ok_or_else(|| "데이터 저장 위치를 찾을 수 없어요.".into())
}

fn file_bytes(path: &Path) -> Result<u64, String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => Ok(if meta.is_file() { meta.len() } else { 0 }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e.to_string()),
    }
}

fn cache_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for name in ["images", "icons"] {
        let directory = root.join(name);
        match std::fs::symlink_metadata(&directory) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("캐시 폴더가 다른 위치로 연결되어 있어 정리할 수 없어요.".into())
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
        }
        for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map_err(|e| e.to_string())?.is_file() {
                paths.push(entry.path());
            }
        }
    }
    Ok(paths)
}

fn usage(conn: &Connection) -> Result<Usage, String> {
    let directory = root(conn)?;
    let mut bytes = 0;
    for name in [
        "pastel.db",
        "pastel.db-wal",
        "pastel.db-shm",
        "settings.json",
    ] {
        bytes += file_bytes(&directory.join(name))?;
    }
    for path in cache_files(&directory)? {
        bytes += file_bytes(&path)?;
    }
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    Ok(Usage {
        bytes,
        count: count as u64,
    })
}

struct StoredItem {
    id: i64,
    created_at: i64,
    record_bytes: u64,
    image: Option<String>,
    og_image: Option<String>,
    app_icon: Option<String>,
}

fn items(conn: &Connection) -> Result<Vec<StoredItem>, String> {
    // Do not load all copied text into memory during each maintenance pass.
    let mut statement = conn
        .prepare(
            "SELECT id, created_at, CASE WHEN kind = 'image' THEN content END, og_image, app_icon,
         length(CAST(content AS BLOB)) + length(CAST(preview AS BLOB)) + length(hash)
         + length(CAST(COALESCE(og_title, '') AS BLOB)) + 128
         FROM items ORDER BY created_at ASC, id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(StoredItem {
                id: row.get(0)?,
                created_at: row.get(1)?,
                image: row.get(2)?,
                og_image: row.get(3)?,
                app_icon: row.get(4)?,
                record_bytes: row.get::<_, i64>(5)? as u64,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(|e| e.to_string())
}

// Never remove the original files referenced by a `files` history entry.
// Only owned cache files may be deleted, and shared references are respected.
pub fn remove_item_images(conn: &Connection, item: &Item) {
    let Ok(directory) = root(conn) else { return };
    let image_directory = directory.join("images");
    if std::fs::symlink_metadata(&image_directory).is_ok_and(|m| m.file_type().is_symlink()) {
        return;
    }
    let Ok(image_directory) = std::fs::canonicalize(image_directory) else {
        return;
    };
    let Ok(mut statement) = conn.prepare(
        "SELECT content FROM items WHERE kind = 'image' UNION SELECT og_image FROM items WHERE og_image IS NOT NULL"
    ) else { return };
    let Ok(paths) = statement.query_map([], |row| row.get::<_, String>(0)) else {
        return;
    };
    let mut referenced = HashSet::new();
    for path in paths {
        let Ok(path) = path else { return };
        match std::fs::canonicalize(path) {
            Ok(path) => {
                referenced.insert(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return,
        }
    }
    for path in item
        .og_image
        .iter()
        .map(String::as_str)
        .chain((item.kind == "image").then_some(item.content.as_str()))
    {
        let path = Path::new(path);
        let Ok(canonical) = std::fs::canonicalize(path) else {
            continue;
        };
        if canonical.parent() != Some(image_directory.as_path()) {
            continue;
        }
        if !referenced.contains(&canonical) {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn clean_unused(conn: &Connection, remaining: &[StoredItem]) -> Result<(), String> {
    let mut referenced = HashSet::new();
    for item in remaining {
        for path in item
            .image
            .iter()
            .chain(&item.og_image)
            .chain(&item.app_icon)
        {
            // SQLite resolves directory aliases (e.g. /var -> /private/var
            // on macOS). Compare real paths on both sides before deletion.
            match std::fs::canonicalize(path) {
                Ok(path) => {
                    referenced.insert(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
    }
    for path in cache_files(&root(conn)?)? {
        let canonical = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
        if !referenced.contains(&canonical) {
            std::fs::remove_file(path)
                .map_err(|e| format!("저장 파일을 정리하지 못했어요: {e}"))?;
        }
    }
    Ok(())
}

fn compact(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|e| e.to_string())
}

fn remove_rows(conn: &mut Connection, removed: &[StoredItem]) -> Result<(), String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for item in removed {
        tx.execute("DELETE FROM items WHERE id = ?1", params![item.id])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}

// Caller holds the file-operation lock for the entire snapshot/delete/compact
// sequence, so a concurrently recopied image cannot lose its file.
fn enforce(conn: &mut Connection, policy: &Retention, now: i64) -> Result<CleanupResult, String> {
    policy.validate()?;
    if !policy.days_enabled && !policy.size_enabled {
        return Ok(CleanupResult {
            removed: 0,
            usage: usage(conn)?,
        });
    }
    let mut remaining = items(conn)?;
    let mut removed = 0;
    if policy.days_enabled {
        let cutoff = now - i64::from(policy.days) * DAY;
        let expired = remaining.partition_point(|item| item.created_at < cutoff);
        if expired > 0 {
            remove_rows(conn, &remaining[..expired])?;
            remaining.drain(..expired);
            removed += expired as u64;
        }
    }
    clean_unused(conn, &remaining)?;
    if removed > 0 {
        compact(conn)?;
    }
    let limit = u64::from(policy.max_mb) * MB;
    let mut current = usage(conn)?;
    if policy.size_enabled && current.bytes > limit {
        // First reclaim free database pages/WAL without sacrificing history.
        compact(conn)?;
        current = usage(conn)?;
        while current.bytes > limit && !remaining.is_empty() {
            let mut estimate = 0;
            let mut count = 0;
            let mut files = HashSet::new();
            for item in &remaining {
                // An estimate only batches work; actual disk usage is checked
                // again after compaction before deciding whether to continue.
                estimate += item.record_bytes;
                for path in item
                    .og_image
                    .iter()
                    .map(String::as_str)
                    .chain(item.image.iter().map(String::as_str))
                {
                    if files.insert(path) {
                        estimate += file_bytes(Path::new(path))?;
                    }
                }
                count += 1;
                if estimate >= current.bytes - limit {
                    break;
                }
            }
            remove_rows(conn, &remaining[..count])?;
            remaining.drain(..count);
            removed += count as u64;
            clean_unused(conn, &remaining)?;
            compact(conn)?;
            current = usage(conn)?;
        }
    }
    Ok(CleanupResult {
        removed,
        usage: current,
    })
}

#[tauri::command]
pub async fn storage_usage(app: AppHandle) -> Result<Usage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<Db>();
        let _files = db.1.lock().unwrap();
        let result = usage(&db.0.lock().unwrap());
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_retention(app: AppHandle, retention: Retention) -> Result<CleanupResult, String> {
    retention.validate()?;
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<Db>();
        let _files = db.1.lock().unwrap();
        let store = app.state::<SettingsStore>();
        store.update(|settings| settings.retention = retention.clone())?;
        let result = enforce(&mut db.0.lock().unwrap(), &retention, now_ms());
        let _ = app.emit(clipboard::CHANGED_EVENT, ());
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

pub fn clear(db: &Db) -> Result<(), String> {
    // No new capture may sneak in between deletion and cache cleanup.
    let _files = db.1.lock().unwrap();
    let mut conn = db.0.lock().unwrap();
    let all = items(&conn)?;
    remove_rows(&mut conn, &all)?;
    clean_unused(&conn, &[])?;
    compact(&conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewItem;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        db: Option<Db>,
        directory: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "pinchy-retention-{}-{}-{}",
                std::process::id(),
                now_ms(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(directory.join("images")).unwrap();
            std::fs::create_dir_all(directory.join("icons")).unwrap();
            Self {
                db: Some(Db::open(&directory.join("pastel.db")).unwrap()),
                directory,
            }
        }
        fn db(&self) -> &Db {
            self.db.as_ref().unwrap()
        }
        fn add(&self, hash: &str, timestamp: i64, image_bytes: usize) {
            let path = self.directory.join("images").join(format!("{hash}.png"));
            if image_bytes > 0 {
                std::fs::write(&path, vec![1; image_bytes]).unwrap();
            }
            self.db()
                .upsert(NewItem {
                    kind: if image_bytes > 0 { "image" } else { "text" }.into(),
                    content: if image_bytes > 0 {
                        path.to_string_lossy().into_owned()
                    } else {
                        "한글 기록".into()
                    },
                    preview: "기록".into(),
                    hash: hash.into(),
                    source_app: None,
                    app_icon: None,
                    app_color: None,
                    width: None,
                    height: None,
                    size: image_bytes as i64,
                })
                .unwrap();
            self.db()
                .0
                .lock()
                .unwrap()
                .execute(
                    "UPDATE items SET created_at = ?1 WHERE hash = ?2",
                    params![timestamp, hash],
                )
                .unwrap();
        }
        fn run(&self, policy: Retention, now: i64) -> CleanupResult {
            enforce(&mut self.db().0.lock().unwrap(), &policy, now).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            drop(self.db.take());
            std::fs::remove_dir_all(&self.directory).unwrap();
        }
    }
    fn unlimited() -> Retention {
        Retention {
            days_enabled: false,
            size_enabled: false,
            ..Retention::default()
        }
    }

    #[test]
    fn disabled_conditions_preserve_old_and_large_records() {
        let f = Fixture::new();
        f.add("old", 0, 2 * MB as usize);
        let result = f.run(
            Retention {
                max_mb: 1,
                ..unlimited()
            },
            100 * DAY,
        );
        assert_eq!(result.removed, 0);
        assert_eq!(result.usage.count, 1);
        assert!(result.usage.bytes > MB);
    }

    #[test]
    fn age_boundary_and_recopy_use_last_copied_time() {
        let f = Fixture::new();
        f.add("expired", 69 * DAY, 100);
        f.add("boundary", 70 * DAY, 0);
        f.add("recopied", DAY, 0);
        f.add("recopied", 99 * DAY, 0);
        let result = f.run(
            Retention {
                days_enabled: true,
                ..unlimited()
            },
            100 * DAY,
        );
        assert_eq!(result.removed, 1);
        assert_eq!(result.usage.count, 2);
        assert!(!f.directory.join("images/expired.png").exists());
    }

    #[test]
    fn size_removes_oldest_and_reclaims_real_disk_space() {
        let f = Fixture::new();
        f.add("old", DAY, 700_000);
        f.add("new", 2 * DAY, 700_000);
        let result = f.run(
            Retention {
                size_enabled: true,
                max_mb: 1,
                ..unlimited()
            },
            3 * DAY,
        );
        assert_eq!(result.removed, 1);
        assert!(result.usage.bytes <= MB);
        assert_eq!(result.usage.count, 1);
        assert!(f.directory.join("images/new.png").exists());
        assert!(!f.directory.join("images/old.png").exists());
    }

    #[test]
    fn combined_conditions_apply_independently_and_large_single_item_is_removed() {
        let f = Fixture::new();
        f.add("expired", DAY, 10);
        f.add("large", 99 * DAY, 2 * MB as usize);
        let result = f.run(
            Retention {
                max_mb: 1,
                ..Retention::default()
            },
            100 * DAY,
        );
        assert_eq!(result.removed, 2);
        assert_eq!(result.usage.count, 0);
        assert!(result.usage.bytes <= MB);
    }

    #[test]
    fn usage_counts_utf8_database_and_caches_not_original_file_sizes() {
        let f = Fixture::new();
        let original = f.directory.join("original.txt");
        std::fs::write(&original, vec![0; 2 * MB as usize]).unwrap();
        f.add("files", 0, 0);
        {
            let conn = f.db().0.lock().unwrap();
            conn.execute(
                "UPDATE items SET kind = 'files', content = ?1, size = 1",
                params![serde_json::to_string(&vec![&original]).unwrap()],
            )
            .unwrap();
            assert!(usage(&conn).unwrap().bytes < MB);
        }
        let result = f.run(
            Retention {
                days_enabled: true,
                ..unlimited()
            },
            100 * DAY,
        );
        assert_eq!(result.removed, 1);
        assert!(original.exists());
    }

    #[test]
    fn shared_preview_survives_until_last_reference_and_orphans_are_cleaned() {
        let f = Fixture::new();
        f.add("old", DAY, 0);
        f.add("new", 99 * DAY, 0);
        let preview = f.directory.join("images/shared.png");
        let orphan = f.directory.join("images/orphan.png");
        let icon = f.directory.join("icons/unused.png");
        for path in [&preview, &orphan, &icon] {
            std::fs::write(path, b"cached").unwrap();
        }
        f.db()
            .0
            .lock()
            .unwrap()
            .execute(
                "UPDATE items SET og_image = ?1",
                params![preview.to_string_lossy()],
            )
            .unwrap();
        f.run(
            Retention {
                days_enabled: true,
                ..unlimited()
            },
            100 * DAY,
        );
        assert!(preview.exists());
        assert!(!orphan.exists());
        assert!(!icon.exists());
        f.run(
            Retention {
                days_enabled: true,
                ..unlimited()
            },
            200 * DAY,
        );
        assert!(!preview.exists());
    }

    #[test]
    fn free_database_pages_are_reclaimed_before_deleting_live_history() {
        let f = Fixture::new();
        f.add("keep", DAY, 0);
        {
            let conn = f.db().0.lock().unwrap();
            conn.execute_batch("CREATE TABLE temporary_data (data BLOB); INSERT INTO temporary_data VALUES (zeroblob(2000000)); DROP TABLE temporary_data;").unwrap();
        }
        let result = f.run(
            Retention {
                size_enabled: true,
                max_mb: 1,
                ..unlimited()
            },
            2 * DAY,
        );
        assert_eq!(result.removed, 0);
        assert_eq!(result.usage.count, 1);
        assert!(result.usage.bytes <= MB);
    }

    #[test]
    fn invalid_policy_does_not_delete_any_history() {
        let f = Fixture::new();
        f.add("old", 0, 0);
        let policy = Retention {
            days: 0,
            ..Retention::default()
        };
        assert!(enforce(&mut f.db().0.lock().unwrap(), &policy, 100 * DAY).is_err());
        assert_eq!(f.db().list("", 10).unwrap().len(), 1);
    }

    #[test]
    fn different_spellings_of_the_same_cache_path_are_preserved() {
        let f = Fixture::new();
        f.add("keep", 99 * DAY, 100);
        let alias = f
            .directory
            .join("images")
            .join("..")
            .join("images/keep.png");
        f.db()
            .0
            .lock()
            .unwrap()
            .execute(
                "UPDATE items SET content = ?1",
                params![alias.to_string_lossy()],
            )
            .unwrap();
        let result = f.run(
            Retention {
                days_enabled: true,
                ..unlimited()
            },
            100 * DAY,
        );
        assert_eq!(result.removed, 0);
        assert!(f.directory.join("images/keep.png").exists());
    }

    #[test]
    fn individual_delete_preserves_shared_alias_and_removes_last_reference() {
        let f = Fixture::new();
        f.add("first", DAY, 100);
        f.add("second", 2 * DAY, 0);
        let alias = f.directory.join("images/../images/first.png");
        f.db()
            .0
            .lock()
            .unwrap()
            .execute(
                "UPDATE items SET kind = 'image', content = ?1 WHERE hash = 'second'",
                params![alias.to_string_lossy()],
            )
            .unwrap();
        let records = f.db().list("", 10).unwrap();
        f.db().delete(records[1].id).unwrap();
        assert!(alias.exists());
        f.db().delete(records[0].id).unwrap();
        assert!(!alias.exists());
    }

    #[cfg(unix)]
    #[test]
    fn directory_symlink_alias_does_not_delete_referenced_images() {
        let f = Fixture::new();
        f.add("keep", 99 * DAY, 100);
        std::os::unix::fs::symlink(&f.directory, f.directory.join("alias")).unwrap();
        let alias = f.directory.join("alias/images/keep.png");
        f.db()
            .0
            .lock()
            .unwrap()
            .execute(
                "UPDATE items SET content = ?1",
                params![alias.to_string_lossy()],
            )
            .unwrap();
        let result = f.run(
            Retention {
                days_enabled: true,
                ..unlimited()
            },
            100 * DAY,
        );
        assert_eq!(result.removed, 0);
        assert!(f.directory.join("images/keep.png").exists());
    }

    #[test]
    fn clear_removes_all_history_and_caches_but_keeps_originals_and_settings() {
        let f = Fixture::new();
        f.add("image", DAY, 700_000);
        let original = f.directory.join("original.txt");
        let settings = f.directory.join("settings.json");
        std::fs::write(&original, b"original file").unwrap();
        std::fs::write(&settings, b"{}").unwrap();
        std::fs::write(f.directory.join("icons/unused.png"), b"icon").unwrap();
        clear(f.db()).unwrap();
        let current = usage(&f.db().0.lock().unwrap()).unwrap();
        assert_eq!(current.count, 0);
        assert!(current.bytes < MB);
        assert!(original.exists());
        assert!(settings.exists());
        assert!(cache_files(&f.directory).unwrap().is_empty());
    }
}

pub fn start(app: AppHandle) {
    std::thread::Builder::new()
        .name("history-retention".into())
        .spawn(move || loop {
            let result = {
                let db = app.state::<Db>();
                let _files = db.1.lock().unwrap();
                let policy = app.state::<SettingsStore>().get().retention;
                let result = enforce(&mut db.0.lock().unwrap(), &policy, now_ms());
                result
            };
            match result {
                Ok(result) if result.removed > 0 => {
                    let _ = app.emit(clipboard::CHANGED_EVENT, ());
                }
                Err(error) => {
                    eprintln!("[pinchy] history retention failed: {error}");
                    let _ = app.emit("storage-error", error);
                    let _ = app.emit(clipboard::CHANGED_EVENT, ());
                }
                _ => {}
            }
            std::thread::sleep(Duration::from_secs(60));
        })
        .expect("spawn history retention worker");
}
