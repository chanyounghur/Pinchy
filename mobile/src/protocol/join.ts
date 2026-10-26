// Join links are what the desktop shows as a QR: `{server}/#join?code=…&key=…`.
// The key rides in the fragment so it never reaches the relay.

export interface JoinLink {
  server: string;
  code: string;
  key: string;
}

export function parseJoinLink(text: string): JoinLink | null {
  const trimmed = text.trim();
  const hash = trimmed.indexOf("#");
  if (hash < 0) return null;
  const base = trimmed.slice(0, hash);
  const fragment = trimmed.slice(hash + 1);
  if (!fragment.startsWith("join?")) return null;
  let url: URL;
  try {
    url = new URL(base);
  } catch {
    return null;
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return null;
  const params = new URLSearchParams(fragment.slice("join?".length));
  const code = params.get("code")?.trim() ?? "";
  const key = params.get("key")?.trim() ?? "";
  if (code === "" || key === "") return null;
  return { server: `${url.protocol}//${url.host}`, code, key };
}
