// Deep links before expo-router matches them. The iOS share extension opens
// the app with pinchy:///dataUrl=pinchyShareKey?nonce=…, which is not a
// route: it only tells expo-share-intent to read what was shared. Send it to
// the history, which opens the upload screen once the share intent is in.

import { getShareExtensionKey } from "expo-share-intent";

export function redirectSystemPath({ path }: { path: string; initial: boolean }): string {
  try {
    if (path.includes(`dataUrl=${getShareExtensionKey()}`)) return "/";
    return path;
  } catch {
    return "/";
  }
}
