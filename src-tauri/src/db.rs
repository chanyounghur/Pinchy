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
}

const COLUMNS: &str =
    "id, kind, content, preview, source_app, app_icon, app_color, width, height, size, created_at, og_title, og_image";

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
    })
}

pub struct Db(pub Mutex<Connection>);

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
        for col in ["app_icon", "app_color", "og_title", "og_image", "og_checked"] {
            let exists: bool = conn
                .prepare("SELECT 1 FROM pragma_table_info('items') WHERE name = ?1")?
                .exists(params![col])?;
            if !exists {
                conn.execute(&format!("ALTER TABLE items ADD COLUMN {col} TEXT"), [])?;
            }
        }
        Ok(Db(Mutex::new(conn)))
    }

    /// Inserts a new item, or bumps an existing one with the same hash to the top.
    /// Returns true when a new row was created.
    pub fn upsert(&self, item: NewItem) -> rusqlite::Result<bool> {
        let conn = self.0.lock().unwrap();
        let now = now_ms();
        let existing: Option<i64> = conn
            .query_row("SELECT id FROM items WHERE hash = ?1", params![item.hash], |r| r.get(0))
            .optional()?;
        match existing {
            Some(id) => {
                conn.execute(
                    "UPDATE items SET created_at = ?1,
                       source_app = COALESCE(?2, source_app),
                       app_icon = COALESCE(?3, app_icon),
                       app_color = COALESCE(?4, app_color)
                     WHERE id = ?5",
                    params![now, item.source_app, item.app_icon, item.app_color, id],
                )?;
                Ok(false)
            }
            None => {
                conn.execute(
                    "INSERT INTO items (kind, content, preview, hash, source_app, app_icon, app_color, width, height, size, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
                        now
                    ],
                )?;
                Ok(true)
            }
        }
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

    pub fn delete(&self, id: i64) -> rusqlite::Result<Option<Item>> {
        let item = self.get(id)?;
        let conn = self.0.lock().unwrap();
        conn.execute("DELETE FROM items WHERE id = ?1", params![id])?;
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

    /// Deletes everything; returns the image items so their files can be removed.
    pub fn clear(&self) -> rusqlite::Result<Vec<Item>> {
        let conn = self.0.lock().unwrap();
        let images: Vec<Item> = conn
            .prepare(&format!("SELECT {COLUMNS} FROM items WHERE kind = 'image' OR og_image IS NOT NULL"))?
            .query_map([], row_to_item)?
            .collect::<rusqlite::Result<_>>()?;
        conn.execute("DELETE FROM items", [])?;
        Ok(images)
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
            width: None, height: None, size: 19 }
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
            let removed = db.clear().unwrap();
            assert_eq!(removed.len(), 1);
            assert_eq!(removed[0].og_image.as_deref(), image.to_str());
            assert!(db.list("", 10).unwrap().is_empty());
        }
        std::fs::remove_file(&image).unwrap();
        std::fs::remove_file(&path).unwrap();
        let _ = std::fs::remove_dir(&dir);
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
