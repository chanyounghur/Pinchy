// Moving items between the shared history and this phone's clipboard, and
// turning what other apps hand us into upload drafts.

import * as Clipboard from "expo-clipboard";
import { File } from "expo-file-system";
import { ImageManipulator, SaveFormat } from "expo-image-manipulator";
import * as Sharing from "expo-sharing";
import type { ShareIntent } from "expo-share-intent";
import { decodeBase64, encodeBase64 } from "./protocol/base64";
import type { Item } from "./store/db";
import type { SyncEngine } from "./sync";
import { fileFor, imageFile } from "./blobs";
import { isLink, type Draft } from "./upload";

/// Puts an item on the phone's clipboard. Files cannot go on a clipboard, so
/// they open the share sheet instead. Returns what happened for the toast.
export async function copyItem(engine: SyncEngine, item: Item): Promise<"copied" | "shared" | "pending"> {
  if (item.kind === "text" || item.kind === "link") {
    await Clipboard.setStringAsync(item.text ?? "");
    return "copied";
  }
  if (item.blob_state !== "ready") {
    const ok = await engine.ensureBlob(item.id);
    if (!ok) return "pending";
  }
  if (item.kind === "image") {
    const png = await imageFile(item.id).bytes();
    await Clipboard.setImageAsync(encodeBase64(png));
    return "copied";
  }
  if (item.kind === "files" && item.files) {
    await shareFile(item, 0);
    return "shared";
  }
  return "pending";
}

export async function shareFile(item: Item, index: number): Promise<void> {
  if (!item.files) return;
  const file = fileFor(item.id, item.files, index);
  await Sharing.shareAsync(file.uri, { dialogTitle: item.files[index]?.name });
}

export async function shareImage(item: Item): Promise<void> {
  await Sharing.shareAsync(imageFile(item.id).uri, { mimeType: "image/png" });
}

/// What is on the clipboard right now, as a draft. Null when it is empty.
export async function draftFromClipboard(): Promise<Draft | null> {
  if (await Clipboard.hasImageAsync()) {
    const image = await Clipboard.getImageAsync({ format: "png" });
    const png = image && dataUriBytes(image.data);
    if (png) return { kind: "image", png };
  }
  if (await Clipboard.hasStringAsync()) {
    const text = await Clipboard.getStringAsync();
    if (text.trim()) return { kind: isLink(text) ? "link" : "text", text };
  }
  return null;
}

/// What another app shared with us, as a draft.
export async function draftFromShare(intent: ShareIntent): Promise<Draft | null> {
  const files = intent.files ?? [];
  if (files.length === 1 && files[0]!.mimeType.startsWith("image/")) {
    const f = files[0]!;
    return { kind: "image", png: await toPng(f.path, f.mimeType), app: appOf(intent) };
  }
  if (files.length > 0) {
    const out: { name: string; bytes: Uint8Array }[] = [];
    for (const f of files) {
      out.push({ name: f.fileName || new File(f.path).name, bytes: await new File(f.path).bytes() });
    }
    return { kind: "files", files: out, app: appOf(intent) };
  }
  const text = intent.webUrl?.trim() || intent.text?.trim();
  if (text) return { kind: isLink(text) ? "link" : "text", text, app: appOf(intent) };
  return null;
}

/// Other apps do not tell us their name; iOS gives a page title at most.
function appOf(_intent: ShareIntent): string | undefined {
  return undefined;
}

/// Reads an image file as PNG bytes, converting other formats on the way.
export async function toPng(uri: string, mimeType: string): Promise<Uint8Array> {
  if (mimeType === "image/png") return new File(uri).bytes();
  const context = ImageManipulator.manipulate(uri);
  const rendered = await context.renderAsync();
  const saved = await rendered.saveAsync({ format: SaveFormat.PNG });
  const bytes = await new File(saved.uri).bytes();
  try {
    new File(saved.uri).delete();
  } catch {}
  return bytes;
}

function dataUriBytes(data: string): Uint8Array | null {
  const comma = data.indexOf(",");
  return decodeBase64(comma >= 0 ? data.slice(comma + 1) : data);
}
