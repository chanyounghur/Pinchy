//! Devices and the shared history. Item contents are end-to-end encrypted by
//! the clients; the relay only stores opaque envelopes and blobs.
//!
//! Every item carries the sequence number of its latest change, so a client
//! that remembers the last sequence it saw can ask for everything that
//! changed since. A deleted item stays as a tombstone (its envelope and blob
//! gone) long enough for every device to learn of the deletion; tombstones
//! older than that are compacted away and the highest compacted sequence is
//! kept as the `floor`: a client behind the floor must resynchronise from
//! scratch.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct Store {
    conn: Mutex<Connection>,
    blobs: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub id: String,
    /// The device's name, sealed by the clients; opaque to the relay.
    #[serde(serialize_with = "as_base64")]
    pub label: Vec<u8>,
    pub created_at: i64,
    pub last_seen: Option<i64>,
}

fn as_base64<S: serde::Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
    use base64::Engine;
    s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// One entry of the change feed: the current state of an item.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Change {
    pub id: String,
    pub seq: i64,
    /// Device that made this change.
    pub origin: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted: bool,
    /// Base64 of the encrypted envelope; absent for tombstones.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub envelope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob_size: Option<i64>,
}

/// A page of the change feed.
#[derive(Debug, Serialize)]
pub struct Changes {
    pub items: Vec<Change>,
    /// Pass this as `since` next time.
    pub next: i64,
    /// Whether more changes follow `next` right now.
    pub more: bool,
}

#[derive(Debug, PartialEq)]
pub enum PutOutcome {
    /// The item is new and complete.
    Created,
    /// The item is new and waits for its blob before it becomes visible.
    AwaitingBlob,
    /// An existing item's envelope was replaced.
    Updated,
}

#[derive(Debug, PartialEq)]
pub enum PutError {
    /// The item was deleted; deletions win over late updates.
    Deleted,
    /// An existing item's blob status cannot change.
    BlobMismatch,
    Db(String),
}

impl From<rusqlite::Error> for PutError {
    fn from(e: rusqlite::Error) -> Self {
        PutError::Db(e.to_string())
    }
}

/// How much the relay stores for the history, blobs included.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Usage {
    pub items: i64,
    pub bytes: i64,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("system randomness");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

/// Item ids are chosen by the sender so they can be bound into the
/// ciphertext before upload; they must look like 128 random bits.
pub fn valid_item_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn row_to_change(r: &Row) -> rusqlite::Result<Change> {
    use base64::Engine;
    let deleted: bool = r.get(5)?;
    let envelope: Option<Vec<u8>> = r.get(6)?;
    Ok(Change {
        id: r.get(0)?,
        seq: r.get(1)?,
        origin: r.get(2)?,
        created_at: r.get(3)?,
        updated_at: r.get(4)?,
        deleted,
        envelope: envelope.filter(|_| !deleted).map(|e| base64::engine::general_purpose::STANDARD.encode(e)),
        blob_size: if deleted { None } else { r.get(7)? },
    })
}

const CHANGE_COLUMNS: &str = "id, seq, origin, created_at, updated_at, deleted_at IS NOT NULL, envelope, blob_size";

impl Store {
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        let blobs = data_dir.join("blobs");
        std::fs::create_dir_all(&blobs).map_err(|e| format!("{}: {e}", blobs.display()))?;
        let conn = Connection::open(data_dir.join("relay.db")).map_err(|e| e.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS devices (
               id TEXT PRIMARY KEY,
               label BLOB NOT NULL,
               token_hash TEXT NOT NULL UNIQUE,
               created_at INTEGER NOT NULL,
               last_seen INTEGER
             );",
        )
        .map_err(|e| e.to_string())?;
        // The mailbox schema of relay 0.x kept items only until every device
        // had them, so nothing in it is worth migrating.
        let mailbox: bool = conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'deliveries'")
            .and_then(|mut s| s.exists([]))
            .map_err(|e| e.to_string())?;
        if mailbox {
            conn.execute_batch("DROP TABLE deliveries; DROP TABLE items;").map_err(|e| e.to_string())?;
            for entry in std::fs::read_dir(&blobs).map_err(|e| e.to_string())?.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
            eprintln!("[pinchy-relay] cleared the 0.x mailbox; devices stay connected");
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS items (
               id TEXT PRIMARY KEY,
               seq INTEGER NOT NULL UNIQUE,
               origin TEXT NOT NULL,
               creator TEXT NOT NULL,
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL,
               deleted_at INTEGER,
               envelope BLOB,
               has_blob INTEGER NOT NULL,
               blob_size INTEGER,
               ready INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_items_updated ON items(updated_at);
             CREATE TABLE IF NOT EXISTS meta (
               key TEXT PRIMARY KEY,
               value INTEGER NOT NULL
             );
             INSERT OR IGNORE INTO meta (key, value) VALUES ('seq', 0), ('floor', 0);",
        )
        .map_err(|e| e.to_string())?;
        Ok(Store { conn: Mutex::new(conn), blobs })
    }

