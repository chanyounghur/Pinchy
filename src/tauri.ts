// Tauri bridge with a browser fallback (plain `vite dev` in a browser) so the
// panel UI can be iterated on and screenshotted without the native shell.
import * as core from "@tauri-apps/api/core";
import * as tauriEvent from "@tauri-apps/api/event";

const isTauri = "__TAURI_INTERNALS__" in window;

type Handler = (e: { payload: unknown }) => void;
const handlers = new Map<string, Set<Handler>>();
let mockAutostart = false;
let mockRetention = { days_enabled: true, days: 30, size_enabled: true, max_mb: 1024 };
let mockUsage = { bytes: 249561088, count: 1240 };
// `?demo=share` starts the browser preview already connected.
let mockShare: { name: string; server: string; device_id: string } | null =
  !isTauri && new URLSearchParams(window.location.search).get("demo") === "share"
    ? { name: "Chanyoung’s MacBook Air", server: "https://example.iptime.org", device_id: "a1" }
    : null;
const mockPolicy = { retention_days: 30, max_storage_mb: 10240, max_blob_mb: 128, sync_concealed: false, usage: { items: 1240, bytes: 183500800 } };
const mockQr = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8" shape-rendering="crispEdges"><rect width="8" height="8" fill="#fff"/><path d="M0 0h3v3H0zM5 0h3v3H5zM0 5h3v3H0zM4 4h1v1H4zM6 5h1v1H6zM5 7h1v1H5z" fill="#000"/></svg>';
let mockDevices = [
  { id: "a1", name: "Chanyoung’s MacBook Air", created_at: Date.now() - 86_400_000, last_seen: Date.now(), me: true },
  { id: "b2", name: "회사 맥", created_at: Date.now() - 3_600_000, last_seen: Date.now() - 600_000, me: false },
];

const MOCK_ITEMS = [
  { id: 1, kind: "text", content: "", preview: "abc123!@#", source_app: "1Password", app_icon: null, app_color: "#8e8e8e", width: null, height: null, size: 9, created_at: Date.now() - 300_000, concealed: true },
  { id: 2, kind: "text", content: "", preview: "회사에서 복사한 메모", source_app: "Notes", app_icon: null, app_color: "#c64735", width: null, height: null, size: 9, created_at: Date.now() - 1_080_000, origin_device: "회사 맥" },
  { id: 3, kind: "link", content: "https://tauri.app/develop/", preview: "https://tauri.app/develop/", og_title: "Tauri — Build smaller, faster desktop apps", og_image: "/brand/pinchy-wordmark.png", source_app: "Safari", app_icon: null, app_color: "#1f8fff", width: null, height: null, size: 26, created_at: Date.now() - 1_620_000 },
  { id: 6, kind: "link", content: "https://example.com/notes", preview: "https://example.com/notes", og_title: null, og_image: null, source_app: "Chrome", app_icon: null, app_color: "#788fbc", width: null, height: null, size: 25, created_at: Date.now() - 1_800_000 },
  { id: 4, kind: "files", content: "", preview: "report.pdf\nscreenshot.png", source_app: "Finder", app_icon: null, app_color: "#3b82f6", width: null, height: null, size: 2, created_at: Date.now() - 7_200_000 },
  { id: 5, kind: "text", content: "", preview: "회의록 초안입니다. 다음 주 월요일까지 검토 부탁드려요. 특히 3번 항목 일정이 빠듯합니다.", source_app: "Notes", app_icon: null, app_color: "#f5b400", width: null, height: null, size: 31, created_at: Date.now() - 86_400_000 },
];

// A longer browser-only sample list for trying held-arrow navigation locally.
const browserItems = !isTauri && new URLSearchParams(window.location.search).get("demo") === "scroll"
  ? Array.from({ length: 8 }, (_, batch) => MOCK_ITEMS.map((item, index) => ({
      ...item,
      id: batch * MOCK_ITEMS.length + index + 1,
    }))).flat()
  : MOCK_ITEMS;

export const invoke: typeof core.invoke = isTauri
  ? core.invoke
  : (async (cmd: string, args?: Record<string, unknown>) => {
      console.log("[mock invoke]", cmd, args);
      if (cmd === "get_settings") return { shortcut: /Mac/i.test(navigator.platform) ? "Alt+Super+V" : "Super+Alt+V", retention: mockRetention, share: mockShare };
      if (cmd === "share_status") return mockShare ? { state: "connected", devices: mockDevices.length, policy: mockPolicy } : { state: "off" };
      if (cmd === "default_device_name") return "Chanyoung’s MacBook Air";
      if (cmd === "join_share") { mockShare = { name: String(args?.name ?? ""), server: String(args?.server ?? ""), device_id: "a1" }; return { state: "connected", devices: mockDevices.length, policy: mockPolicy }; }
      if (cmd === "start_invite") { setTimeout(() => handlers.get("share-invite")?.forEach((h) => h({ payload: { state: "joined", name: "새 노트북" } })), 8000); return "37-4821-1937"; }
      if (cmd === "cancel_invite") return;
      if (cmd === "start_phone_invite") return { link: "https://example.iptime.org/#join?code=0123456789abcdef0123456789abcdef&key=k", svg: mockQr, expires_in: 600 };
      if (cmd === "cancel_phone_invite") return;
      if (cmd === "share_devices") return mockDevices;
      if (cmd === "remove_share_device") { mockDevices = mockDevices.filter((d) => d.id !== args?.id); return; }
      if (cmd === "disconnect_share") { mockShare = null; return null; }
      if (cmd === "set_share_name") { const name = String(args?.name ?? "").trim(); if (mockShare) mockShare = { ...mockShare, name }; mockDevices = mockDevices.map((d) => d.me ? { ...d, name } : d); return name; }
      if (cmd === "storage_usage") return mockUsage;
      if (cmd === "set_retention") { mockRetention = args?.retention as typeof mockRetention; return { removed: 0, usage: mockUsage }; }
      if (cmd === "clear_history") { mockUsage = { bytes: 49152, count: 0 }; return; }
      if (cmd === "get_autostart") return mockAutostart;
      if (cmd === "set_autostart") { mockAutostart = Boolean(args?.enabled); return mockAutostart; }
      if (cmd === "update_status") return { phase: "idle", current_version: "0.4.2", version: null, error: null };
      if (cmd === "list_items") {
        const q = String(args?.query ?? "").toLowerCase();
        return browserItems.filter((i) => `${i.preview} ${i.og_title ?? ""}`.toLowerCase().includes(q));
      }
      return undefined;
    }) as typeof core.invoke;

export const listen: typeof tauriEvent.listen = isTauri
  ? tauriEvent.listen
  : (async (name: string, handler: Handler) => {
      if (!handlers.has(name)) handlers.set(name, new Set());
      handlers.get(name)!.add(handler);
      return () => handlers.get(name)?.delete(handler);
    }) as typeof tauriEvent.listen;

export const convertFileSrc = isTauri ? core.convertFileSrc : (p: string) => p;

if (!isTauri) {
  (window as unknown as { __pinchyMock: unknown }).__pinchyMock = {
    emit: (name: string) => handlers.get(name)?.forEach((h) => h({ payload: null })),
  };
}
