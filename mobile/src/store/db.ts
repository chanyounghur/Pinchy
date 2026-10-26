// The phone's copy of the shared history: opened envelopes only. The relay
// is the source of truth; this is a cache that `sync.ts` keeps current and
// can rebuild from scratch after a reset.

import * as SQLite from "expo-sqlite";
import type { FileMeta } from "../protocol/meta";

export type BlobState = "none" | "missing" | "ready" | "failed";

export interface Item {
  id: string;
  seq: number;
  origin: string;
  kind: string;
  text: string | null;
  files: FileMeta[] | null;
  from: string;
  app: string | null;
  copied_at: number;
  updated_at: number;
  blob_size: number | null;
  blob_state: BlobState;
  width: number | null;
  height: number | null;
}

interface Row {
  id: string;
  seq: number;
  origin: string;
  kind: string;
  text: string | null;
  files: string | null;
  from_device: string;
  app: string | null;
  copied_at: number;
  updated_at: number;
  blob_size: number | null;
  blob_state: BlobState;
  width: number | null;
  height: number | null;
}

const SCHEMA = `
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS items (
  id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL,
  origin TEXT NOT NULL,
  kind TEXT NOT NULL,
  text TEXT,
  files TEXT,
  from_device TEXT NOT NULL,
  app TEXT,
  copied_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  blob_size INTEGER,
  blob_state TEXT NOT NULL DEFAULT 'none',
  width INTEGER,
  height INTEGER
);
CREATE INDEX IF NOT EXISTS items_copied_at ON items(copied_at DESC);
CREATE TABLE IF NOT EXISTS sync (key TEXT PRIMARY KEY, value TEXT NOT NULL);
`;

export class Db {
  private constructor(private readonly db: SQLite.SQLiteDatabase) {}

  static async open(name = "pinchy.db"): Promise<Db> {
    const db = await SQLite.openDatabaseAsync(name);
    await db.execAsync(SCHEMA);
    return new Db(db);
  }

  async upsert(item: Omit<Item, "blob_state" | "width" | "height"> & Partial<Pick<Item, "blob_state" | "width" | "height">>): Promise<void> {
    // A blob already on disk survives a re-copy: blobs never change.
    await this.db.runAsync(
      `INSERT INTO items (id, seq, origin, kind, text, files, from_device, app, copied_at, updated_at, blob_size, blob_state, width, height)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
       ON CONFLICT(id) DO UPDATE SET
         seq = excluded.seq, origin = excluded.origin, kind = excluded.kind, text = excluded.text,
         files = excluded.files, from_device = excluded.from_device, app = excluded.app,
         copied_at = excluded.copied_at, updated_at = excluded.updated_at, blob_size = excluded.blob_size,
         blob_state = CASE WHEN items.blob_state = 'ready' THEN 'ready' ELSE excluded.blob_state END,
         width = COALESCE(items.width, excluded.width), height = COALESCE(items.height, excluded.height)`,
      item.id,
      item.seq,
      item.origin,
      item.kind,
      item.text,
      item.files ? JSON.stringify(item.files) : null,
      item.from,
      item.app,
      item.copied_at,
      item.updated_at,
      item.blob_size,
      item.blob_state ?? (item.blob_size == null ? "none" : "missing"),
      item.width ?? null,
      item.height ?? null,
    );
  }

  async delete(id: string): Promise<void> {
    await this.db.runAsync("DELETE FROM items WHERE id = ?", id);
  }

  async setBlobState(id: string, state: BlobState, size?: { width: number; height: number }): Promise<void> {
    if (size) {
      await this.db.runAsync("UPDATE items SET blob_state = ?, width = ?, height = ? WHERE id = ?", state, size.width, size.height, id);
    } else {
      await this.db.runAsync("UPDATE items SET blob_state = ? WHERE id = ?", state, id);
    }
  }

  async get(id: string): Promise<Item | null> {
    const row = await this.db.getFirstAsync<Row>("SELECT * FROM items WHERE id = ?", id);
    return row ? fromRow(row) : null;
  }

  async list(search: string, limit: number, offset = 0): Promise<Item[]> {
    const needle = search.trim();
    const rows = needle
      ? await this.db.getAllAsync<Row>(
          `SELECT * FROM items WHERE text LIKE ? ESCAPE '\\' OR files LIKE ? ESCAPE '\\' OR from_device LIKE ? ESCAPE '\\' OR app LIKE ? ESCAPE '\\'
           ORDER BY copied_at DESC LIMIT ? OFFSET ?`,
          like(needle),
          like(needle),
          like(needle),
          like(needle),
          limit,
          offset,
        )
      : await this.db.getAllAsync<Row>("SELECT * FROM items ORDER BY copied_at DESC LIMIT ? OFFSET ?", limit, offset);
    return rows.map(fromRow);
  }

  async count(): Promise<number> {
    const row = await this.db.getFirstAsync<{ n: number }>("SELECT COUNT(*) AS n FROM items");
    return row?.n ?? 0;
  }

  async ids(): Promise<string[]> {
    const rows = await this.db.getAllAsync<{ id: string }>("SELECT id FROM items");
    return rows.map((r) => r.id);
  }

  async getSync(key: string): Promise<string | null> {
    const row = await this.db.getFirstAsync<{ value: string }>("SELECT value FROM sync WHERE key = ?", key);
    return row?.value ?? null;
  }

  async setSync(key: string, value: string): Promise<void> {
    await this.db.runAsync("INSERT INTO sync (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", key, value);
  }

  /// Forgets everything: used on a relay reset and when leaving the relay.
  async clear(): Promise<void> {
    await this.db.execAsync("DELETE FROM items; DELETE FROM sync;");
  }
}

function like(needle: string): string {
  return `%${needle.replace(/[\\%_]/g, (c) => `\\${c}`)}%`;
}

function fromRow(row: Row): Item {
  let files: FileMeta[] | null = null;
  if (row.files) {
    try {
      files = JSON.parse(row.files) as FileMeta[];
    } catch {
      files = null;
    }
  }
  return {
    id: row.id,
    seq: row.seq,
    origin: row.origin,
    kind: row.kind,
    text: row.text,
    files,
    from: row.from_device,
    app: row.app,
    copied_at: row.copied_at,
    updated_at: row.updated_at,
    blob_size: row.blob_size,
    blob_state: row.blob_state,
    width: row.width,
    height: row.height,
  };
}
