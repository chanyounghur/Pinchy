// Tauri bridge with a browser fallback (plain `vite dev` in a browser) so the
// panel UI can be iterated on and screenshotted without the native shell.
import * as core from "@tauri-apps/api/core";
import * as tauriEvent from "@tauri-apps/api/event";

const isTauri = "__TAURI_INTERNALS__" in window;

type Handler = (e: { payload: unknown }) => void;
const handlers = new Map<string, Set<Handler>>();

const MOCK_ITEMS = [
  { id: 1, kind: "text", content: "", preview: "const panel = document.querySelector('.panel');\npanel.classList.add('open');", source_app: "Code", width: null, height: null, size: 72, created_at: Date.now() - 30_000 },
  { id: 2, kind: "link", content: "", preview: "https://tauri.app/develop/", source_app: "Safari", width: null, height: null, size: 26, created_at: Date.now() - 600_000 },
  { id: 3, kind: "files", content: "", preview: "report.pdf\nscreenshot.png", source_app: "Finder", width: null, height: null, size: 2, created_at: Date.now() - 7_200_000 },
  { id: 4, kind: "text", content: "", preview: "회의록 초안입니다. 다음 주 월요일까지 검토 부탁드려요.", source_app: "Notes", width: null, height: null, size: 31, created_at: Date.now() - 86_400_000 },
];

export const invoke: typeof core.invoke = isTauri
  ? core.invoke
  : (async (cmd: string, args?: Record<string, unknown>) => {
      console.log("[mock invoke]", cmd, args);
      if (cmd === "list_items") {
        const q = String(args?.query ?? "").toLowerCase();
        return MOCK_ITEMS.filter((i) => i.preview.toLowerCase().includes(q));
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
  (window as unknown as { __pastelMock: unknown }).__pastelMock = {
    emit: (name: string) => handlers.get(name)?.forEach((h) => h({ payload: null })),
  };
}
