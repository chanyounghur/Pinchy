use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub id: i64,
    pub kind: String, // text | link | image | files
    pub content: String,
    pub preview: String,
    pub source_app: Option<String>,
    pub app_icon: Option<String>,
    pub app_color: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub size: i64,
    pub created_at: i64,
    pub og_title: Option<String>,
    pub og_image: Option<String>,
    /// Name of the device this item was first copied on, when not this one.
    pub origin_device: Option<String>,
    /// Id shared with the other devices; every row has one.
    pub sync_id: String,
    /// The source app marked the copy as concealed (a password manager).
    pub concealed: bool,
}

pub struct NewItem {
    pub kind: String,
    pub content: String,
    pub preview: String,
    pub hash: String,
    pub source_app: Option<String>,
    pub app_icon: Option<String>,
    pub app_color: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub size: i64,
    pub origin_device: Option<String>,
    pub concealed: bool,
}

/// What a remote change did to the local history.
#[derive(Debug, Default, PartialEq)]
pub struct Applied {
    /// The visible history changed.
    pub changed: bool,
    /// A duplicate of this item on the relay that every device drops.
    pub delete_remote: Option<String>,
    /// A local row whose state is newer than the relay's and must be pushed.
    pub push: Option<i64>,
}

/// A change waiting to reach the relay.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub sync_id: String,
    pub op: Op,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Upsert,
    Delete,
}

impl Op {
    fn as_str(self) -> &'static str {
        match self {
            Op::Upsert => "upsert",
            Op::Delete => "delete",
        }
    }
}

const COLUMNS: &str =
    "id, kind, content, preview, source_app, app_icon, app_color, width, height, size, created_at, og_title, og_image, origin_device, sync_id, concealed";

fn row_to_item(r: &Row) -> rusqlite::Result<Item> {
    Ok(Item {
        id: r.get(0)?,
        kind: r.get(1)?,
        content: r.get(2)?,
        preview: r.get(3)?,
        source_app: r.get(4)?,
        app_icon: r.get(5)?,
        app_color: r.get(6)?,
        width: r.get(7)?,
        height: r.get(8)?,
        size: r.get(9)?,
        created_at: r.get(10)?,
        og_title: r.get(11)?,
        og_image: r.get(12)?,
        origin_device: r.get(13)?,
        sync_id: r.get(14)?,
        concealed: r.get::<_, Option<bool>>(15)?.unwrap_or(false),
    })
}