    // -----------------------------------------------------------------------
    // Devices

    /// Registers a device and returns it with its token. Only the token's
    /// hash is kept.
    pub fn add_device(&self, label: &[u8]) -> Result<(Device, String), String> {
        let token = format!("pr1_{}", random_hex(32));
        let device = Device { id: random_hex(8), label: label.to_vec(), created_at: now_ms(), last_seen: None };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO devices (id, label, token_hash, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![device.id, device.label, token_hash(&token), device.created_at],
        )
        .map_err(|e| e.to_string())?;
        Ok((device, token))
    }

    pub fn device_count(&self) -> Result<i64, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM devices", [], |r| r.get(0)).map_err(|e| e.to_string())
    }

    pub fn list_devices(&self) -> Result<Vec<Device>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, label, created_at, last_seen FROM devices ORDER BY created_at")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Device { id: r.get(0)?, label: r.get(1)?, created_at: r.get(2)?, last_seen: r.get(3)? })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    pub fn set_label(&self, device: &Device, label: &[u8]) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE devices SET label = ?1 WHERE id = ?2", params![label, device.id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Removes a device. The history stays for the others; blob uploads the
    /// device never finished are dropped.
    pub fn remove_device(&self, id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute("DELETE FROM devices WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
        let unfinished = self
            .ids(&conn, "SELECT id FROM items WHERE ready = 0 AND deleted_at IS NULL AND creator = ?1", params![id])
            .map_err(|e| e.to_string())?;
        for item in unfinished {
            self.tombstone(&conn, &item, id, now_ms()).map_err(|e| e.to_string())?;
        }
        Ok(removed > 0)
    }

    pub fn authenticate(&self, token: &str) -> Option<Device> {
        let conn = self.conn.lock().unwrap();
        let device = conn
            .query_row(
                "SELECT id, label, created_at, last_seen FROM devices WHERE token_hash = ?1",
                params![token_hash(token)],
                |r| Ok(Device { id: r.get(0)?, label: r.get(1)?, created_at: r.get(2)?, last_seen: r.get(3)? }),
            )
            .optional()
            .ok()??;
        let _ = conn.execute("UPDATE devices SET last_seen = ?1 WHERE id = ?2", params![now_ms(), device.id]);
        Some(device)
    }

    // -----------------------------------------------------------------------
    // Items

    fn next_seq(conn: &Connection) -> rusqlite::Result<i64> {
        conn.query_row("UPDATE meta SET value = value + 1 WHERE key = 'seq' RETURNING value", [], |r| r.get(0))
    }

    fn meta(conn: &Connection, key: &str) -> rusqlite::Result<i64> {
        conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0))
    }

    fn ids(&self, conn: &Connection, sql: &str, params: impl rusqlite::Params) -> rusqlite::Result<Vec<String>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| r.get(0))?;
        rows.collect()
    }

    /// Creates an item, or replaces the envelope of an existing one. An item
    /// with a blob stays invisible until its creator uploads the blob.
    pub fn put_item(&self, device: &Device, id: &str, envelope: &[u8], has_blob: bool) -> Result<PutOutcome, PutError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let existing: Option<(bool, bool)> = tx
            .query_row("SELECT deleted_at IS NOT NULL, has_blob FROM items WHERE id = ?1", params![id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let now = now_ms();
        let seq = Self::next_seq(&tx)?;
        let outcome = match existing {
            Some((true, _)) => return Err(PutError::Deleted),
            Some((false, had_blob)) if had_blob != has_blob => return Err(PutError::BlobMismatch),
            Some((false, _)) => {
                tx.execute(
                    "UPDATE items SET seq = ?1, origin = ?2, updated_at = ?3, envelope = ?4 WHERE id = ?5",
                    params![seq, device.id, now, envelope, id],
                )?;
                PutOutcome::Updated
            }
            None => {
                tx.execute(
                    "INSERT INTO items (id, seq, origin, creator, created_at, updated_at, envelope, has_blob, ready)
                     VALUES (?1, ?2, ?3, ?3, ?4, ?4, ?5, ?6, ?7)",
                    params![id, seq, device.id, now, envelope, has_blob, !has_blob],
                )?;
                if has_blob { PutOutcome::AwaitingBlob } else { PutOutcome::Created }
            }
        };
        tx.commit()?;
        Ok(outcome)
    }

    /// The upload target for an item's blob, if `device` may still upload it.
    pub fn pending_blob(&self, device: &Device, id: &str) -> Option<PathBuf> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM items WHERE id = ?1 AND creator = ?2 AND has_blob = 1 AND ready = 0 AND deleted_at IS NULL",
            params![id, device.id],
            |_| Ok(()),
        )
        .optional()
        .ok()??;
        Some(self.blob_path(id))
    }

    /// Makes a blob item visible once its blob is stored.
    pub fn mark_ready(&self, id: &str, size: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let seq = Self::next_seq(&conn).map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE items SET ready = 1, blob_size = ?1, seq = ?2, updated_at = ?3 WHERE id = ?4",
            params![size, seq, now_ms(), id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The stored blob of a visible item.
    pub fn blob_for(&self, id: &str) -> Option<PathBuf> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM items WHERE id = ?1 AND has_blob = 1 AND ready = 1 AND deleted_at IS NULL",
            params![id],
            |_| Ok(()),
        )
        .optional()
        .ok()??;
        Some(self.blob_path(id))
    }

    /// Deletes an item for every device. Returns false when there was no such
    /// live item.
    pub fn delete_item(&self, device: &Device, id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        let live: bool = conn
            .prepare("SELECT 1 FROM items WHERE id = ?1 AND deleted_at IS NULL")
            .and_then(|mut s| s.exists(params![id]))
            .map_err(|e| e.to_string())?;
        if !live {
            return Ok(false);
        }
        self.tombstone(&conn, id, &device.id, now_ms()).map_err(|e| e.to_string())?;
        Ok(true)
    }

    /// Deletes the whole history. Returns how many items were live.
    pub fn delete_all(&self, device: &Device) -> Result<usize, String> {
        let conn = self.conn.lock().unwrap();
        let ids = self.ids(&conn, "SELECT id FROM items WHERE deleted_at IS NULL", []).map_err(|e| e.to_string())?;
        let now = now_ms();
        for id in &ids {
            self.tombstone(&conn, id, &device.id, now).map_err(|e| e.to_string())?;
        }
        Ok(ids.len())
    }

    fn tombstone(&self, conn: &Connection, id: &str, origin: &str, now: i64) -> rusqlite::Result<()> {
        let seq = Self::next_seq(conn)?;
        conn.execute(
            "UPDATE items SET seq = ?1, origin = ?2, updated_at = ?3, deleted_at = ?3, envelope = NULL, blob_size = NULL
             WHERE id = ?4",
            params![seq, origin, now, id],
        )?;
        self.remove_blob(id);
        Ok(())
    }

    /// Everything that changed after `since`, oldest change first, at most
    /// `limit` entries. Invisible blob items are left out. `None` when
    /// `since` is behind the compaction floor and the client must start over.
    pub fn changes(&self, since: i64, limit: i64) -> Result<Option<Changes>, String> {
        let conn = self.conn.lock().unwrap();
        let floor = Self::meta(&conn, "floor").map_err(|e| e.to_string())?;
        if since > 0 && since < floor {
            return Ok(None);
        }
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {CHANGE_COLUMNS} FROM items WHERE seq > ?1 AND ready = 1 ORDER BY seq LIMIT ?2"
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![since, limit + 1], row_to_change).map_err(|e| e.to_string())?;
        let mut items: Vec<Change> = rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
        let more = items.len() as i64 > limit;
        if more {
            items.truncate(limit as usize);
        }
        let next = match items.last() {
            Some(last) => last.seq,
            // Nothing visible after `since`: the client is up to date with
            // the whole feed, so it can skip ahead.
            None => Self::meta(&conn, "seq").map_err(|e| e.to_string())?.max(since),
        };
        Ok(Some(Changes { items, next, more }))
    }

    pub fn usage(&self) -> Result<Usage, String> {
        let conn = self.conn.lock().unwrap();
        Self::usage_in(&conn).map_err(|e| e.to_string())
    }

    fn usage_in(conn: &Connection) -> rusqlite::Result<Usage> {
        conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(COALESCE(length(envelope), 0) + COALESCE(blob_size, 0)), 0)
             FROM items WHERE deleted_at IS NULL",
            [],
            |r| Ok(Usage { items: r.get(0)?, bytes: r.get(1)? }),
        )
    }

    // -----------------------------------------------------------------------
    // Maintenance

    /// Applies the retention policy, oldest change first: items not touched
    /// for `retention_ms`, then the oldest items while the history exceeds
    /// `max_bytes`, and blob items whose upload never finished within
    /// `stale_ms`. Deleted items become tombstones. Returns how many.
    pub fn purge(&self, now: i64, retention_ms: i64, max_bytes: i64, stale_ms: i64) -> Result<usize, String> {
        let conn = self.conn.lock().unwrap();
        let mut ids = self
            .ids(
                &conn,
                "SELECT id FROM items WHERE deleted_at IS NULL
                 AND (updated_at < ?1 OR (ready = 0 AND created_at < ?2))",
                params![now - retention_ms, now - stale_ms],
            )
            .map_err(|e| e.to_string())?;
        for id in &ids {
            self.tombstone(&conn, id, "relay", now).map_err(|e| e.to_string())?;
        }
        let mut usage = Self::usage_in(&conn).map_err(|e| e.to_string())?;
        if usage.bytes > max_bytes {
            let oldest = self
                .ids(
                    &conn,
                    "SELECT id FROM items WHERE deleted_at IS NULL AND ready = 1 ORDER BY updated_at, seq",
                    [],
                )
                .map_err(|e| e.to_string())?;
            for id in oldest {
                if usage.bytes <= max_bytes {
                    break;
                }
                self.tombstone(&conn, &id, "relay", now).map_err(|e| e.to_string())?;
                usage = Self::usage_in(&conn).map_err(|e| e.to_string())?;
                ids.push(id);
            }
        }
        Ok(ids.len())
    }

    /// Forgets tombstones older than `keep_ms` and raises the floor past
    /// them. Returns how many were removed.
    pub fn compact(&self, now: i64, keep_ms: i64) -> Result<usize, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let highest: Option<i64> = tx
            .query_row("SELECT MAX(seq) FROM items WHERE deleted_at < ?1", params![now - keep_ms], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let Some(highest) = highest else { return Ok(0) };
        let removed = tx
            .execute("DELETE FROM items WHERE deleted_at < ?1", params![now - keep_ms])
            .map_err(|e| e.to_string())?;
        tx.execute("UPDATE meta SET value = MAX(value, ?1) WHERE key = 'floor'", params![highest])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(removed)
    }

    fn remove_blob(&self, id: &str) {
        let _ = std::fs::remove_file(self.blob_path(id));
        let _ = std::fs::remove_file(self.blob_path(id).with_extension("part"));
    }

    fn blob_path(&self, id: &str) -> PathBuf {
        self.blobs.join(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        (dir, store)
    }

    fn id(n: u8) -> String {
        format!("{n:032x}")
    }

    fn feed(store: &Store, since: i64) -> Vec<(String, bool)> {
        store.changes(since, 100).unwrap().unwrap().items.into_iter().map(|c| (c.id, c.deleted)).collect()
    }

    #[test]
    fn tokens_authenticate_and_labels_stay_opaque() {
        let (_dir, store) = store();
        assert_eq!(store.device_count().unwrap(), 0);
        let (device, token) = store.add_device(b"sealed name").unwrap();
        assert_eq!(store.authenticate(&token).unwrap().id, device.id);
        assert!(store.authenticate("pr1_wrong").is_none());
        store.set_label(&device, b"renamed").unwrap();
        assert_eq!(store.list_devices().unwrap()[0].label, b"renamed");
        assert!(store.list_devices().unwrap()[0].last_seen.is_some());
        assert_eq!(store.device_count().unwrap(), 1);
    }

    #[test]
    fn changes_replay_the_latest_state_of_every_item_in_order() {
        let (_dir, store) = store();
        let (a, _) = store.add_device(b"a").unwrap();
        let (b, _) = store.add_device(b"b").unwrap();
        assert_eq!(store.put_item(&a, &id(1), b"one", false).unwrap(), PutOutcome::Created);
        assert_eq!(store.put_item(&a, &id(2), b"two", false).unwrap(), PutOutcome::Created);
        let page = store.changes(0, 100).unwrap().unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!((page.items[0].seq, page.items[1].seq, page.next, page.more), (1, 2, 2, false));
        assert_eq!(page.items[0].origin, a.id);
        assert_eq!(page.items[0].envelope.as_deref(), Some("b25l"), "base64 of 'one'");

        // An update moves the item to the end of the feed under the new origin.
        assert_eq!(store.put_item(&b, &id(1), b"uno", false).unwrap(), PutOutcome::Updated);
        let page = store.changes(2, 100).unwrap().unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!((page.items[0].id.as_str(), page.items[0].seq, page.items[0].origin.as_str()), (id(1).as_str(), 3, b.id.as_str()));
        assert_eq!(page.items[0].envelope.as_deref(), Some("dW5v"));
        assert_eq!(feed(&store, 0), vec![(id(2), false), (id(1), false)]);

        // Pages and the empty tail.
        let page = store.changes(0, 1).unwrap().unwrap();
        assert_eq!((page.items.len(), page.next, page.more), (1, 2, true));
        let tail = store.changes(3, 100).unwrap().unwrap();
        assert!(tail.items.is_empty() && tail.next == 3 && !tail.more);
    }

    #[test]
    fn deletions_become_tombstones_that_win_over_late_updates() {
        let (_dir, store) = store();
        let (a, _) = store.add_device(b"a").unwrap();
        store.put_item(&a, &id(1), b"x", false).unwrap();
        assert!(store.delete_item(&a, &id(1)).unwrap());
        assert!(!store.delete_item(&a, &id(1)).unwrap(), "already gone");
        assert!(!store.delete_item(&a, &id(9)).unwrap(), "never existed");
        let page = store.changes(1, 100).unwrap().unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(page.items[0].deleted && page.items[0].envelope.is_none());
        assert_eq!(store.put_item(&a, &id(1), b"late", false), Err(PutError::Deleted));
        assert_eq!(store.usage().unwrap(), Usage { items: 0, bytes: 0 });
    }

    #[test]
    fn blob_items_stay_hidden_until_uploaded_by_their_creator() {
        let (_dir, store) = store();
        let (a, _) = store.add_device(b"a").unwrap();
        let (b, _) = store.add_device(b"b").unwrap();
        assert_eq!(store.put_item(&a, &id(2), b"meta", true).unwrap(), PutOutcome::AwaitingBlob);
        assert!(feed(&store, 0).is_empty());
        assert!(store.pending_blob(&b, &id(2)).is_none(), "only the creator uploads");
        assert!(store.blob_for(&id(2)).is_none());
        let path = store.pending_blob(&a, &id(2)).unwrap();
        std::fs::write(&path, b"blob").unwrap();
        store.mark_ready(&id(2), 4).unwrap();
        assert!(store.pending_blob(&a, &id(2)).is_none(), "blobs cannot be replaced");
        let page = store.changes(0, 100).unwrap().unwrap();
        assert_eq!(page.items[0].blob_size, Some(4));
        assert_eq!(page.items[0].seq, 2, "becoming visible is a change");
        assert_eq!(store.blob_for(&id(2)).unwrap(), path);
        assert_eq!(store.put_item(&b, &id(2), b"meta2", true).unwrap(), PutOutcome::Updated, "envelope updates keep the blob");
        assert_eq!(store.put_item(&b, &id(2), b"meta3", false), Err(PutError::BlobMismatch));
        assert_eq!(store.usage().unwrap(), Usage { items: 1, bytes: 9 });
        store.delete_item(&b, &id(2)).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn retention_tombstones_old_untouched_and_oversized_items() {
        let (_dir, store) = store();
        let (a, _) = store.add_device(b"a").unwrap();
        let day = 86_400_000;
        let now = now_ms();
        store.put_item(&a, &id(1), b"old", false).unwrap();
        store.put_item(&a, &id(2), b"unfinished", true).unwrap();
        assert_eq!(store.purge(now, 30 * day, 1 << 30, 3_600_000).unwrap(), 0);
        assert_eq!(store.purge(now + 3_600_001, 30 * day, 1 << 30, 3_600_000).unwrap(), 1, "stale upload");
        store.put_item(&a, &id(3), b"touched", false).unwrap();
        // Bumping an item on another device keeps it alive.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let later = now_ms();
        store.put_item(&a, &id(3), b"touched again", false).unwrap();
        let removed = store.purge(later + 30 * day - 1, 30 * day, 1 << 30, 3_600_000).unwrap();
        assert_eq!(removed, 1, "only the untouched item expired");
        assert_eq!(feed(&store, 0).iter().filter(|(_, deleted)| !deleted).count(), 1);
        store.put_item(&a, &id(4), &[b'a'; 10], false).unwrap();
        store.put_item(&a, &id(5), &[b'b'; 10], false).unwrap();
        assert_eq!(store.purge(now_ms(), 30 * day, 25, 3_600_000).unwrap(), 1, "oldest first until under the cap");
        assert!(store.usage().unwrap().bytes <= 25);
        assert_eq!(store.usage().unwrap().items, 2);
        let live: Vec<String> = feed(&store, 0).into_iter().filter(|(_, d)| !d).map(|(id, _)| id).collect();
        assert_eq!(live, vec![id(4), id(5)]);
    }

    #[test]
    fn compaction_drops_old_tombstones_and_raises_the_floor() {
        let (_dir, store) = store();
        let (a, _) = store.add_device(b"a").unwrap();
        store.put_item(&a, &id(1), b"x", false).unwrap();
        store.put_item(&a, &id(2), b"y", false).unwrap();
        store.delete_item(&a, &id(1)).unwrap(); // seq 3
        let now = now_ms();
        assert_eq!(store.compact(now, 7 * 86_400_000).unwrap(), 0, "too fresh");
        assert_eq!(store.compact(now + 7 * 86_400_001, 7 * 86_400_000).unwrap(), 1);
        assert_eq!(feed(&store, 0), vec![(id(2), false)], "a full sync no longer sees it");
        assert!(store.changes(2, 100).unwrap().is_none(), "behind the floor: start over");
        assert!(store.changes(3, 100).unwrap().is_some(), "at the floor is fine");
        assert_eq!(store.changes(3, 100).unwrap().unwrap().next, 3);
    }

    #[test]
    fn delete_all_and_removing_a_device() {
        let (_dir, store) = store();
        let (a, _) = store.add_device(b"a").unwrap();
        let (b, token) = store.add_device(b"b").unwrap();
        store.put_item(&a, &id(1), b"x", false).unwrap();
        store.put_item(&b, &id(2), b"y", true).unwrap();
        assert!(store.remove_device(&b.id).unwrap());
        assert!(!store.remove_device(&b.id).unwrap());
        assert!(store.authenticate(&token).is_none());
        assert_eq!(store.usage().unwrap().items, 1, "its unfinished upload is gone, the history stays");
        store.put_item(&a, &id(3), b"z", false).unwrap();
        assert_eq!(store.delete_all(&a).unwrap(), 2);
        assert_eq!(store.delete_all(&a).unwrap(), 0);
        assert!(feed(&store, 0).iter().all(|(_, deleted)| *deleted));
        assert_eq!(store.device_count().unwrap(), 1);
    }

    #[test]
    fn the_old_mailbox_schema_is_cleared_but_devices_survive() {
        let dir = tempfile::tempdir().unwrap();
        {
            let conn = Connection::open(dir.path().join("relay.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE devices (id TEXT PRIMARY KEY, label BLOB NOT NULL, token_hash TEXT NOT NULL UNIQUE, created_at INTEGER NOT NULL, last_seen INTEGER);
                 INSERT INTO devices VALUES ('dev', x'00', 'hash', 1, NULL);
                 CREATE TABLE items (id TEXT PRIMARY KEY, sender_id TEXT NOT NULL, created_at INTEGER NOT NULL, envelope BLOB NOT NULL, has_blob INTEGER NOT NULL, blob_size INTEGER, ready INTEGER NOT NULL);
                 CREATE TABLE deliveries (item_id TEXT NOT NULL, device_id TEXT NOT NULL, PRIMARY KEY (item_id, device_id));
                 INSERT INTO items VALUES ('old', 'dev', 1, x'00', 0, NULL, 1);",
            )
            .unwrap();
            std::fs::create_dir_all(dir.path().join("blobs")).unwrap();
            std::fs::write(dir.path().join("blobs/old"), b"blob").unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.device_count().unwrap(), 1);
        assert!(feed(&store, 0).is_empty());
        assert!(!dir.path().join("blobs/old").exists());
        assert_eq!(store.changes(0, 100).unwrap().unwrap().next, 0);
    }

    #[test]
    fn item_ids_must_be_random_hex() {
        assert!(valid_item_id(&id(9)));
        assert!(!valid_item_id("../../etc/passwd"));
        assert!(!valid_item_id(&"A".repeat(32)));
        assert!(!valid_item_id("abc"));
    }
}
