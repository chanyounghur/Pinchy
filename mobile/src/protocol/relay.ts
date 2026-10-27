// HTTP client for the relay's `/v1` API. Mirrors `Http` and `Relay` in
// share.rs: bearer token per device, JSON in, JSON or raw bytes out. The
// fetch implementation is injected so tests run on Node and the app uses
// expo/fetch for streaming bodies.

import { encodeBase64 } from "./base64";

export interface Change {
  id: string;
  seq: number;
  origin: string;
  created_at: number;
  updated_at: number;
  deleted: boolean;
  /// Base64 of the encrypted envelope; absent for tombstones.
  envelope?: string;
  blob_size?: number;
}

export interface ChangePage {
  items: Change[];
  /// Pass this as `since` next time.
  next: number;
  /// Whether more changes follow `next` right now.
  more: boolean;
}

export interface Usage {
  items: number;
  bytes: number;
}

export interface Policy {
  retention_days: number;
  max_storage_mb: number;
  max_blob_mb: number;
  sync_concealed: boolean;
  usage: Usage | null;
}

export interface Device {
  id: string;
  /// Sealed label, base64.
  label: string;
  created_at: number;
  last_seen: number | null;
}

export type FetchLike = (input: string, init?: RequestInit) => Promise<Response>;

export class RelayError extends Error {
  constructor(
    message: string,
    readonly status: number | null,
  ) {
    super(message);
    this.name = "RelayError";
  }
}

export const MAX_WAIT_SECS = 30;
export const MAX_CHANGES = 200;

/// Accepts `host`, `host:port` or a full URL and returns `scheme://host[:port]`.
/// Plain HTTP is only for loopback test servers.
export function normalizeServer(input: string): string | null {
  let text = input.trim().replace(/\/+$/, "");
  if (text === "") return null;
  if (!/^https?:\/\//i.test(text)) text = `https://${text}`;
  let url: URL;
  try {
    url = new URL(text);
  } catch {
    return null;
  }
  if (url.pathname !== "/" && url.pathname !== "") return null;
  if (url.search !== "" || url.hash !== "" || url.username !== "" || url.password !== "") return null;
  const loopback = url.hostname === "localhost" || url.hostname === "127.0.0.1" || url.hostname === "[::1]";
  if (url.protocol === "http:" && !loopback) return null;
  return `${url.protocol}//${url.host}`;
}

export class Relay {
  constructor(
    readonly server: string,
    readonly token: string | null,
    private readonly fetchImpl: FetchLike,
  ) {}

  with(token: string): Relay {
    return new Relay(this.server, token, this.fetchImpl);
  }

  /// Registers the device that scanned a join code. The label is sealed by
  /// the caller; the relay never sees device names.
  async join(code: string, sealedLabel: string): Promise<{ device_id: string; token: string }> {
    const response = await this.send("POST", "/v1/join", json({ code, label: sealedLabel }), {
      403: "코드가 틀렸거나 만료됐어요. 데스크톱에서 QR을 다시 띄워 주세요.",
    });
    return (await response.json()) as { device_id: string; token: string };
  }

  async me(): Promise<{ id: string; devices: number }> {
    return (await (await this.send("GET", "/v1/me")).json()) as { id: string; devices: number };
  }

  async devices(): Promise<{ me: string; devices: Device[] }> {
    return (await (await this.send("GET", "/v1/devices")).json()) as { me: string; devices: Device[] };
  }

  async rename(sealedLabel: string): Promise<void> {
    await this.send("PUT", "/v1/me", json({ label: sealedLabel }));
  }

  async removeDevice(id: string): Promise<void> {
    await this.send("DELETE", `/v1/devices/${id}`);
  }

  async policy(): Promise<Policy> {
    const raw = (await (await this.send("GET", "/v1/policy")).json()) as Partial<Policy>;
    return {
      retention_days: raw.retention_days ?? 30,
      max_storage_mb: raw.max_storage_mb ?? 1024,
      max_blob_mb: raw.max_blob_mb ?? 128,
      sync_concealed: raw.sync_concealed ?? false,
      usage: raw.usage ?? null,
    };
  }

  /// The change feed after `since`, waiting up to `wait` seconds for
  /// something new. "reset" means `since` fell behind the relay's floor and
  /// the caller must start over from 0.
  async changes(since: number, wait: number, limit = MAX_CHANGES, signal?: AbortSignal): Promise<ChangePage | "reset"> {
    const query = `since=${since}&wait=${Math.min(wait, MAX_WAIT_SECS)}&limit=${limit}`;
    try {
      const response = await this.send("GET", `/v1/changes?${query}`, undefined, {}, signal);
      return (await response.json()) as ChangePage;
    } catch (e) {
      if (e instanceof RelayError && e.status === 409) return "reset";
      throw e;
    }
  }

  /// Creates or re-describes an item. "deleted" means the item is a
  /// tombstone on the relay and must not come back.
  async putItem(id: string, envelope: Uint8Array, hasBlob: boolean): Promise<{ blobPending: boolean } | "deleted"> {
    try {
      const response = await this.send(
        "PUT",
        `/v1/items/${id}`,
        json({ envelope: encodeBase64(envelope), has_blob: hasBlob }),
        { 409: "서버의 항목과 모양이 달라 올리지 못했어요." },
      );
      const body = (await response.json()) as { blob_pending?: boolean };
      return { blobPending: body.blob_pending === true };
    } catch (e) {
      if (e instanceof RelayError && e.status === 410) return "deleted";
      throw e;
    }
  }

  async uploadBlob(id: string, sealed: Uint8Array): Promise<void> {
    await this.send("PUT", `/v1/items/${id}/blob`, { body: sealed, type: "application/octet-stream" });
  }

  async blob(id: string): Promise<Uint8Array> {
    const response = await this.send("GET", `/v1/items/${id}/blob`);
    return new Uint8Array(await response.arrayBuffer());
  }

  async deleteItem(id: string): Promise<void> {
    await this.send("DELETE", `/v1/items/${id}`);
  }

  async deleteAll(): Promise<number> {
    const body = (await (await this.send("DELETE", "/v1/items")).json()) as { removed?: number };
    return body.removed ?? 0;
  }

  private async send(
    method: string,
    path: string,
    payload?: { body: Uint8Array | string; type: string },
    messages: Record<number, string> = {},
    signal?: AbortSignal,
  ): Promise<Response> {
    const headers: Record<string, string> = {};
    if (this.token) headers.Authorization = `Bearer ${this.token}`;
    if (payload) headers["Content-Type"] = payload.type;
    let response: Response;
    try {
      response = await this.fetchImpl(`${this.server}${path}`, {
        method,
        headers,
        body: payload?.body as BodyInit | undefined,
        signal,
      });
    } catch (e) {
      if (signal?.aborted) throw e;
      throw new RelayError(`서버에 연결할 수 없어요. (${describe(e)})`, null);
    }
    if (response.ok) return response;
    const status = response.status;
    const message =
      messages[status] ??
      (status === 401
        ? "이 기기의 연결이 해제됐어요. 연결을 해제한 뒤 QR로 다시 연결해 주세요."
        : status === 413
          ? "너무 커서 올릴 수 없어요."
          : status === 429
            ? "실패한 시도가 많아 잠시 막혔어요. 10분 뒤 다시 시도해 주세요."
            : `서버 오류예요. (HTTP ${status})`);
    throw new RelayError(message, status);
  }
}

function json(value: unknown): { body: string; type: string } {
  return { body: JSON.stringify(value), type: "application/json" };
}

function describe(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
