// Tauri bridge with a browser fallback (plain `vite dev` in a browser) so the
// panel UI can be iterated on and screenshotted without the native shell.
import * as core from "@tauri-apps/api/core";
import * as tauriEvent from "@tauri-apps/api/event";

const isTauri = "__TAURI_INTERNALS__" in window;

type Handler = (e: { payload: unknown }) => void;
const handlers = new Map<string, Set<Handler>>();
let mockAutostart = false;

const MOCK_ITEMS = [
  { id: 1, kind: "text", content: "", preview: "abc123!@#", source_app: "Claude", app_icon: null, app_color: "#8e8e8e", width: null, height: null, size: 9, created_at: Date.now() - 300_000 },
  { id: 2, kind: "text", content: "", preview: "fuk", source_app: "KakaoTalk", app_icon: null, app_color: "#f7d400", width: null, height: null, size: 3, created_at: Date.now() - 1_080_000 },
  { id: 3, kind: "link", content: "https://tauri.app/develop/", preview: "https://tauri.app/develop/", og_title: "Tauri — Build smaller, faster desktop apps", og_image: "/brand/pinchy-wordmark.png", source_app: "Safari", app_icon: null, app_color: "#1f8fff", width: null, height: null, size: 26, created_at: Date.now() - 1_620_000 },
  { id: 6, kind: "link", content: "https://example.com/notes", preview: "https://example.com/notes", og_title: null, og_image: null, source_app: "Chrome", app_icon: null, app_color: "#788fbc", width: null, height: null, size: 25, created_at: Date.now() - 1_800_000 },
  { id: 4, kind: "files", content: "", preview: "report.pdf\nscreenshot.png", source_app: "Finder", app_icon: null, app_color: "#3b82f6", width: null, height: null, size: 2, created_at: Date.now() - 7_200_000 },
  { id: 5, kind: "text", content: "", preview: "회의록 초안입니다. 다음 주 월요일까지 검토 부탁드려요. 특히 3번 항목 일정이 빠듯합니다.", source_app: "Notes", app_icon: null, app_color: "#f5b400", width: null, height: null, size: 31, created_at: Date.now() - 86_400_000 },
];

export const invoke: typeof core.invoke = isTauri
  ? core.invoke
  : (async (cmd: string, args?: Record<string, unknown>) => {
      console.log("[mock invoke]", cmd, args);
      if (cmd === "get_settings") return { shortcut: "Shift+Super+V" };
      if (cmd === "get_autostart") return mockAutostart;
      if (cmd === "set_autostart") { mockAutostart = Boolean(args?.enabled); return mockAutostart; }
      if (cmd === "update_status") return { phase: "idle", current_version: "0.3.2", version: null, error: null };
      if (cmd === "list_items") {
        const q = String(args?.query ?? "").toLowerCase();
        return MOCK_ITEMS.filter((i) => `${i.preview} ${i.og_title ?? ""}`.toLowerCase().includes(q));
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
