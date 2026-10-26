// The sharing key: XChaCha20-Poly1305 with the nonce in front of the
// ciphertext and the part and item id bound as associated data, exactly as
// the desktop's `Key` in src-tauri/src/share.rs. Everything a device sends
// to the relay goes through `seal`; the relay only ever sees the output.

import { xchacha20poly1305 } from "@noble/ciphers/chacha.js";
import { decodeBase64, encodeBase64 } from "./base64";

export const KEY_LEN = 32;
export const NONCE_LEN = 24;
const TAG_LEN = 16;

export type RandomBytes = (length: number) => Uint8Array;

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export class Key {
  private constructor(
    private readonly bytes: Uint8Array,
    private readonly random: RandomBytes,
  ) {}

  /// Parses the URL-safe base64 form used in join links and pairing.
  static parse(text: string, random: RandomBytes): Key | null {
    const bytes = decodeBase64(text.trim());
    return bytes && bytes.length === KEY_LEN ? new Key(bytes, random) : null;
  }

  static fromBytes(bytes: Uint8Array, random: RandomBytes): Key | null {
    return bytes.length === KEY_LEN ? new Key(new Uint8Array(bytes), random) : null;
  }

  static generate(random: RandomBytes): Key {
    return new Key(random(KEY_LEN), random);
  }

  /// The URL-safe base64 form, for the secure store and join links.
  toString(): string {
    return encodeBase64(this.bytes, true);
  }

  seal(part: string, id: string, plaintext: Uint8Array): Uint8Array {
    const nonce = this.random(NONCE_LEN);
    const sealed = xchacha20poly1305(this.bytes, nonce, aad(part, id)).encrypt(plaintext);
    const out = new Uint8Array(NONCE_LEN + sealed.length);
    out.set(nonce, 0);
    out.set(sealed, NONCE_LEN);
    return out;
  }

  /// Null when the bytes were sealed with another key, for another part or
  /// item, or were tampered with.
  open(part: string, id: string, sealed: Uint8Array): Uint8Array | null {
    if (sealed.length < NONCE_LEN + TAG_LEN) return null;
    try {
      return xchacha20poly1305(this.bytes, sealed.subarray(0, NONCE_LEN), aad(part, id)).decrypt(
        sealed.subarray(NONCE_LEN),
      );
    } catch {
      return null;
    }
  }

  sealLabel(name: string): string {
    return encodeBase64(this.seal("label", "", encoder.encode(name)));
  }

  openLabel(label: string): string | null {
    const sealed = decodeBase64(label);
    const opened = sealed && this.open("label", "", sealed);
    if (!opened) return null;
    const name = decoder.decode(opened).trim();
    return name.length > 0 ? name : null;
  }
}

function aad(part: string, id: string): Uint8Array {
  return encoder.encode(`pinchy/v1/${part}/${id}`);
}
