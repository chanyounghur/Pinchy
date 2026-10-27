// Drafts from the phone's own pickers: photos from the album and files
// from the Files app or the Android document picker. One photo becomes an
// image item like a shared screenshot does; several become a file bundle.

import * as DocumentPicker from "expo-document-picker";
import { File } from "expo-file-system";
import * as ImagePicker from "expo-image-picker";
import { toPng } from "./clipboard";
import type { Draft } from "./upload";

/// Opens the album. Null when the user picked nothing.
export async function pickPhotos(): Promise<Draft | null> {
  const result = await ImagePicker.launchImageLibraryAsync({
    mediaTypes: ["images"],
    allowsMultipleSelection: true,
    selectionLimit: 20,
    quality: 1,
    exif: false,
  });
  if (result.canceled || result.assets.length === 0) return null;
  if (result.assets.length === 1) {
    const a = result.assets[0]!;
    return { kind: "image", png: await toPng(a.uri, a.mimeType ?? "image/jpeg") };
  }
  const files: { name: string; bytes: Uint8Array }[] = [];
  for (const a of result.assets) {
    const file = new File(a.uri);
    files.push({ name: a.fileName || file.name, bytes: await file.bytes() });
  }
  return { kind: "files", files };
}

/// Opens the system file picker. Null when the user picked nothing.
export async function pickFiles(): Promise<Draft | null> {
  const result = await DocumentPicker.getDocumentAsync({ multiple: true, copyToCacheDirectory: true });
  if (result.canceled || result.assets.length === 0) return null;
  const files: { name: string; bytes: Uint8Array }[] = [];
  for (const a of result.assets) {
    const file = new File(a.uri);
    files.push({ name: a.name || file.name, bytes: await file.bytes() });
  }
  return { kind: "files", files };
}

// A draft picked on the add sheet is handed to the upload screen here, since
// bytes do not travel through route params.
let pending: Draft | null = null;

export function setPendingDraft(draft: Draft): void {
  pending = draft;
}

export function takePendingDraft(): Draft | null {
  const d = pending;
  pending = null;
  return d;
}
