// The sealed description of an item (`Meta` in share.rs). Text travels in
// the envelope; image and file bytes travel as a separate blob.

export type Kind = "text" | "link" | "image" | "files";

export interface FileMeta {
  name: string;
  size: number;
}

export interface Meta {
  v: number;
  kind: string;
  text?: string;
  /// For files, their names and sizes in blob order.
  files?: FileMeta[];
  /// Name of the device the item was first copied on.
  from?: string;
  /// Name of the app it was copied from.
  app?: string;
  /// When it was last copied, ms since the epoch.
  copied_at?: number;
  /// The app's card color (`#rrggbb`) as the copying device shows it.
  color?: string;
  /// The app's icon as a base64 PNG (64px), so this device can show it
  /// without having the app.
  icon?: string;
}

/// A 64px PNG is a few KB; anything bigger is not an icon.
export const MAX_ICON_CHARS = 64 * 1024 * 4 / 3 + 4;

export const META_VERSION = 2;

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export function encodeMeta(meta: Meta): Uint8Array {
  const clean: Record<string, unknown> = { v: meta.v, kind: meta.kind };
  if (meta.text !== undefined) clean.text = meta.text;
  if (meta.files !== undefined) clean.files = meta.files;
  if (meta.from !== undefined) clean.from = meta.from;
  if (meta.app !== undefined) clean.app = meta.app;
  if (meta.copied_at !== undefined) clean.copied_at = meta.copied_at;
  if (meta.color !== undefined) clean.color = meta.color;
  if (meta.icon !== undefined) clean.icon = meta.icon;
  return encoder.encode(JSON.stringify(clean));
}

/// Null when the bytes are not a meta we understand.
export function decodeMeta(bytes: Uint8Array): Meta | null {
  let raw: unknown;
  try {
    raw = JSON.parse(decoder.decode(bytes));
  } catch {
    return null;
  }
  if (typeof raw !== "object" || raw === null) return null;
  const r = raw as Record<string, unknown>;
  if (typeof r.v !== "number" || typeof r.kind !== "string") return null;
  const meta: Meta = { v: r.v, kind: r.kind };
  if (typeof r.text === "string") meta.text = r.text;
  if (Array.isArray(r.files)) {
    const files: FileMeta[] = [];
    for (const f of r.files) {
      if (typeof f !== "object" || f === null) return null;
      const { name, size } = f as Record<string, unknown>;
      if (typeof name !== "string" || typeof size !== "number" || size < 0) return null;
      files.push({ name, size });
    }
    meta.files = files;
  }
  if (typeof r.from === "string") meta.from = r.from;
  if (typeof r.app === "string") meta.app = r.app;
  if (typeof r.copied_at === "number") meta.copied_at = r.copied_at;
  if (typeof r.color === "string" && HEX_COLOR.test(r.color.trim())) meta.color = r.color.trim().toLowerCase();
  if (typeof r.icon === "string" && r.icon.length <= MAX_ICON_CHARS && PNG_BASE64.test(r.icon)) meta.icon = r.icon;
  return meta;
}

const HEX_COLOR = /^#[0-9a-fA-F]{6}$/;
/// Standard base64 that starts with the PNG signature (`iVBORw0KGgo`).
const PNG_BASE64 = /^iVBORw0KGgo[A-Za-z0-9+/]*={0,2}$/;

/// Splits a files blob back into its parts. Null when the sizes do not add
/// up to the blob.
export function splitFiles(files: FileMeta[], blob: Uint8Array): Uint8Array[] | null {
  if (files.length === 0) return null;
  let total = 0;
  for (const f of files) total += f.size;
  if (total !== blob.length) return null;
  const out: Uint8Array[] = [];
  let offset = 0;
  for (const f of files) {
    out.push(blob.subarray(offset, offset + f.size));
    offset += f.size;
  }
  return out;
}

export function concatFiles(parts: Uint8Array[]): Uint8Array {
  let total = 0;
  for (const p of parts) total += p.length;
  const out = new Uint8Array(total);
  let offset = 0;
  for (const p of parts) {
    out.set(p, offset);
    offset += p.length;
  }
  return out;
}

/// File names made safe to create on the phone and unique within an item,
/// matching the desktop's `split_files`.
export function safeFileNames(files: FileMeta[]): string[] {
  const seen = new Set<string>();
  return files.map((f, index) => {
    let name = f.name.replace(/[\\/:*?"<>|\u0000-\u001f]/g, "_").trim();
    if (name === "" || name === "." || name === "..") name = `file-${index + 1}`;
    let unique = name;
    for (let n = 2; seen.has(unique.toLowerCase()); n++) {
      const dot = name.lastIndexOf(".");
      unique = dot > 0 ? `${name.slice(0, dot)} (${n})${name.slice(dot)}` : `${name} (${n})`;
    }
    seen.add(unique.toLowerCase());
    return unique;
  });
}
