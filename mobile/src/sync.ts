// Follows the relay's change feed and keeps the local history equal to it.
// First connect pulls everything from seq 0; afterwards one long-poll at a
// time while the app is in the foreground. A 409 (behind the relay's floor)
// wipes the local copy and starts over, like the desktop's `FullSync::Reset`.

import { decodeBase64 } from "./protocol/base64";
import type { Key } from "./protocol/key";
import { decodeMeta, type Meta } from "./protocol/meta";
import { Relay, RelayError, type Change, type Policy } from "./protocol/relay";
import type { Db } from "./store/db";
import { pngSize, removeAllBlobs, removeBlob, writeFiles, writeImage } from "./blobs";

export type SyncStatus =
  | { state: "idle" }
  | { state: "syncing"; initial: boolean }
  | { state: "live" }
  | { state: "offline"; message: string }
  | { state: "unauthorized" };

export type SyncEvent = "items" | "status" | "policy";

const SINCE = "since";
const INITIAL_DONE = "initial_done";
const WAIT_SECS = 25;
const POLICY_REFRESH_MS = 10 * 60 * 1000;
const RETRY_MS = [2000, 5000, 10000, 30000];

export class SyncEngine {
  status: SyncStatus = { state: "idle" };
  policy: Policy | null = null;
  /// Items that exist on the relay but were sealed with a different key.
  locked = 0;

  private listeners = new Map<SyncEvent, Set<() => void>>();
  private running = false;
  private abort: AbortController | null = null;
  private wake: (() => void) | null = null;
  private policyAt = 0;
  private inflight = new Map<string, Promise<boolean>>();

  constructor(
    readonly relay: Relay,
    readonly key: Key,
    readonly db: Db,
    readonly deviceId: string,
  ) {}

  on(event: SyncEvent, listener: () => void): () => void {
    let set = this.listeners.get(event);
    if (!set) this.listeners.set(event, (set = new Set()));
    set.add(listener);
    return () => set!.delete(listener);
  }

  private emit(event: SyncEvent): void {
    for (const l of this.listeners.get(event) ?? []) l();
  }

  private setStatus(status: SyncStatus): void {
    this.status = status;
    this.emit("status");
  }

  /// Runs until `stop()`. Safe to call again after stopping.
  start(): void {
    if (this.running) return;
    this.running = true;
    void this.loop();
  }

  stop(): void {
    this.running = false;
    this.abort?.abort();
    this.wake?.();
    if (this.status.state !== "unauthorized") this.setStatus({ state: "idle" });
  }

  /// Interrupts the current long-poll so the next request goes out now.
  poke(): void {
    this.abort?.abort();
    this.wake?.();
  }

  private async loop(): Promise<void> {
    let failures = 0;
    while (this.running) {
      try {
        await this.refreshPolicy();
        const initialDone = (await this.db.getSync(INITIAL_DONE)) === "1";
        let since = Number((await this.db.getSync(SINCE)) ?? "0");
        if (!initialDone) {
          this.setStatus({ state: "syncing", initial: true });
          since = await this.pullAll(since);
          await this.db.setSync(INITIAL_DONE, "1");
          this.emit("items");
        } else if (this.status.state !== "live") {
          // Back from offline, the background or a fresh start: catch up on
          // what happened meanwhile before claiming to be in sync.
          since = await this.pullAll(since);
        }
        this.setStatus({ state: "live" });
        failures = 0;
        this.abort = new AbortController();
        const page = await this.relay.changes(since, WAIT_SECS, undefined, this.abort.signal);
        this.abort = null;
        if (!this.running) break;
        if (page === "reset") {
          await this.reset();
          continue;
        }
        if (page.items.length > 0) {
          await this.applyPage(page.items);
          this.emit("items");
        }
        since = page.next;
        await this.db.setSync(SINCE, String(since));
      } catch (e) {
        this.abort = null;
        if (!this.running) break;
        if (isAbort(e)) continue;
        if (e instanceof RelayError && e.status === 401) {
          this.setStatus({ state: "unauthorized" });
          this.running = false;
          break;
        }
        this.setStatus({ state: "offline", message: e instanceof Error ? e.message : String(e) });
        await this.sleep(RETRY_MS[Math.min(failures, RETRY_MS.length - 1)]!);
        failures++;
      }
    }
  }

