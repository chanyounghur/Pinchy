// Putting something from this phone into the shared history: seal the meta
// and the blob with the sharing key, PUT them, and record the item locally
// so it shows up before the change feed echoes it back.

import * as Crypto from "expo-crypto";
import { encodeMeta, type FileMeta, type Kind } from "./protocol/meta";
import { concatFiles } from "./protocol/meta";
import type { SyncEngine } from "./sync";
import { pngSize, writeFiles, writeImage } from "./blobs";

export interface Draft {
  kind: Kind;
  text?: string;
  /// PNG bytes for images.
  png?: Uint8Array;
  files?: { name: string; bytes: Uint8Array }[];
  /// App the content came from, when known.
  app?: string;
}

export const MB = 1024 * 1024;

export function newItemId(): string {
  const bytes = Crypto.getRandomBytes(16);
  let hex = "";
  for (const b of bytes) hex += b.toString(16).padStart(2, "0");
  return hex;
}

export function isLink(text: string): boolean {
  const t = text.trim();
  return /^https?:\/\/\S+$/i.test(t) && !/\s/.test(t);
}

/// Null when the draft fits the relay's limits, else why not.
export function tooLarge(draft: Draft, maxBlobMb: number): string | null {
  const size = draft.png?.length ?? draft.files?.reduce((n, f) => n + f.bytes.length, 0) ?? 0;
  if (size > maxBlobMb * MB) return `너무 커요. 서버가 받는 최대 크기는 ${maxBlobMb}MB예요.`;
  return null;
}

export async function upload(engine: SyncEngine, deviceName: string, draft: Draft): Promise<string> {
  const id = newItemId();
  const now = Date.now();
  const files: FileMeta[] | undefined = draft.files?.map((f) => ({ name: f.name, size: f.bytes.length }));
  const blob = draft.kind === "image" ? draft.png : draft.kind === "files" && draft.files ? concatFiles(draft.files.map((f) => f.bytes)) : undefined;
  const meta = {
    v: 2,
    kind: draft.kind,
    text: draft.kind === "text" || draft.kind === "link" ? draft.text : undefined,
    files,
    from: deviceName,
    app: draft.app,
    copied_at: now,
  };
  const envelope = engine.key.seal("meta", id, encodeMeta(meta));
  const result = await engine.relay.putItem(id, envelope, blob !== undefined);
  if (result === "deleted") throw new Error("서버가 이 항목을 이미 지운 것으로 알고 있어요.");
  if (result.blobPending && blob) {
    await engine.relay.uploadBlob(id, engine.key.seal("blob", id, blob));
  }
  // Keep a local copy so the item appears at once and its blob is not re-downloaded.
  let size: { width: number; height: number } | null = null;
  if (draft.kind === "image" && draft.png) {
    size = pngSize(draft.png);
    await writeImage(id, draft.png);
  } else if (draft.kind === "files" && files && blob) {
    await writeFiles(id, files, blob);
  }
  await engine.db.upsert({
    id,
    seq: 0,
    origin: engine.deviceId,
    kind: draft.kind,
    text: meta.text ?? null,
    files: files ?? null,
    from: deviceName,
    app: draft.app ?? null,
    color: null,
    copied_at: now,
    updated_at: now,
    blob_size: blob?.length ?? null,
    blob_state: blob ? "ready" : "none",
    width: size?.width ?? null,
    height: size?.height ?? null,
  });
  engine.poke();
  return id;
}
