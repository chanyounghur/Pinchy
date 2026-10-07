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
    "id, kind, content, preview, source_app, app_icon, app_color, width, height, size, created_at";

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
        for col in ["app_icon", "app_color"] {
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
             WHERE ?1 = '%%' OR preview LIKE ?1 OR content LIKE ?1 OR source_app LIKE ?1
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

    /// Deletes everything; returns the image items so their files can be removed.
    pub fn clear(&self) -> rusqlite::Result<Vec<Item>> {
        let conn = self.0.lock().unwrap();
        let images: Vec<Item> = conn
            .prepare(&format!("SELECT {COLUMNS} FROM items WHERE kind = 'image'"))?
            .query_map([], row_to_item)?
            .collect::<rusqlite::Result<_>>()?;
        conn.execute("DELETE FROM items", [])?;
        Ok(images)
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
