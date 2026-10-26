import { test } from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
import { Key, NONCE_LEN } from "../src/protocol/key";
import { decodeBase64, encodeBase64 } from "../src/protocol/base64";
import { decodeMeta } from "../src/protocol/meta";

const random = (n: number) => new Uint8Array(randomBytes(n));

interface Vectors {
  key: string;
  id: string;
  meta_json: string;
  envelope: string;
  blob_hex: string;
  sealed_blob: string;
  label: string;
  sealed_label: string;
}

const vectors: Vectors = JSON.parse(readFileSync(new URL("./vectors.json", import.meta.url).pathname, "utf8"));

test("opens envelopes, blobs and labels sealed by the desktop", () => {
  const key = Key.parse(vectors.key, random);
  assert.ok(key, "key parses");
  const envelope = decodeBase64(vectors.envelope)!;
  const meta = key.open("meta", vectors.id, envelope);
  assert.ok(meta, "envelope opens");
  assert.equal(new TextDecoder().decode(meta), vectors.meta_json);
  const parsed = decodeMeta(meta);
  assert.equal(parsed?.kind, "files");
  assert.equal(parsed?.from, "집 맥");
  assert.equal(parsed?.color, "#3b5bdb");
  assert.deepEqual(parsed?.files, [
    { name: "a.txt", size: 3 },
    { name: "b.bin", size: 4 },
  ]);

  const blob = key.open("blob", vectors.id, decodeBase64(vectors.sealed_blob)!);
  assert.ok(blob);
  assert.equal(Buffer.from(blob).toString("hex"), vectors.blob_hex);

  assert.equal(key.openLabel(vectors.sealed_label), vectors.label);
  // The wrong part, item or key never opens.
  assert.equal(key.open("blob", vectors.id, envelope), null);
  assert.equal(key.open("meta", (vectors.id[0] === "0" ? "1" : "0") + vectors.id.slice(1), envelope), null);
  assert.equal(Key.generate(random).open("meta", vectors.id, envelope), null);
});

test("seals in the same shape it opens", () => {
  const key = Key.generate(random);
  const id = Buffer.from(randomBytes(16)).toString("hex");
  const plain = new TextEncoder().encode("hello");
  const sealed = key.seal("meta", id, plain);
  assert.equal(sealed.length, NONCE_LEN + plain.length + 16);
  assert.deepEqual(key.open("meta", id, sealed), plain);
  assert.notDeepEqual(key.seal("meta", id, plain), sealed, "nonces are random");
  const tampered = new Uint8Array(sealed);
  tampered[tampered.length - 1]! ^= 1;
  assert.equal(key.open("meta", id, tampered), null);
  assert.equal(key.open("meta", id, sealed.subarray(0, 10)), null);
  assert.equal(key.openLabel(key.sealLabel(" 집 맥 ")), "집 맥");
  assert.equal(Key.generate(random).openLabel(key.sealLabel("집 맥")), null);
  assert.equal(Key.parse(key.toString(), random)?.toString(), key.toString());
  assert.equal(key.toString().length, 43, "url-safe, unpadded");
  assert.equal(Key.parse("short", random), null);
});

test("base64 matches Node for every alphabet and length", () => {
  for (let length = 0; length < 40; length++) {
    const bytes = new Uint8Array(randomBytes(length));
    const standard = Buffer.from(bytes).toString("base64");
    const url = Buffer.from(bytes).toString("base64url");
    assert.equal(encodeBase64(bytes), standard);
    assert.equal(encodeBase64(bytes, true), url);
    assert.deepEqual(decodeBase64(standard), bytes);
    assert.deepEqual(decodeBase64(url), bytes);
  }
  assert.equal(decodeBase64("a"), null);
  assert.equal(decodeBase64("!!!!"), null);
  const big = new Uint8Array(randomBytes(300_000));
  assert.deepEqual(decodeBase64(encodeBase64(big)), big);
});
