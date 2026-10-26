// Base64 that works the same on Hermes and Node: the relay speaks standard
// base64 for envelopes and labels, and URL-safe base64 without padding for
// the sharing key in join links.

const STANDARD = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_SAFE = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

const LOOKUP = new Int16Array(256).fill(-1);
for (let i = 0; i < 64; i++) {
  LOOKUP[STANDARD.charCodeAt(i)] = i;
  LOOKUP[URL_SAFE.charCodeAt(i)] = i;
}

export function encodeBase64(bytes: Uint8Array, urlSafe = false): string {
  const alphabet = urlSafe ? URL_SAFE : STANDARD;
  const parts: string[] = [];
  let out = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const a = bytes[i]!;
    const b = i + 1 < bytes.length ? bytes[i + 1]! : 0;
    const c = i + 2 < bytes.length ? bytes[i + 2]! : 0;
    const n = (a << 16) | (b << 8) | c;
    out += alphabet[(n >> 18) & 63]! + alphabet[(n >> 12) & 63]!;
    out += i + 1 < bytes.length ? alphabet[(n >> 6) & 63]! : urlSafe ? "" : "=";
    out += i + 2 < bytes.length ? alphabet[n & 63]! : urlSafe ? "" : "=";
    // Keep string concatenation chunks small so large blobs do not thrash.
    if (out.length > 65536) {
      parts.push(out);
      out = "";
    }
  }
  parts.push(out);
  return parts.join("");
}

/// Accepts standard or URL-safe alphabets, padded or not. Returns null for
/// anything that is not base64.
export function decodeBase64(text: string): Uint8Array | null {
  let end = text.length;
  while (end > 0 && (text[end - 1] === "=" || text[end - 1] === "\n" || text[end - 1] === " ")) end--;
  const remainder = end % 4;
  if (remainder === 1) return null;
  const length = Math.floor(end / 4) * 3 + (remainder === 0 ? 0 : remainder - 1);
  const bytes = new Uint8Array(length);
  let o = 0;
  let buffer = 0;
  let bits = 0;
  for (let i = 0; i < end; i++) {
    const v = LOOKUP[text.charCodeAt(i)];
    if (v === undefined || v < 0) return null;
    buffer = (buffer << 6) | v;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      bytes[o++] = (buffer >> bits) & 255;
    }
  }
  return bytes;
}
