// Runs the phone's protocol code against a real relay binary. Set
// PINCHY_RELAY_BIN to the built `pinchy-relay` (cargo build in relay/);
// without it the test is skipped. Plays the desktop with the same TS code,
// which is fine: the Rust side is covered by vectors.json.

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Key } from "../src/protocol/key";
import { decodeBase64 } from "../src/protocol/base64";
import { decodeMeta, encodeMeta } from "../src/protocol/meta";
import { Relay } from "../src/protocol/relay";

const bin = process.env.PINCHY_RELAY_BIN;
const random = (n: number) => new Uint8Array(randomBytes(n));

test("a phone joins by QR, follows the desktop's history and adds to it", { skip: !bin && "PINCHY_RELAY_BIN not set" }, async () => {
  const dir = mkdtempSync(join(tmpdir(), "pinchy-relay-"));
  const port = 18000 + Math.floor(Math.random() * 1000);
  const relay = spawn(bin!, ["--data-dir", dir, "--listen", `127.0.0.1:${port}`], { stdio: ["ignore", "ignore", "pipe"] });
  let stderr = "";
  const firstCode = new Promise<string>((resolve, reject) => {
    relay.stderr.on("data", (chunk: Buffer) => {
      stderr += chunk.toString();
      const m = /first device code: (\S+)/.exec(stderr);
      if (m) resolve(m[1]!);
    });
    relay.on("exit", (code) => reject(new Error(`relay exited early (${code}): ${stderr}`)));
  });
  try {
    const code = await firstCode;
    await waitFor(`http://127.0.0.1:${port}/health`);
    const server = `http://127.0.0.1:${port}`;
    const key = Key.generate(random);

    // The desktop registers first and copies a text and an image.
    const first = await fetch(`${server}/v1/first`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code, label: key.sealLabel("집 맥") }),
    });
    assert.equal(first.status, 201);
    const desktop = new Relay(server, ((await first.json()) as { token: string }).token, fetch);
    const textId = hex(16);
    assert.deepEqual(
      await desktop.putItem(textId, key.seal("meta", textId, encodeMeta({ v: 2, kind: "text", text: "안녕", from: "집 맥", copied_at: 1 })), false),
      { blobPending: false },
    );
    const imageId = hex(16);
    const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 2, 0, 0, 0, 3, 8, 6, 0, 0, 0]);
    assert.deepEqual(
      await desktop.putItem(imageId, key.seal("meta", imageId, encodeMeta({ v: 2, kind: "image", from: "집 맥", app: "Preview", copied_at: 2 })), true),
      { blobPending: true },
    );
    await desktop.uploadBlob(imageId, key.seal("blob", imageId, png));

    // The desktop mints a join code; the phone scans it with the key.
    const codes = await fetch(`${server}/v1/join-codes`, { method: "POST", headers: { Authorization: `Bearer ${desktop.token}` } });
    assert.equal(codes.status, 201);
    const joinCode = ((await codes.json()) as { code: string }).code;
    const phoneKey = Key.parse(key.toString(), random)!;
    const joined = await new Relay(server, null, fetch).join(joinCode, phoneKey.sealLabel("내 iPhone"));
    const phone = new Relay(server, joined.token, fetch);
    assert.equal((await phone.me()).devices, 2);
    const devices = await phone.devices();
    assert.deepEqual(
      devices.devices.map((d) => phoneKey.openLabel(d.label)).sort(),
      ["내 iPhone", "집 맥"],
    );

    // First sync: everything from 0, opened with the key.
    const page = await phone.changes(0, 0);
    assert.notEqual(page, "reset");
    if (page === "reset") throw new Error("unreachable");
    assert.equal(page.items.length, 2);
    const opened = page.items.map((c) => decodeMeta(phoneKey.open("meta", c.id, decodeBase64(c.envelope!)!)!)!);
    assert.deepEqual(opened.map((m) => m.kind).sort(), ["image", "text"]);
    const imageChange = page.items.find((c) => c.id === imageId)!;
    assert.equal(imageChange.blob_size, png.length + 24 + 16);
    assert.deepEqual(phoneKey.open("blob", imageId, await phone.blob(imageId)), png);
    const policy = await phone.policy();
    assert.equal(policy.retention_days, 30);
    assert.equal(policy.usage?.items, 2);

    // The phone adds a link; the desktop's long-poll sees it.
    const waiting = desktop.changes(page.next, 5);
    const linkId = hex(16);
    await phone.putItem(linkId, phoneKey.seal("meta", linkId, encodeMeta({ v: 2, kind: "link", text: "https://expo.dev", from: "내 iPhone", copied_at: 3 })), false);
    const seen = await waiting;
    if (seen === "reset") throw new Error("unexpected reset");
    assert.equal(seen.items.length, 1);
    assert.equal(seen.items[0]!.origin, joined.device_id);
    assert.equal(decodeMeta(key.open("meta", linkId, decodeBase64(seen.items[0]!.envelope!)!)!)?.text, "https://expo.dev");

    // Deleting from the phone tombstones it for everyone; re-putting is refused.
    await phone.deleteItem(textId);
    const after = await phone.changes(seen.next, 0);
    if (after === "reset") throw new Error("unexpected reset");
    assert.deepEqual(after.items.map((c) => [c.id, c.deleted]), [[textId, true]]);
    assert.equal(await desktop.putItem(textId, key.seal("meta", textId, encodeMeta({ v: 2, kind: "text", text: "다시" })), false), "deleted");

    // A stale cursor far ahead is just empty; a bad token is unauthorized.
    const ahead = await phone.changes(after.next + 100, 0);
    if (ahead === "reset") throw new Error("unexpected reset");
    assert.equal(ahead.items.length, 0);
    await assert.rejects(new Relay(server, "nope", fetch).me(), /연결이 해제/);
  } finally {
    relay.kill();
    rmSync(dir, { recursive: true, force: true });
  }
});

function hex(n: number): string {
  return Buffer.from(randomBytes(n)).toString("hex");
}

async function waitFor(url: string): Promise<void> {
  for (let i = 0; i < 50; i++) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`relay did not come up at ${url}`);
}
