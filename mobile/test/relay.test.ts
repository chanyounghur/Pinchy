import { test } from "node:test";
import assert from "node:assert/strict";
import { Relay, RelayError, type FetchLike } from "../src/protocol/relay";

interface Call {
  url: string;
  method: string;
  headers: Record<string, string>;
  body: unknown;
}

function fake(responder: (call: Call) => Response): { fetch: FetchLike; calls: Call[] } {
  const calls: Call[] = [];
  const fetch: FetchLike = async (url, init) => {
    const call: Call = {
      url,
      method: init?.method ?? "GET",
      headers: (init?.headers as Record<string, string>) ?? {},
      body: init?.body,
    };
    calls.push(call);
    return responder(call);
  };
  return { fetch, calls };
}

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

test("sends the bearer token and decodes responses", async () => {
  const f = fake((call) => {
    if (call.url.endsWith("/v1/me")) return json(200, { id: "d1", devices: 3 });
    if (call.url.includes("/v1/changes?since=7&wait=25&limit=200"))
      return json(200, { items: [], next: 7, more: false });
    if (call.url.endsWith("/v1/items/abc")) return json(201, { blob_pending: true });
    if (call.url.endsWith("/v1/items/abc/blob")) return new Response(new Uint8Array([1, 2, 3]), { status: 200 });
    return json(404, {});
  });
  const relay = new Relay("https://relay.test", "tok", f.fetch);
  assert.deepEqual(await relay.me(), { id: "d1", devices: 3 });
  assert.equal(f.calls[0]?.headers.Authorization, "Bearer tok");
  assert.deepEqual(await relay.changes(7, 25), { items: [], next: 7, more: false });
  assert.deepEqual(await relay.putItem("abc", new Uint8Array([9]), true), { blobPending: true });
  assert.equal(JSON.parse(f.calls[2]!.body as string).envelope, "CQ==");
  assert.deepEqual(Array.from(await relay.blob("abc")), [1, 2, 3]);
});

test("maps the relay's statuses to outcomes and Korean messages", async () => {
  let status = 409;
  const f = fake(() => json(status, { error: "x" }));
  const relay = new Relay("https://relay.test", "tok", f.fetch);
  assert.equal(await relay.changes(0, 1), "reset", "409 on changes means start over");
  status = 410;
  assert.equal(await relay.putItem("abc", new Uint8Array([1]), false), "deleted");
  status = 401;
  await assert.rejects(relay.me(), (e: unknown) => e instanceof RelayError && e.status === 401 && /연결이 해제/.test(e.message));
  status = 403;
  await assert.rejects(relay.join("c", "l"), /코드가 틀렸거나 만료/);
  const down = new Relay("https://relay.test", null, async () => {
    throw new Error("ECONNREFUSED");
  });
  await assert.rejects(down.me(), (e: unknown) => e instanceof RelayError && e.status === null && /연결할 수 없어요/.test(e.message));
});

test("joining needs no token and sends the sealed label", async () => {
  const f = fake(() => json(201, { device_id: "d9", token: "t9" }));
  const relay = new Relay("https://relay.test", null, f.fetch);
  assert.deepEqual(await relay.join("code1", "sealed"), { device_id: "d9", token: "t9" });
  assert.equal(f.calls[0]?.headers.Authorization, undefined);
  assert.deepEqual(JSON.parse(f.calls[0]!.body as string), { code: "code1", label: "sealed" });
});