  private async pullAll(from: number): Promise<number> {
    let since = from;
    for (;;) {
      const page = await this.relay.changes(since, 0);
      // Only once a request has succeeded, so a dead network keeps showing offline.
      if (this.status.state !== "syncing") this.setStatus({ state: "syncing", initial: false });
      if (page === "reset") {
        await this.reset();
        since = 0;
        continue;
      }
      if (page.items.length > 0) {
        await this.applyPage(page.items);
        this.emit("items");
      }
      since = page.next;
      await this.db.setSync(SINCE, String(since));
      if (!page.more) return since;
    }
  }

  private async reset(): Promise<void> {
    await this.db.clear();
    removeAllBlobs();
    this.locked = 0;
    this.emit("items");
  }

  private async applyPage(changes: Change[]): Promise<void> {
    for (const change of changes) await this.apply(change);
  }

  private async apply(change: Change): Promise<void> {
    if (change.deleted || !change.envelope) {
      await this.db.delete(change.id);
      removeBlob(change.id);
      return;
    }
    const sealed = decodeBase64(change.envelope);
    const opened = sealed && this.key.open("meta", change.id, sealed);
    if (!opened) {
      this.locked++;
      return;
    }
    const meta = decodeMeta(opened);
    if (!meta || !acceptable(meta, change)) return;
    await this.db.upsert({
      id: change.id,
      seq: change.seq,
      origin: change.origin,
      kind: meta.kind,
      text: meta.text ?? null,
      files: meta.files ?? null,
      from: meta.from?.trim() || "알 수 없는 기기",
      app: meta.app ?? null,
      color: meta.color ?? null,
      icon: meta.icon ?? null,
      copied_at: meta.copied_at ?? change.updated_at,
      updated_at: change.updated_at,
      blob_size: change.blob_size ?? null,
    });
  }

  /// Downloads and opens an item's blob if it is not on disk yet. Resolves
  /// true when the blob is ready.
  ensureBlob(id: string): Promise<boolean> {
    let pending = this.inflight.get(id);
    if (!pending) {
      pending = this.fetchBlob(id).finally(() => this.inflight.delete(id));
      this.inflight.set(id, pending);
    }
    return pending;
  }

  private async fetchBlob(id: string): Promise<boolean> {
    const item = await this.db.get(id);
    if (!item || item.blob_size == null) return false;
    if (item.blob_state === "ready") return true;
    try {
      const sealed = await this.relay.blob(id);
      const blob = this.key.open("blob", id, sealed);
      if (!blob) throw new Error("blob does not open");
      if (item.kind === "image") {
        const size = pngSize(blob);
        if (!size) throw new Error("blob is not a PNG");
        await writeImage(id, blob);
        await this.db.setBlobState(id, "ready", size);
      } else if (item.kind === "files" && item.files) {
        if (!(await writeFiles(id, item.files, blob))) throw new Error("file sizes do not match");
        await this.db.setBlobState(id, "ready");
      } else {
        return false;
      }
      this.emit("items");
      return true;
    } catch (e) {
      if (e instanceof RelayError && e.status === 404) {
        // Gone from the relay between the change and now: the tombstone follows.
        await this.db.setBlobState(id, "failed");
      } else {
        await this.db.setBlobState(id, "failed");
      }
      this.emit("items");
      return false;
    }
  }

  async refreshPolicy(force = false): Promise<Policy | null> {
    const now = Date.now();
    if (!force && this.policy && now - this.policyAt < POLICY_REFRESH_MS) return this.policy;
    this.policy = await this.relay.policy();
    this.policyAt = now;
    this.emit("policy");
    return this.policy;
  }

  /// Local removal is immediate; the relay's tombstone confirms it to all.
  async deleteItem(id: string): Promise<void> {
    await this.db.delete(id);
    removeBlob(id);
    this.emit("items");
    await this.relay.deleteItem(id);
    this.poke();
  }

  async deleteAll(): Promise<void> {
    await this.relay.deleteAll();
    await this.db.clear();
    await this.db.setSync(INITIAL_DONE, "1");
    removeAllBlobs();
    this.emit("items");
    this.poke();
  }

  private sleep(ms: number): Promise<void> {
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        this.wake = null;
        resolve();
      }, ms);
      this.wake = () => {
        clearTimeout(timer);
        this.wake = null;
        resolve();
      };
    });
  }
}

function acceptable(meta: Meta, change: Change): boolean {
  switch (meta.kind) {
    case "text":
    case "link":
      return typeof meta.text === "string" && meta.text.trim().length > 0;
    case "image":
      return change.blob_size != null;
    case "files":
      return change.blob_size != null && Array.isArray(meta.files) && meta.files.length > 0;
    default:
      return false;
  }
}

function isAbort(e: unknown): boolean {
  return e instanceof Error && (e.name === "AbortError" || /abort/i.test(e.message));
}
