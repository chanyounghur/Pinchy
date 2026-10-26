// What ties this phone to a relay: the server, this device's token and the
// sharing key. All of it lives in the OS secure store (Keychain / Keystore),
// never in the SQLite history.

import * as SecureStore from "expo-secure-store";
import { Platform } from "react-native";

export interface Session {
  server: string;
  deviceId: string;
  token: string;
  /// URL-safe base64 sharing key.
  key: string;
  /// This device's name as other devices see it.
  name: string;
}

const KEY = "pinchy.session";

export async function loadSession(): Promise<Session | null> {
  const raw = await SecureStore.getItemAsync(KEY);
  if (!raw) return null;
  try {
    const s = JSON.parse(raw) as Partial<Session>;
    if (!s.server || !s.deviceId || !s.token || !s.key) return null;
    return { server: s.server, deviceId: s.deviceId, token: s.token, key: s.key, name: s.name ?? defaultDeviceName() };
  } catch {
    return null;
  }
}

export async function saveSession(session: Session): Promise<void> {
  await SecureStore.setItemAsync(KEY, JSON.stringify(session), {
    keychainAccessible: SecureStore.AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY,
  });
}

export async function clearSession(): Promise<void> {
  await SecureStore.deleteItemAsync(KEY);
}

export function defaultDeviceName(): string {
  return Platform.OS === "ios" ? "내 iPhone" : "내 Android";
}
