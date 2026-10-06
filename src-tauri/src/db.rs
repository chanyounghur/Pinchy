use rusqlite::{params, Connection, OptionalExtension};
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
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub size: i64,
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
                    "UPDATE items SET created_at = ?1, source_app = COALESCE(?2, source_app) WHERE id = ?3",
                    params![now, item.source_app, id],
                )?;
                Ok(false)
            }
            None => {
                conn.execute(
                    "INSERT INTO items (kind, content, preview, hash, source_app, width, height, size, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        item.kind,
                        item.content,
                        item.preview,
                        item.hash,
                        item.source_app,
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
        let mut stmt = conn.prepare(
            "SELECT id, kind, content, preview, source_app, width, height, size, created_at
             FROM items
             WHERE ?1 = '%%' OR preview LIKE ?1 OR content LIKE ?1 OR source_app LIKE ?1
             ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, limit], |r| {
            Ok(Item {
                id: r.get(0)?,
                kind: r.get(1)?,
                content: r.get(2)?,
                preview: r.get(3)?,
                source_app: r.get(4)?,
                width: r.get(5)?,
                height: r.get(6)?,
                size: r.get(7)?,
                created_at: r.get(8)?,
            })
        })?;
        rows.collect()
    }

    pub fn get(&self, id: i64) -> rusqlite::Result<Option<Item>> {
        let conn = self.0.lock().unwrap();
        conn.query_row(
            "SELECT id, kind, content, preview, source_app, width, height, size, created_at FROM items WHERE id = ?1",
            params![id],
            |r| {
                Ok(Item {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    content: r.get(2)?,
                    preview: r.get(3)?,
                    source_app: r.get(4)?,
                    width: r.get(5)?,
                    height: r.get(6)?,
                    size: r.get(7)?,
                    created_at: r.get(8)?,
                })
            },
        )
        .optional()
    }

    pub fn delete(&self, id: i64) -> rusqlite::Result<Option<Item>> {
        let item = self.get(id)?;
        let conn = self.0.lock().unwrap();
        conn.execute("DELETE FROM items WHERE id = ?1", params![id])?;
        Ok(item)
    }

    pub fn clear(&self) -> rusqlite::Result<Vec<Item>> {
        let images = self.list_images()?;
        let conn = self.0.lock().unwrap();
        conn.execute("DELETE FROM items", [])?;
        Ok(images)
    }

    fn list_images(&self) -> rusqlite::Result<Vec<Item>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, kind, content, preview, source_app, width, height, size, created_at FROM items WHERE kind = 'image'",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Item {
                id: r.get(0)?,
                kind: r.get(1)?,
                content: r.get(2)?,
                preview: r.get(3)?,
                source_app: r.get(4)?,
                width: r.get(5)?,
                height: r.get(6)?,
                size: r.get(7)?,
                created_at: r.get(8)?,
            })
        })?;
        rows.collect()
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
