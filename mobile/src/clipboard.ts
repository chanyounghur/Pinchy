// Moving items between the shared history and this phone's clipboard, and
// turning what other apps hand us into upload drafts.

import * as Clipboard from "expo-clipboard";
import { Directory, File, Paths } from "expo-file-system";
import { ImageManipulator, SaveFormat } from "expo-image-manipulator";
import * as MediaLibrary from "expo-media-library";
import * as Sharing from "expo-sharing";
import type { ShareIntent } from "expo-share-intent";
import { Platform, Share } from "react-native";
import { decodeBase64, encodeBase64 } from "./protocol/base64";
import type { Item } from "./store/db";
import type { SyncEngine } from "./sync";
import { fileFor, imageFile } from "./blobs";
import { isPhotoBundle } from "./ui/format";
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

/// Every picture of an item as local file URIs, in order: the one PNG of an
/// image item, or each file of a photo bundle. The caller makes sure the blob
/// is here first.
export function imageUris(item: Item): string[] {
  if (item.kind === "image") return [imageFile(item.id).uri];
  if (item.kind === "files" && item.files) return item.files.map((_, i) => fileFor(item.id, item.files!, i).uri);
  return [];
}

/// Puts one picture of an item on the clipboard as PNG.
export async function copyPicture(item: Item, index: number): Promise<void> {
  const uri = imageUris(item)[index];
  if (!uri) throw new Error("아직 받는 중이에요");
  const name = item.kind === "image" ? "x.png" : (item.files?.[index]?.name ?? "");
  const png = await toPng(uri, mimeOf(name));
  await Clipboard.setImageAsync(encodeBase64(png));
}

/// Saves pictures of an item to the photo album; all of them when no index
/// is given. Asks for add-only access the first time.
export async function savePictures(item: Item, index?: number): Promise<number> {
  const uris = imageUris(item);
  const chosen = index === undefined ? uris : uris.slice(index, index + 1);
  if (chosen.length === 0) throw new Error("아직 받는 중이에요");
  const permission = await MediaLibrary.requestPermissionsAsync(true);
  if (!permission.granted) throw new Error("사진 앱에 추가할 권한이 없어요. 설정에서 허용해 주세요.");
  for (const uri of chosen) await MediaLibrary.Asset.create(uri);
  return chosen.length;
}

/// Opens the share sheet for one picture of an item.
export async function sharePicture(item: Item, index: number): Promise<void> {
  const uri = imageUris(item)[index];
  if (!uri) throw new Error("아직 받는 중이에요");
  await Sharing.shareAsync(uri, { mimeType: item.kind === "image" ? "image/png" : mimeOf(item.files?.[index]?.name ?? "") });
}

function mimeOf(name: string): string {
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  if (ext === "png") return "image/png";
  if (ext === "jpg" || ext === "jpeg") return "image/jpeg";
  if (ext === "heic" || ext === "heif") return "image/heic";
  if (ext === "webp") return "image/webp";
  if (ext === "gif") return "image/gif";
  return "image/jpeg";
}

/// Saves an item outside the app: pictures to the album, anything else to
/// a folder the user picks (Android) or through the share sheet, where
/// "파일에 저장" lives (iOS). Text and links go as a .txt file. Returns a
/// note for the toast, or null when the share sheet said it for us.
export async function saveItem(item: Item): Promise<string | null> {
  if (item.kind === "image" || isPhotoBundle(item.kind, item.files)) {
    const n = await savePictures(item);
    return n > 1 ? `사진 ${n}장을 앨범에 저장했어요` : "앨범에 저장했어요";
  }
  const files = item.kind === "files" && item.files ? item.files.map((_, i) => fileFor(item.id, item.files!, i)) : [textFile(item)];
  if (Platform.OS === "android") {
    const dir = await Directory.pickDirectoryAsync();
    for (const f of files) f.copy(dir);
    return files.length > 1 ? `파일 ${files.length}개를 저장했어요` : "저장했어요";
  }
  for (const f of files) await Sharing.shareAsync(f.uri, { dialogTitle: f.name });
  return null;
}

/// Hands an item to the share sheet: text as text, pictures and files one
/// file at a time.
export async function shareItem(item: Item): Promise<void> {
  if (item.kind === "text" || item.kind === "link") {
    await Share.share(Platform.OS === "ios" ? { message: item.text ?? "" } : { message: item.text ?? "", title: "Pinchy" });
    return;
  }
  if (item.kind === "image") return shareImage(item);
  for (let i = 0; i < (item.files?.length ?? 0); i++) await shareFile(item, i);
}

/// Puts an item on the clipboard: text, the image, or the first photo of a
/// bundle. Other files cannot go on a clipboard.
export async function copyAny(item: Item): Promise<void> {
  if (item.kind === "text" || item.kind === "link") {
    await Clipboard.setStringAsync(item.text ?? "");
  } else if (item.kind === "image" || isPhotoBundle(item.kind, item.files)) {
    await copyPicture(item, 0);
  } else {
    throw new Error("파일은 복사할 수 없어요. 저장이나 공유를 써 주세요.");
  }
}

/// Text or a link as a .txt file in the cache, named after its first words.
function textFile(item: Item): File {
  const text = item.text ?? "";
  const head = text.trim().split(/\s+/).slice(0, 4).join(" ").replace(/[\\/:*?"<>|]/g, "").slice(0, 40) || "pinchy";
  const file = new File(Paths.cache, `${head}.txt`);
  if (file.exists) file.delete();
  file.write(text);
  return file;
}
