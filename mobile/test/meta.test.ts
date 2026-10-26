import { test } from "node:test";
import assert from "node:assert/strict";
import { concatFiles, decodeMeta, encodeMeta, safeFileNames, splitFiles } from "../src/protocol/meta";
import { parseJoinLink } from "../src/protocol/join";
import { normalizeServer } from "../src/protocol/relay";

test("meta round-trips and drops unknown or malformed fields", () => {
  const meta = { v: 2, kind: "text", text: "hi", from: "집 맥", copied_at: 5, color: "#3b5bdb" };
  assert.deepEqual(decodeMeta(encodeMeta(meta)), meta);
  assert.equal(decodeMeta(new TextEncoder().encode('{"v":2,"kind":"text","text":"x","color":"red"}'))?.color, undefined, "only #rrggbb");
  assert.equal(decodeMeta(new TextEncoder().encode('{"v":2,"kind":"text","text":"x","color":" #ABCDEF "}'))?.color, "#abcdef");
  const json = new TextDecoder().decode(encodeMeta({ v: 2, kind: "image", app: undefined }));
  assert.equal(json, '{"v":2,"kind":"image"}', "undefined fields are omitted like serde's skip_serializing_if");
  assert.equal(decodeMeta(new TextEncoder().encode("nope")), null);
  assert.equal(decodeMeta(new TextEncoder().encode('{"kind":"text"}')), null);
  assert.equal(decodeMeta(new TextEncoder().encode('{"v":2,"kind":"files","files":[{"name":1}]}')), null);
  const extra = decodeMeta(new TextEncoder().encode('{"v":3,"kind":"text","text":"x","future":true}'));
  assert.deepEqual(extra, { v: 3, kind: "text", text: "x" });
});

test("files blobs split by the declared sizes", () => {
  const parts = [new Uint8Array([1, 2, 3]), new Uint8Array([]), new Uint8Array([4])];
  const blob = concatFiles(parts);
  const files = [
    { name: "a", size: 3 },
    { name: "b", size: 0 },
    { name: "c", size: 1 },
  ];
  assert.deepEqual(splitFiles(files, blob)?.map((p) => Array.from(p)), [[1, 2, 3], [], [4]]);
  assert.equal(splitFiles(files, blob.subarray(1)), null, "sizes must add up");
  assert.equal(splitFiles([], new Uint8Array()), null);
  assert.deepEqual(safeFileNames([{ name: "a/b:c.txt", size: 0 }, { name: "a_b_c.txt", size: 0 }, { name: "", size: 0 }]), [
    "a_b_c.txt",
    "a_b_c (2).txt",
    "file-3",
  ]);
});

test("join links carry server, code and key", () => {
  assert.deepEqual(parseJoinLink("https://relay.example.com/#join?code=abc123&key=k_-x"), {
    server: "https://relay.example.com",
    code: "abc123",
    key: "k_-x",
  });
  assert.deepEqual(parseJoinLink(" http://127.0.0.1:8787/#join?key=k&code=c "), {
    server: "http://127.0.0.1:8787",
    code: "c",
    key: "k",
  });
  assert.equal(parseJoinLink("https://relay.example.com/#other?code=a&key=b"), null);
  assert.equal(parseJoinLink("https://relay.example.com/#join?code=a"), null);
  assert.equal(parseJoinLink("hello"), null);
});

test("server addresses normalize to an origin", () => {
  assert.equal(normalizeServer("relay.example.com"), "https://relay.example.com");
  assert.equal(normalizeServer("https://relay.example.com/"), "https://relay.example.com");
  assert.equal(normalizeServer("http://localhost:8787"), "http://localhost:8787");
  assert.equal(normalizeServer("http://relay.example.com"), null, "plain http only for loopback");
  assert.equal(normalizeServer("https://relay.example.com/path"), null);
  assert.equal(normalizeServer(""), null);
});