/// 128 random bits as lowercase hex: the id an item carries on every device.
pub fn new_sync_id() -> String {
    use chacha20poly1305::aead::rand_core::RngCore;
    let mut bytes = [0u8; 16];
    chacha20poly1305::aead::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// Lock file operations before the connection whenever a mutation also touches
// cached files. This keeps image capture, previews and retention consistent.
pub struct Db(pub Mutex<Connection>, pub Mutex<()>);

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS items (
               id INTEGER PRIMARY KEY,
               kind TEXT NOT NULL,
               content TEXT NOT NULL,
               preview TEXT NOT NULL,
               hash TEXT NOT NULL UNIQUE,
               source_app TEXT,
               width INTEGER,
               height INTEGER,
               size INTEGER NOT NULL DEFAULT 0,
               created_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_items_created ON items(created_at DESC);",
        )?;
        // Columns added after the first release.
        for (col, kind) in [
            ("app_icon", "TEXT"),
            ("app_color", "TEXT"),
            ("og_title", "TEXT"),
            ("og_image", "TEXT"),
            ("og_checked", "TEXT"),
            ("origin_device", "TEXT"),
            ("sync_id", "TEXT"),
            ("concealed", "INTEGER"),
        ] {
            let exists: bool = conn
                .prepare("SELECT 1 FROM pragma_table_info('items') WHERE name = ?1")?
                .exists(params![col])?;
            if !exists {
                conn.execute(&format!("ALTER TABLE items ADD COLUMN {col} {kind}"), [])?;
            }
        }
        conn.execute_batch(
            "UPDATE items SET sync_id = lower(hex(randomblob(16))) WHERE sync_id IS NULL;
             CREATE UNIQUE INDEX IF NOT EXISTS idx_items_sync ON items(sync_id);
             CREATE TABLE IF NOT EXISTS outbox (
               sync_id TEXT PRIMARY KEY,
               op TEXT NOT NULL,
               queued_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS sync (
               key TEXT PRIMARY KEY,
               value TEXT NOT NULL
             );",
        )?;
        Ok(Db(Mutex::new(conn), Mutex::new(())))
    }

    /// Inserts a new item, or bumps an existing one with the same hash to the
    /// top. Returns the row id and whether a new row was created. The change
    /// is queued for the other devices.
    pub fn upsert(&self, item: NewItem) -> rusqlite::Result<(i64, bool)> {
        let conn = self.0.lock().unwrap();
        let now = now_ms();
        let existing: Option<(i64, String)> = conn
            .query_row("SELECT id, sync_id FROM items WHERE hash = ?1", params![item.hash], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        match existing {
            Some((id, sync_id)) => {
                // A copy that is not concealed shows the content is no secret.
                conn.execute(
                    "UPDATE items SET created_at = ?1,
                       source_app = COALESCE(?2, source_app),
                       app_icon = COALESCE(?3, app_icon),
                       app_color = COALESCE(?4, app_color),
                       origin_device = COALESCE(?5, origin_device),
                       concealed = COALESCE(concealed, 0) AND ?6
                     WHERE id = ?7",
                    params![now, item.source_app, item.app_icon, item.app_color, item.origin_device, item.concealed, id],
                )?;
                Self::queue(&conn, &sync_id, Op::Upsert)?;
                Ok((id, false))
            }
            None => {
                let sync_id = new_sync_id();
                conn.execute(
                    "INSERT INTO items (kind, content, preview, hash, source_app, app_icon, app_color, width, height, size, created_at, origin_device, sync_id, concealed)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                    params![
                        item.kind,
                        item.content,
                        item.preview,
                        item.hash,
                        item.source_app,
                        item.app_icon,
                        item.app_color,
                        item.width,
                        item.height,
                        item.size,
                        now,
                        item.origin_device,
                        sync_id,
                        item.concealed
                    ],
                )?;
                let id = conn.last_insert_rowid();
                Self::queue(&conn, &sync_id, Op::Upsert)?;
                Ok((id, true))
            }
        }
    }

    /// Applies an item as another device last saw it. The relay's state wins
    /// for timestamps going forward; a duplicate of the same content created
    /// separately on two devices is merged onto the smaller id so every
    /// device settles on the same row.
    pub fn apply_remote(&self, sync_id: &str, item: NewItem, copied_at: i64) -> rusqlite::Result<Applied> {
        let _files = self.1.lock().unwrap();
        let conn = self.0.lock().unwrap();
        if let Some((id, created_at)) = conn
            .query_row("SELECT id, created_at FROM items WHERE sync_id = ?1", params![sync_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })
            .optional()?
        {
            conn.execute(
                "UPDATE items SET created_at = MAX(created_at, ?1),
                   source_app = COALESCE(?2, source_app),
                   origin_device = COALESCE(?3, origin_device)
                 WHERE id = ?4",
                params![copied_at, item.source_app, item.origin_device, id],
            )?;
            return Ok(Applied { changed: copied_at > created_at, ..Applied::default() });
        }
        let twin: Option<(i64, String, i64)> = conn
            .query_row("SELECT id, sync_id, created_at FROM items WHERE hash = ?1", params![item.hash], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?;
        if let Some((id, local_id, created_at)) = twin {
            let (keep, drop) = if sync_id < local_id.as_str() { (sync_id, local_id.clone()) } else { (local_id.as_str(), sync_id.to_string()) };
            conn.execute(
                "UPDATE items SET sync_id = ?1, created_at = MAX(created_at, ?2),
                   source_app = COALESCE(source_app, ?3), origin_device = COALESCE(origin_device, ?4)
                 WHERE id = ?5",
                params![keep, copied_at, item.source_app, item.origin_device, id],
            )?;
            conn.execute("DELETE FROM outbox WHERE sync_id = ?1", params![drop])?;
            // The survivor's envelope on the relay is stale when the other
            // copy was newer, or when it was never there under this id.
            let push = (created_at > copied_at || keep == sync_id).then_some(id);
            if push.is_some() {
                Self::queue(&conn, keep, Op::Upsert)?;
            }
            return Ok(Applied { changed: copied_at > created_at, delete_remote: Some(drop), push });
        }
        conn.execute(
            "INSERT INTO items (kind, content, preview, hash, source_app, app_icon, app_color, width, height, size, created_at, origin_device, sync_id, concealed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0)",
            params![
                item.kind,
                item.content,
                item.preview,
                item.hash,
                item.source_app,
                item.app_icon,
                item.app_color,
                item.width,
                item.height,
                item.size,
                copied_at,
                item.origin_device,
                sync_id
            ],
        )?;
        Ok(Applied { changed: true, ..Applied::default() })
    }

    /// Applies a recopy of an item this device already has, without its
    /// content. `None` when the item is not here yet.
    pub fn bump_remote(&self, sync_id: &str, copied_at: i64, source_app: Option<&str>, origin_device: Option<&str>) -> rusqlite::Result<Option<bool>> {
        let conn = self.0.lock().unwrap();
        let Some(created_at) = conn
            .query_row("SELECT created_at FROM items WHERE sync_id = ?1", params![sync_id], |r| r.get::<_, i64>(0))
            .optional()?
        else {
            return Ok(None);
        };
        conn.execute(
            "UPDATE items SET created_at = MAX(created_at, ?1),
               source_app = COALESCE(?2, source_app),
               origin_device = COALESCE(?3, origin_device)
             WHERE sync_id = ?4",
            params![copied_at, source_app, origin_device, sync_id],
        )?;
        Ok(Some(copied_at > created_at))
    }

    /// Removes an item another device deleted. Returns whether it was here.
    pub fn apply_remote_delete(&self, sync_id: &str) -> rusqlite::Result<bool> {
        let _files = self.1.lock().unwrap();
        let item = {
            let conn = self.0.lock().unwrap();
            conn.query_row(&format!("SELECT {COLUMNS} FROM items WHERE sync_id = ?1"), params![sync_id], row_to_item)
                .optional()?
        };
        let conn = self.0.lock().unwrap();
        conn.execute("DELETE FROM outbox WHERE sync_id = ?1", params![sync_id])?;
        let Some(item) = item else { return Ok(false) };
        conn.execute("DELETE FROM items WHERE id = ?1", params![item.id])?;
        crate::storage::remove_item_files(&conn, &item);
        Ok(true)
    }

    fn queue(conn: &Connection, sync_id: &str, op: Op) -> rusqlite::Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO outbox (sync_id, op, queued_at) VALUES (?1, ?2, ?3)",
            params![sync_id, op.as_str(), now_ms()],
        )?;
        Ok(())
    }

    /// Queues an item for the relay.
    #[cfg(test)]
    pub fn queue_upsert(&self, sync_id: &str) -> rusqlite::Result<()> {
        Self::queue(&self.0.lock().unwrap(), sync_id, Op::Upsert)
    }

    /// Queues a deletion for the relay.
    pub fn queue_delete(&self, sync_id: &str) -> rusqlite::Result<()> {
        Self::queue(&self.0.lock().unwrap(), sync_id, Op::Delete)
    }

    /// Queues every item the relay does not have yet, oldest first, as when
    /// a device connects with a history of its own.
    pub fn queue_missing(&self, on_relay: &std::collections::HashSet<String>) -> rusqlite::Result<usize> {
        let conn = self.0.lock().unwrap();
        let ids: Vec<(String, i64)> = {
            let mut stmt = conn.prepare("SELECT sync_id, created_at FROM items ORDER BY created_at")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut queued = 0;
        for (sync_id, created_at) in ids {
            if on_relay.contains(&sync_id) {
                continue;
            }
            conn.execute(
                "INSERT OR REPLACE INTO outbox (sync_id, op, queued_at) VALUES (?1, 'upsert', ?2)",
                params![sync_id, created_at],
            )?;
            queued += 1;
        }
        Ok(queued)
    }

    /// What waits for the relay, oldest first.
    pub fn outbox(&self, limit: i64) -> rusqlite::Result<Vec<Pending>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare("SELECT sync_id, op FROM outbox ORDER BY queued_at, sync_id LIMIT ?1")?;
        let rows = stmt.query_map(params![limit], |r| {
            let op: String = r.get(1)?;
            Ok(Pending { sync_id: r.get(0)?, op: if op == "delete" { Op::Delete } else { Op::Upsert } })
        })?;
        rows.collect()
    }

    /// Drops a queued change once the relay has it, unless it was re-queued
    /// with another operation meanwhile.
    pub fn dequeue(&self, pending: &Pending) -> rusqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        conn.execute("DELETE FROM outbox WHERE sync_id = ?1 AND op = ?2", params![pending.sync_id, pending.op.as_str()])?;
        Ok(())
    }

    pub fn clear_outbox(&self) -> rusqlite::Result<()> {
        self.0.lock().unwrap().execute("DELETE FROM outbox", [])?;
        Ok(())
    }

    pub fn get_by_sync_id(&self, sync_id: &str) -> rusqlite::Result<Option<Item>> {
        let conn = self.0.lock().unwrap();
        conn.query_row(&format!("SELECT {COLUMNS} FROM items WHERE sync_id = ?1"), params![sync_id], row_to_item)
            .optional()
    }

    pub fn sync_value(&self, key: &str) -> rusqlite::Result<Option<String>> {
        let conn = self.0.lock().unwrap();
        conn.query_row("SELECT value FROM sync WHERE key = ?1", params![key], |r| r.get(0)).optional()
    }

    pub fn set_sync_value(&self, key: &str, value: Option<&str>) -> rusqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        match value {
            Some(value) => {
                conn.execute("INSERT OR REPLACE INTO sync (key, value) VALUES (?1, ?2)", params![key, value])?;
            }
            None => {
                conn.execute("DELETE FROM sync WHERE key = ?1", params![key])?;
            }
        }
        Ok(())
    }

    /// Deletes local-only rows (concealed copies the relay never sees) older
    /// than `cutoff`, as the relay's retention would. Returns how many.
    pub fn expire_concealed(&self, cutoff: i64) -> rusqlite::Result<usize> {
        let _files = self.1.lock().unwrap();
        let items: Vec<Item> = {
            let conn = self.0.lock().unwrap();
            let mut stmt = conn.prepare(&format!(
                "SELECT {COLUMNS} FROM items WHERE concealed = 1 AND created_at < ?1"
            ))?;
            let rows = stmt.query_map(params![cutoff], row_to_item)?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let conn = self.0.lock().unwrap();
        for item in &items {
            conn.execute("DELETE FROM items WHERE id = ?1", params![item.id])?;
            conn.execute("DELETE FROM outbox WHERE sync_id = ?1", params![item.sync_id])?;
            crate::storage::remove_item_files(&conn, item);
        }
        Ok(items.len())
    }

    pub fn list(&self, query: &str, limit: i64) -> rusqlite::Result<Vec<Item>> {
        let conn = self.0.lock().unwrap();
        let pattern = format!("%{}%", query.trim());
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM items
             WHERE ?1 = '%%' OR preview LIKE ?1 OR content LIKE ?1 OR source_app LIKE ?1 OR og_title LIKE ?1
             ORDER BY created_at DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![pattern, limit], row_to_item)?;
        rows.collect()
    }

    pub fn get(&self, id: i64) -> rusqlite::Result<Option<Item>> {
        let conn = self.0.lock().unwrap();
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM items WHERE id = ?1"),
            params![id],
            row_to_item,
        )
        .optional()
    }

    /// Deletes an item here and queues the deletion for the other devices.
    pub fn delete(&self, id: i64) -> rusqlite::Result<Option<Item>> {
        let _files = self.1.lock().unwrap();
        let item = self.get(id)?;
        let conn = self.0.lock().unwrap();
        conn.execute("DELETE FROM items WHERE id = ?1", params![id])?;
        if let Some(item) = &item {
            Self::queue(&conn, &item.sync_id, Op::Delete)?;
            crate::storage::remove_item_files(&conn, item);
        }
        Ok(item)
    }

    /// Distinct source apps of rows that have no icon yet.
    pub fn apps_missing_icon(&self) -> rusqlite::Result<Vec<String>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT source_app FROM items WHERE app_icon IS NULL AND source_app IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect()
    }

    pub fn set_app_look(&self, source_app: &str, icon: &str, color: &str) -> rusqlite::Result<()> {
        let conn = self.0.lock().unwrap();
        conn.execute(
            "UPDATE items SET app_icon = ?1, app_color = ?2 WHERE source_app = ?3 AND app_icon IS NULL",
            params![icon, color, source_app],
        )?;
        Ok(())
    }

    /// Successful previews are permanent; failed requests can retry after an hour.
    pub fn pending_link(&self) -> rusqlite::Result<Option<Item>> {
        let conn = self.0.lock().unwrap();
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM items WHERE kind = 'link'
                AND og_title IS NULL AND og_image IS NULL
                AND (og_checked IS NULL OR CAST(og_checked AS INTEGER) < ?1)
                ORDER BY created_at DESC LIMIT 1"),
            params![now_ms() - 3_600_000], row_to_item,
        ).optional()
    }

    /// Keep the existence check and file write under the same lock as deletion.
    pub fn save_link_preview(&self, item: &Item, title: Option<&str>, image: Option<(&Path, &[u8])>) -> Result<(), String> {
        let _files = self.1.lock().unwrap();
        let conn = self.0.lock().unwrap();
        let exists = conn.prepare("SELECT 1 FROM items WHERE id = ?1 AND content = ?2")
            .and_then(|mut s| s.exists(params![item.id, item.content])).map_err(|e| e.to_string())?;
        if !exists { return Ok(()); }
        let image_path = image.and_then(|(path, bytes)| {
            std::fs::write(path, bytes).ok().map(|_| path.to_string_lossy().into_owned())
        });
        let result = conn.execute(
            "UPDATE items SET og_title = ?1, og_image = ?2, og_checked = ?3 WHERE id = ?4",
            params![title, image_path, now_ms(), item.id],
        );
        if result.is_err() {
            if let Some(path) = image_path { let _ = std::fs::remove_file(path); }
        }
        result.map(|_| ()).map_err(|e| e.to_string())
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> NewItem {
        NewItem { kind: "link".into(), content: "https://example.com".into(),
            preview: "https://example.com".into(), hash: "test-link".into(),
            source_app: None, app_icon: None, app_color: None,
            width: None, height: None, size: 19, origin_device: None, concealed: false }
    }

    fn text(content: &str) -> NewItem {
        let mut item = crate::clipboard::text_item(content.to_string());
        item.source_app = Some("Notes".into());
        item
    }

    fn ids(db: &Db) -> Vec<(String, Op)> {
        db.outbox(100).unwrap().into_iter().map(|p| (p.sync_id, p.op)).collect()
    }

    #[test]
    fn preview_survives_reopen_and_recopy_and_is_searchable() {
        let dir = std::env::temp_dir().join(format!("pinchy-og-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.db");
        let image = dir.join("cover.png");
        {
            let db = Db::open(&path).unwrap();
            db.upsert(link()).unwrap();
            let item = db.pending_link().unwrap().unwrap();
            db.save_link_preview(&item, Some("Saved title"), Some((&image, b"cached image"))).unwrap();
        }
        {
            let db = Db::open(&path).unwrap();
            db.upsert(link()).unwrap();
            assert!(db.pending_link().unwrap().is_none());
            let results = db.list("Saved title", 10).unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].content, "https://example.com");
            assert_eq!(std::fs::read(results[0].og_image.as_ref().unwrap()).unwrap(), b"cached image");
            crate::storage::clear(&db).unwrap();
            assert!(db.list("", 10).unwrap().is_empty());
        }
        std::fs::remove_file(&image).unwrap();
        std::fs::remove_file(&path).unwrap();
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn local_changes_queue_for_the_relay_and_rows_always_have_sync_ids() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let (id, created) = db.upsert(text("hello")).unwrap();
        assert!(created);
        let item = db.get(id).unwrap().unwrap();
        assert_eq!(item.sync_id.len(), 32);
        assert_eq!(ids(&db), vec![(item.sync_id.clone(), Op::Upsert)]);
        let pending = db.outbox(1).unwrap().pop().unwrap();
        db.dequeue(&pending).unwrap();
        assert!(ids(&db).is_empty());
        let (again, created) = db.upsert(text("hello")).unwrap();
        assert_eq!((again, created), (id, false), "a recopy bumps the same row");
        assert_eq!(ids(&db), vec![(item.sync_id.clone(), Op::Upsert)]);
        db.delete(id).unwrap();
        assert_eq!(ids(&db), vec![(item.sync_id.clone(), Op::Delete)], "the delete replaces the pending upsert");
        db.dequeue(&Pending { sync_id: item.sync_id.clone(), op: Op::Upsert }).unwrap();
        assert_eq!(ids(&db).len(), 1, "a stale dequeue leaves the newer operation");
        db.clear_outbox().unwrap();
        assert!(ids(&db).is_empty());
        db.set_sync_value("since", Some("42")).unwrap();
        assert_eq!(db.sync_value("since").unwrap().as_deref(), Some("42"));
        db.set_sync_value("since", None).unwrap();
        assert_eq!(db.sync_value("since").unwrap(), None);
    }

    #[test]
    fn remote_changes_apply_without_echoing_and_duplicates_merge_onto_the_smaller_id() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let mut remote = text("from the office");
        remote.origin_device = Some("회사 맥".into());
        let applied = db.apply_remote(&"b".repeat(32), remote, 1_000).unwrap();
        assert_eq!(applied, Applied { changed: true, delete_remote: None, push: None });
        let item = db.get_by_sync_id(&"b".repeat(32)).unwrap().unwrap();
        assert_eq!((item.created_at, item.origin_device.as_deref()), (1_000, Some("회사 맥")));
        assert!(ids(&db).is_empty(), "nothing goes back to the relay");

        // The same id again with a later copy time only bumps.
        let applied = db.apply_remote(&"b".repeat(32), text("from the office"), 2_000).unwrap();
        assert_eq!(applied, Applied { changed: true, delete_remote: None, push: None });
        assert_eq!(db.get_by_sync_id(&"b".repeat(32)).unwrap().unwrap().created_at, 2_000);
        let applied = db.apply_remote(&"b".repeat(32), text("from the office"), 1_500).unwrap();
        assert!(!applied.changed, "older times never move an item back");
        assert_eq!(db.bump_remote(&"b".repeat(32), 3_000, Some("Mail"), None).unwrap(), Some(true));
        assert_eq!(db.bump_remote(&"b".repeat(32), 2_500, None, None).unwrap(), Some(false));
        let bumped = db.get_by_sync_id(&"b".repeat(32)).unwrap().unwrap();
        assert_eq!((bumped.created_at, bumped.source_app.as_deref()), (3_000, Some("Mail")));
        assert_eq!(db.bump_remote(&"c".repeat(32), 1, None, None).unwrap(), None, "unknown items need their content");

        // A local copy of the same text made separately: the smaller id wins.
        let (local, _) = db.upsert(text("twice")).unwrap();
        db.clear_outbox().unwrap();
        let local_id = db.get(local).unwrap().unwrap().sync_id.clone();
        let smaller = "0".repeat(32);
        let applied = db.apply_remote(&smaller, text("twice"), 5).unwrap();
        assert_eq!(applied.delete_remote.as_deref(), Some(local_id.as_str()));
        assert_eq!(applied.push, Some(local), "our newer copy time must reach the relay under the kept id");
        let kept = db.get(local).unwrap().unwrap();
        assert_eq!(kept.sync_id, smaller);
        assert!(kept.created_at > 5);
        assert_eq!(ids(&db), vec![(smaller.clone(), Op::Upsert)]);
        assert_eq!(db.list("", 10).unwrap().len(), 2, "no duplicate row");

        // The other way round: a larger remote id is dropped from the relay.
        db.clear_outbox().unwrap();
        let larger = "f".repeat(32);
        let applied = db.apply_remote(&larger, text("twice"), now_ms() + 10_000).unwrap();
        assert_eq!(applied.delete_remote.as_deref(), Some(larger.as_str()));
        assert_eq!(applied.push, None, "the relay's copy of the kept id is already newer or equal");
        assert!(applied.changed);
        assert_eq!(db.get(local).unwrap().unwrap().sync_id, smaller);

        // Remote deletions remove the row and any queued change for it.
        db.queue_upsert(&smaller).unwrap();
        assert!(db.apply_remote_delete(&smaller).unwrap());
        assert!(!db.apply_remote_delete(&smaller).unwrap());
        assert!(db.get(local).unwrap().is_none());
        assert!(ids(&db).is_empty());
    }

    #[test]
    fn concealed_copies_expire_locally_and_a_plain_recopy_unconceals() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        let mut secret = text("hunter2");
        secret.concealed = true;
        let (id, _) = db.upsert(secret).unwrap();
        assert!(db.get(id).unwrap().unwrap().concealed);
        assert_eq!(db.expire_concealed(now_ms() - 1).unwrap(), 0, "fresh");
        db.upsert(text("hunter2")).unwrap();
        assert!(!db.get(id).unwrap().unwrap().concealed, "copied in the open once");
        let mut secret = text("p@ss");
        secret.concealed = true;
        let (other, _) = db.upsert(secret).unwrap();
        assert_eq!(db.expire_concealed(now_ms() + 1).unwrap(), 1);
        assert!(db.get(other).unwrap().is_none());
        assert!(db.get(id).unwrap().is_some());
        let kept = db.get(id).unwrap().unwrap().sync_id;
        assert_eq!(db.queue_missing(&std::collections::HashSet::new()).unwrap(), 1);
        assert_eq!(db.queue_missing(&std::collections::HashSet::from([kept])).unwrap(), 0);
    }

    #[test]
    fn failures_back_off_and_deleted_items_cannot_reappear() {
        let db = Db::open(Path::new(":memory:")).unwrap();
        db.upsert(link()).unwrap();
        let item = db.pending_link().unwrap().unwrap();
        db.save_link_preview(&item, None, None).unwrap();
        assert!(db.pending_link().unwrap().is_none());
        db.delete(item.id).unwrap();
        db.save_link_preview(&item, Some("Late response"), None).unwrap();
        assert!(db.list("", 10).unwrap().is_empty());
    }
}
