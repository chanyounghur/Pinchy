// Over-the-air updates. The relay serves the manifest the release workflow
// signed (see relay/src/updates.rs); the phone verifies it against the
// certificate built into the app and downloads the bundle from GitHub.
// Checks happen when the app comes to the foreground, at most every few
// minutes; a downloaded update applies on the next cold start or when the
// user asks for it in settings.

import * as Updates from "expo-updates";
import type { Session } from "./store/session";

const CHECK_EVERY_MS = 10 * 60 * 1000;
let lastCheck = 0;

export const updatesEnabled = Updates.isEnabled && !__DEV__;

/// Points expo-updates at the relay this phone belongs to, with its token.
export function configureUpdates(session: Session | null): void {
  if (!updatesEnabled) return;
  try {
    Updates.setUpdateURLAndRequestHeadersOverride(
      session
        ? { updateUrl: `${session.server}/updates/manifest`, requestHeaders: { Authorization: `Bearer ${session.token}` } }
        : null,
    );
  } catch (e) {
    console.warn("expo-updates override failed", e);
  }
}

export type UpdateCheck = "disabled" | "skipped" | "none" | "downloaded" | "failed";

/// Checks for and downloads a newer update. `force` ignores the throttle.
export async function checkForUpdate(force = false): Promise<UpdateCheck> {
  if (!updatesEnabled) return "disabled";
  const now = Date.now();
  if (!force && now - lastCheck < CHECK_EVERY_MS) return "skipped";
  lastCheck = now;
  try {
    const check = await Updates.checkForUpdateAsync();
    if (!check.isAvailable) return "none";
    const fetched = await Updates.fetchUpdateAsync();
    return fetched.isNew ? "downloaded" : "none";
  } catch (e) {
    console.warn("expo-updates check failed", e);
    return "failed";
  }
}

export async function applyUpdate(): Promise<void> {
  await Updates.reloadAsync();
}

export function currentUpdate(): { id: string | null; createdAt: Date | null; runtime: string | null; embedded: boolean } {
  return {
    id: Updates.updateId ?? null,
    createdAt: Updates.createdAt ?? null,
    runtime: Updates.runtimeVersion ?? null,
    embedded: Updates.isEmbeddedLaunch,
  };
}
