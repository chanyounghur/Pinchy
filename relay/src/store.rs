//! Devices and the mailbox. Item contents are end-to-end encrypted by the
//! clients; the relay only stores opaque envelopes and blobs.

use rusqlite::{params, Connection, OptionalExtension};
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

#[derive(Debug, Clone, Serialize)]
pub struct InboxItem {
    pub id: String,
    /// Id of the sending device.
    pub from: String,
    pub created_at: i64,
    /// Base64 of the encrypted envelope.
    pub envelope: String,
    pub blob_size: Option<i64>,
}

#[derive(Debug, PartialEq)]
pub enum CreateError {
    Duplicate,
    NoRecipients,
    Db(String),
}

impl From<rusqlite::Error> for CreateError {
    fn from(e: rusqlite::Error) -> Self {
        CreateError::Db(e.to_string())
    }
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
             );
             CREATE TABLE IF NOT EXISTS items (
               id TEXT PRIMARY KEY,
               sender_id TEXT NOT NULL,
               created_at INTEGER NOT NULL,
               envelope BLOB NOT NULL,
               has_blob INTEGER NOT NULL,
               blob_size INTEGER,
               ready INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS deliveries (
               item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
               device_id TEXT NOT NULL,
               PRIMARY KEY (item_id, device_id)
             );
             CREATE INDEX IF NOT EXISTS idx_deliveries_device ON deliveries(device_id);",
        )
        .map_err(|e| e.to_string())?;
        Ok(Store { conn: Mutex::new(conn), blobs })
    }

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

    /// Removes a device and its undelivered mail. Items it sent stay for the others.
    pub fn remove_device(&self, id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM deliveries WHERE device_id = ?1", params![id]).map_err(|e| e.to_string())?;
        let removed = conn.execute("DELETE FROM devices WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
        self.delete_orphans(&conn).map_err(|e| e.to_string())?;
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

    /// Queues an item for every other device. Items with a blob stay hidden
    /// until the blob is uploaded. Returns the number of recipients.
    pub fn create_item(&self, sender: &Device, id: &str, envelope: &[u8], has_blob: bool) -> Result<usize, CreateError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO items (id, sender_id, created_at, envelope, has_blob, ready)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, sender.id, now_ms(), envelope, has_blob, !has_blob],
        )?;
        if inserted == 0 {
            return Err(CreateError::Duplicate);
        }
        let recipients = tx.execute(
            "INSERT INTO deliveries (item_id, device_id) SELECT ?1, id FROM devices WHERE id != ?2",
            params![id, sender.id],
        )?;
        if recipients == 0 {
            return Err(CreateError::NoRecipients);
        }
        tx.commit()?;
        Ok(recipients)
    }

    /// The upload target for an item's blob, if `sender` may still upload it.
    pub fn pending_blob(&self, sender: &Device, id: &str) -> Option<PathBuf> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM items WHERE id = ?1 AND sender_id = ?2 AND has_blob = 1 AND ready = 0",
            params![id, sender.id],
            |_| Ok(()),
        )
        .optional()
        .ok()??;
        Some(self.blob_path(id))
    }

    pub fn mark_ready(&self, id: &str, size: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE items SET ready = 1, blob_size = ?1 WHERE id = ?2", params![size, id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn inbox(&self, device: &Device, limit: i64) -> Result<Vec<InboxItem>, String> {
        use base64::Engine;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT i.id, i.sender_id, i.created_at, i.envelope, i.blob_size
                 FROM deliveries d JOIN items i ON i.id = d.item_id
                 WHERE d.device_id = ?1 AND i.ready = 1
                 ORDER BY i.created_at LIMIT ?2",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![device.id, limit], |r| {
                Ok(InboxItem {
                    id: r.get(0)?,
                    from: r.get(1)?,
                    created_at: r.get(2)?,
                    envelope: base64::engine::general_purpose::STANDARD.encode(r.get::<_, Vec<u8>>(3)?),
                    blob_size: r.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    /// The stored blob, if `device` is still waiting for this item.
    pub fn blob_for(&self, device: &Device, id: &str) -> Option<PathBuf> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM deliveries d JOIN items i ON i.id = d.item_id
             WHERE d.item_id = ?1 AND d.device_id = ?2 AND i.has_blob = 1 AND i.ready = 1",
            params![id, device.id],
            |_| Ok(()),
        )
        .optional()
        .ok()??;
        Some(self.blob_path(id))
    }

    /// Marks an item delivered to `device`; deletes it once everyone has it.
    pub fn ack(&self, device: &Device, id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        let removed = conn
            .execute("DELETE FROM deliveries WHERE item_id = ?1 AND device_id = ?2", params![id, device.id])
            .map_err(|e| e.to_string())?;
        self.delete_orphans(&conn).map_err(|e| e.to_string())?;
        Ok(removed > 0)
    }

    /// Drops items older than `retention_ms`, and blob items whose upload
    /// never finished within `stale_ms`. Returns how many were removed.
    pub fn purge(&self, now: i64, retention_ms: i64, stale_ms: i64) -> Result<usize, String> {
        let conn = self.conn.lock().unwrap();
        let ids: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT id FROM items WHERE created_at < ?1 OR (ready = 0 AND created_at < ?2)")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![now - retention_ms, now - stale_ms], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?
        };
        for id in &ids {
            self.delete_item(&conn, id).map_err(|e| e.to_string())?;
        }
        Ok(ids.len())
    }

    fn delete_orphans(&self, conn: &Connection) -> rusqlite::Result<()> {
        let ids: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT id FROM items WHERE NOT EXISTS (SELECT 1 FROM deliveries d WHERE d.item_id = items.id)",
            )?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        for id in &ids {
            self.delete_item(conn, id)?;
        }
        Ok(())
    }

    fn delete_item(&self, conn: &Connection, id: &str) -> rusqlite::Result<()> {
        conn.execute("DELETE FROM items WHERE id = ?1", params![id])?;
        let _ = std::fs::remove_file(self.blob_path(id));
        let _ = std::fs::remove_file(self.blob_path(id).with_extension("part"));
        Ok(())
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
    fn item_reaches_every_other_device_and_is_deleted_after_all_acks() {
        let (_dir, store) = store();
        let (a, _) = store.add_device(b"a").unwrap();
        let (b, _) = store.add_device(b"b").unwrap();
        let (c, _) = store.add_device(b"c").unwrap();
        assert_eq!(store.create_item(&a, &id(1), b"sealed", false).unwrap(), 2);
        assert!(store.inbox(&a, 10).unwrap().is_empty());
        let inbox = store.inbox(&b, 10).unwrap();
        assert_eq!((inbox.len(), inbox[0].from.as_str()), (1, a.id.as_str()));
        assert!(store.ack(&b, &id(1)).unwrap());
        assert!(store.inbox(&b, 10).unwrap().is_empty());
        assert_eq!(store.inbox(&c, 10).unwrap().len(), 1);
        assert!(store.ack(&c, &id(1)).unwrap());
        assert!(!store.ack(&c, &id(1)).unwrap());
        assert_eq!(store.create_item(&a, &id(1), b"again", false).unwrap(), 2, "acked item was removed");
    }

    #[test]
    fn blob_items_stay_hidden_until_uploaded_by_their_sender() {
        let (_dir, store) = store();
        let (a, _) = store.add_device(b"a").unwrap();
        let (b, _) = store.add_device(b"b").unwrap();
        store.create_item(&a, &id(2), b"sealed", true).unwrap();
        assert!(store.inbox(&b, 10).unwrap().is_empty());
        assert!(store.pending_blob(&b, &id(2)).is_none(), "only the sender uploads");
        assert!(store.blob_for(&b, &id(2)).is_none());
        let path = store.pending_blob(&a, &id(2)).unwrap();
        std::fs::write(&path, b"blob").unwrap();
        store.mark_ready(&id(2), 4).unwrap();
        assert!(store.pending_blob(&a, &id(2)).is_none(), "blobs cannot be replaced");
        assert_eq!(store.inbox(&b, 10).unwrap()[0].blob_size, Some(4));
        assert_eq!(store.blob_for(&b, &id(2)).unwrap(), path);
        assert!(store.blob_for(&a, &id(2)).is_none(), "the sender is not a recipient");
        store.ack(&b, &id(2)).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn duplicates_lonely_senders_and_expiry() {
        let (_dir, store) = store();
        let (a, _) = store.add_device(b"a").unwrap();
        assert_eq!(store.create_item(&a, &id(3), b"x", false), Err(CreateError::NoRecipients));
        let (b, _) = store.add_device(b"b").unwrap();
        store.create_item(&a, &id(3), b"x", false).unwrap();
        assert_eq!(store.create_item(&b, &id(3), b"y", false), Err(CreateError::Duplicate));
        store.create_item(&a, &id(4), b"x", true).unwrap();
        let now = now_ms();
        assert_eq!(store.purge(now, 7 * 86_400_000, 3_600_000).unwrap(), 0);
        assert_eq!(store.purge(now + 3_600_001, 7 * 86_400_000, 3_600_000).unwrap(), 1, "stale upload");
        assert_eq!(store.purge(now + 7 * 86_400_001, 7 * 86_400_000, 3_600_000).unwrap(), 1, "expired");
        assert!(store.inbox(&b, 10).unwrap().is_empty());
    }

    #[test]
    fn removing_a_device_drops_its_mail_and_access() {
        let (_dir, store) = store();
        let (a, _) = store.add_device(b"a").unwrap();
        let (b, token) = store.add_device(b"b").unwrap();
        store.create_item(&a, &id(5), b"x", false).unwrap();
        assert!(store.remove_device(&b.id).unwrap());
        assert!(!store.remove_device(&b.id).unwrap());
        assert!(store.authenticate(&token).is_none());
        assert_eq!(store.purge(now_ms() + 8 * 86_400_000, 7 * 86_400_000, 3_600_000).unwrap(), 0, "orphan was already deleted");
        assert_eq!(store.device_count().unwrap(), 1);
    }

    #[test]
    fn item_ids_must_be_random_hex() {
        assert!(valid_item_id(&id(9)));
        assert!(!valid_item_id("../../etc/passwd"));
        assert!(!valid_item_id(&"A".repeat(32)));
        assert!(!valid_item_id("abc"));
    }
}
