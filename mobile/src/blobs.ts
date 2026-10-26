// Where opened blobs live on the phone. Images are one PNG per item; files
// are a folder per item. Everything is derived from the relay and can be
// deleted at any time.

import { Directory, File, Paths } from "expo-file-system";
import { safeFileNames, splitFiles, type FileMeta } from "./protocol/meta";

const root = () => new Directory(Paths.document, "pinchy");

export function imageFile(id: string): File {
  return new File(root(), "images", `${id}.png`);
}

export function filesDir(id: string): Directory {
  return new Directory(root(), "files", id);
}

export function fileFor(id: string, files: FileMeta[], index: number): File {
  return new File(filesDir(id), safeFileNames(files)[index]!);
}

export async function writeImage(id: string, png: Uint8Array): Promise<File> {
  const file = imageFile(id);
  ensureParent(file);
  await file.write(png);
  return file;
}

/// Null when the sizes do not add up to the blob.
export async function writeFiles(id: string, files: FileMeta[], blob: Uint8Array): Promise<File[] | null> {
  const parts = splitFiles(files, blob);
  if (!parts) return null;
  const dir = filesDir(id);
  if (dir.exists) dir.delete();
  dir.create({ intermediates: true });
  const names = safeFileNames(files);
  const out: File[] = [];
  for (let i = 0; i < parts.length; i++) {
    const file = new File(dir, names[i]!);
    await file.write(parts[i]!);
    out.push(file);
  }
  return out;
}

export function removeBlob(id: string): void {
  try {
    const image = imageFile(id);
    if (image.exists) image.delete();
  } catch {}
  try {
    const dir = filesDir(id);
    if (dir.exists) dir.delete();
  } catch {}
}

export function removeAllBlobs(): void {
  try {
    const dir = root();
    if (dir.exists) dir.delete();
  } catch {}
}

function ensureParent(file: File): void {
  const parent = file.parentDirectory;
  if (!parent.exists) parent.create({ intermediates: true });
}

/// Width and height from a PNG header; null when the bytes are not a PNG.
export function pngSize(bytes: Uint8Array): { width: number; height: number } | null {
  if (bytes.length < 24) return null;
  const signature = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
  for (let i = 0; i < 8; i++) if (bytes[i] !== signature[i]) return null;
  if (bytes[12] !== 0x49 || bytes[13] !== 0x48 || bytes[14] !== 0x44 || bytes[15] !== 0x52) return null;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const width = view.getUint32(16);
  const height = view.getUint32(20);
  return width > 0 && height > 0 ? { width, height } : null;
}
