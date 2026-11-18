import { useCallback, useEffect, useRef, useState } from "react";
import { invoke, convertFileSrc, listen } from "./tauri";
import { createSelectionScroller } from "./selectionScroll";
import "./App.css";

type FileMeta = {
  name: string;
  size: number;
};

type DownloadProgress = {
  id: number;
  loaded: number;
  total: number;
};

type Item = {
  id: number;
  kind: "text" | "link" | "image" | "files";
  content: string;
  preview: string;
  source_app: string | null;
  app_icon: string | null;
  app_color: string | null;
  width: number | null;
  height: number | null;
  size: number;
  created_at: number;
  og_title: string | null;
  og_image: string | null;
  og_icon?: string | null;
  origin_device: string | null;
  sync_id: string;
  concealed: boolean;
  blob_pending: boolean;
  blob_size: number | null;
  files_meta: string | null;
};

const IS_MAC = /Mac/i.test(navigator.platform);

const KIND_LABEL: Record<Item["kind"], string> = {
  text: "텍스트",
  link: "링크",
  image: "이미지",
  files: "파일",
};

function timeAgo(ms: number) {
  const diff = Math.max(0, Date.now() - ms);
  const m = Math.floor(diff / 60000);
  if (m < 1) return "방금 전";
  if (m < 60) return `${m}분 전`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}시간 전`;
  return `${Math.floor(h / 24)}일 전`;
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)}KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)}MB`;
  return `${(bytes / 1024 ** 3).toFixed(2)}GB`;
}

function fileExt(name: string) {
  const idx = name.lastIndexOf(".");
  const ext = idx > 0 ? name.slice(idx + 1).trim() : "";
  return ext ? ext.slice(0, 4).toUpperCase() : "FILE";
}

function parseFilesMeta(item: Item): FileMeta[] {
  if (item.files_meta) {
    try {
      const parsed = JSON.parse(item.files_meta);
      if (Array.isArray(parsed) && parsed.length > 0) return parsed as FileMeta[];
    } catch {}
  }
  return item.preview
    .split("\n")
    .filter(Boolean)
    .map((name) => ({ name, size: 0 }));
}

function footerText(item: Item) {
  if (item.kind === "link") return linkDomain(item.content);
  if (item.kind === "image") return item.width && item.height ? `${item.width} × ${item.height}` : "이미지";
  if (item.kind === "files") return `파일 ${item.size}개`;
  return `${item.size.toLocaleString()}자`;
}

function linkDomain(url: string) {
  try { return new URL(url.trim()).hostname.replace(/^www\./, ""); } catch { return "링크"; }
}

function FilesList({ item }: { item: Item }) {
  const files = parseFilesMeta(item);
  const shown = files.slice(0, 3);
  const more = files.length - shown.length;
  return (
    <div className="files-list">
      {shown.map((f, idx) => (
        <div key={`${f.name}-${idx}`} className="file-row">
          <span className="file-ext">{fileExt(f.name)}</span>
          <div className="file-info">
            <span className="file-name">{f.name}</span>
            {f.size > 0 && <span className="file-size">{formatBytes(f.size)}</span>}
          </div>
        </div>
      ))}
      {more > 0 && <div className="file-more">+{more}개 더</div>}
    </div>
  );
}

function LinkPreview({ item }: { item: Item }) {
  const [failedImage, setFailedImage] = useState<string | null>(null);
  const [failedIcon, setFailedIcon] = useState<string | null>(null);
  const hasImage = item.og_image && failedImage !== item.og_image;
  const hasIcon = item.og_icon && failedIcon !== item.og_icon;
  return (
    <div className={`link-preview ${hasImage ? "with-image" : "without-image"}`}>
      {hasImage ? (
        <img className="link-cover" src={convertFileSrc(item.og_image!)} alt="" draggable={false}
          onError={() => setFailedImage(item.og_image)} />
      ) : hasIcon ? (
        <img className="link-placeholder link-icon" src={convertFileSrc(item.og_icon!)} alt="" draggable={false}
          onError={() => setFailedIcon(item.og_icon ?? null)} />
      ) : <div className="link-placeholder" aria-hidden="true">↗</div>}
      <div className="link-meta">
        <div className="link-domain">{linkDomain(item.content)}</div>
        <div className="link-title">{item.og_title || item.content.trim()}</div>
      </div>
    </div>
  );
}

const SearchIcon = () => (
  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round">
    <circle cx="11" cy="11" r="7" />
    <path d="M20 20l-3.5-3.5" />
  </svg>
);
const HistoryIcon = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
    <path d="M3 12a9 9 0 1 0 3-6.7" />
    <path d="M3 4v5h5" />
    <path d="M12 7v5l3 2" />
  </svg>
);
const LinesIcon = () => (
  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round">
    <path d="M4 7h16M4 12h16M4 17h10" />
  </svg>
);

export default function App() {
  const [items, setItems] = useState<Item[]>([]);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const [downloading, setDownloading] = useState<DownloadProgress | null>(null);
  const [downloadError, setDownloadError] = useState<{ id: number; message: string } | null>(null);
  // The right-click menu: which card and where the pointer was.
  const [menu, setMenu] = useState<{ item: Item; x: number; y: number } | null>(null);
  const menuOpenRef = useRef(false);
  menuOpenRef.current = menu !== null;
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const pointerRef = useRef<{ x: number; y: number } | null>(null);
  const revealSelectionRef = useRef(true);
  const stopScrollingRef = useRef(() => {});
  const selectionScrollerRef = useRef<ReturnType<typeof createSelectionScroller> | null>(null);

  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const scroller = createSelectionScroller(list);
    selectionScrollerRef.current = scroller;
    return () => {
      scroller.stop();
      selectionScrollerRef.current = null;
    };
  }, []);

  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    let frame = 0;
    let velocity = 0;
    let lastTime = 0;
    let lastPointer: { x: number; y: number } | null = null;
    const stop = () => {
      cancelAnimationFrame(frame);
      frame = 0;
      velocity = 0;
      list.scrollTo({ left: list.scrollLeft, behavior: "instant" });
    };
    const stopAll = () => {
      stop();
      selectionScrollerRef.current?.stop();
    };
    stopScrollingRef.current = stopAll;
    const tick = (time: number) => {
      const before = list.scrollLeft;
      list.scrollLeft += velocity * Math.min(time - lastTime, 32) / 1000;
      lastTime = time;
      frame = velocity && Math.abs(list.scrollLeft - before) > 0.01
        ? requestAnimationFrame(tick) : 0;
    };
    const move = (e: PointerEvent) => {
      if (e.pointerType !== "mouse" || menuOpenRef.current) return;
      if (lastPointer?.x === e.clientX && lastPointer.y === e.clientY) return;
      lastPointer = { x: e.clientX, y: e.clientY };
      selectionScrollerRef.current?.stop();
      const rect = list.getBoundingClientRect();
      const edge = Math.min(64, rect.width / 4);
      const x = e.clientX - rect.left;
      const strength = x < edge ? -(1 - x / edge)
        : x > rect.width - edge ? 1 - (rect.width - x) / edge : 0;
      if (!strength) { stop(); return; }
      revealSelectionRef.current = false;
      velocity = Math.sign(strength) * (120 + 480 * Math.min(1, Math.abs(strength)));
      if (!frame) {
        list.scrollTo({ left: list.scrollLeft, behavior: "instant" });
        lastTime = performance.now();
        frame = requestAnimationFrame(tick);
      }
    };
    const wheel = (e: WheelEvent) => {
      if (e.ctrlKey || e.metaKey) return;
      const delta = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
      if (!delta || list.scrollWidth <= list.clientWidth) return;
      e.preventDefault();
      stopAll();
      revealSelectionRef.current = false;
      const unit = e.deltaMode === WheelEvent.DOM_DELTA_LINE ? 16
        : e.deltaMode === WheelEvent.DOM_DELTA_PAGE ? list.clientWidth : 1;
      list.scrollLeft += delta * unit;
    };
    const leave = () => { lastPointer = null; stopAll(); };
    list.addEventListener("pointermove", move);
    list.addEventListener("pointerleave", leave);
    list.addEventListener("pointercancel", leave);
    // React's delegated wheel listener is passive, so use a cancellable native listener.
    list.addEventListener("wheel", wheel, { passive: false });
    window.addEventListener("blur", leave);
    window.addEventListener("keydown", stop);
    return () => {
      stopAll();
      list.removeEventListener("pointermove", move);
      list.removeEventListener("pointerleave", leave);
      list.removeEventListener("pointercancel", leave);
      list.removeEventListener("wheel", wheel);
      window.removeEventListener("blur", leave);
      window.removeEventListener("keydown", stop);
      stopScrollingRef.current = () => {};
    };
  }, []);

  const refresh = useCallback(async (q: string) => {
    const result = await invoke<Item[]>("list_items", { query: q, limit: 200 });
    setItems(result);
    setSelected((s) => Math.min(s, Math.max(0, result.length - 1)));
  }, []);

  useEffect(() => {
    refresh(query);
  }, [query, refresh]);

  useEffect(() => {
    const focusInput = () => inputRef.current?.focus();
    window.addEventListener("focus", focusInput);
    const unlisteners = [
      listen("clipboard-changed", () => refresh(query)),
      listen<DownloadProgress>("share-download-progress", ({ payload }) => {
        setDownloading(payload);
      }),
      listen("panel-shown", () => {
        stopScrollingRef.current();
        setMenu(null);
        setDownloading(null);
        setDownloadError(null);
        revealSelectionRef.current = true;
        setQuery("");
        setSelected(0);
        refresh("");
        listRef.current?.scrollTo({ left: 0 });
        focusInput();
      }),
    ];
    return () => {
      window.removeEventListener("focus", focusInput);
      unlisteners.forEach((u) => u.then((f) => f()));
    };
  }, [query, refresh]);

  // Update the destination without restarting the animation on every repeated key.
  useEffect(() => {
    const list = listRef.current;
    const el = list?.querySelector<HTMLElement>(".card.selected");
    if (!list || !el) return;
    const reveal = () => {
      if (!revealSelectionRef.current) return;
      const viewport = list.getBoundingClientRect();
      const card = el.getBoundingClientRect();
      const margin = Math.min(24, Math.max(0, (list.clientWidth - card.width) / 2));
      let target = list.scrollLeft;
      if (card.left < viewport.left + margin) target += card.left - viewport.left - margin;
      else if (card.right > viewport.right - margin) target += card.right - viewport.right + margin;
      target = Math.max(0, Math.min(target, list.scrollWidth - list.clientWidth));
      selectionScrollerRef.current?.to(target);
    };
    let frame = 0;
    const scheduleReveal = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(reveal);
    };
    scheduleReveal();
    const observer = new ResizeObserver(scheduleReveal);
    observer.observe(list);
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
    };
  }, [selected, items]);

  const close = () => invoke("hide_panel");
  const cancelDownload = () => {
    void invoke("cancel_download");
    setDownloading(null);
    setDownloadError(null);
  };
  const runTransfer = async (cmd: "paste_item" | "copy_item", item: Item | undefined) => {
    if (!item) return;
    setDownloadError(null);
    if (item.blob_pending) {
      setDownloading({ id: item.id, loaded: 0, total: item.blob_size ?? item.size ?? 0 });
    }
    try {
      await invoke(cmd, { id: item.id });
      setDownloading((cur) => (cur?.id === item.id ? null : cur));
    } catch (e) {
      setDownloading((cur) => (cur?.id === item.id ? null : cur));
      const msg = String(e);
      if (!msg.includes("DOWNLOAD_CANCELLED")) {
        setDownloadError({ id: item.id, message: msg });
      }
    }
  };
  const paste = (item: Item | undefined) => void runTransfer("paste_item", item);
  const copy = (item: Item | undefined) => void runTransfer("copy_item", item);
  const remove = (item: Item | undefined) => item && invoke("delete_item", { id: item.id });

  // Any click, scroll or blur outside the menu dismisses it.
  useEffect(() => {
    if (!menu) return;
    const dismiss = () => setMenu(null);
    const list = listRef.current;
    window.addEventListener("pointerdown", dismiss);
    window.addEventListener("blur", dismiss);
    list?.addEventListener("scroll", dismiss);
    return () => {
      window.removeEventListener("pointerdown", dismiss);
      window.removeEventListener("blur", dismiss);
      list?.removeEventListener("scroll", dismiss);
    };
  }, [menu]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    revealSelectionRef.current = true;
    const mod = e.metaKey || e.ctrlKey;
    if (mod && /^[1-9]$/.test(e.key)) {
      e.preventDefault();
      paste(items[Number(e.key) - 1]);
      return;
    }
    if (menu) {
      setMenu(null);
      // Escape closes only the menu, not the panel.
      if (e.key === "Escape") {
        e.preventDefault();
        return;
      }
    }
    switch (e.key) {
      case "Escape":
        e.preventDefault();
        if (downloading) cancelDownload();
        else if (downloadError) setDownloadError(null);
        else close();
        break;
      case "ArrowRight":
        e.preventDefault();
        setSelected((s) => Math.min(s + 1, Math.max(0, items.length - 1)));
        break;
      case "ArrowLeft":
        e.preventDefault();
        setSelected((s) => Math.max(s - 1, 0));
        break;
      case "Enter":
        e.preventDefault();
        if (mod) copy(items[selected]);
        else paste(items[selected]);
        break;
      case "Backspace":
        if (mod) {
          e.preventDefault();
          remove(items[selected]);
        }
        break;
    }
  };

  return (
    <div className="panel" onKeyDown={onKeyDown}>
      <div className="toolbar">
        <div className="brand" aria-label="Pinchy">
          <img src="/brand/pinchy-icon.png" alt="" draggable={false} />
          <span>Pinchy</span>
        </div>
        <label className={`search ${query ? "active" : ""}`}>
          <SearchIcon />
          <input
            ref={inputRef}
            autoFocus
            placeholder="검색"
            value={query}
            onChange={(e) => {
              stopScrollingRef.current();
              revealSelectionRef.current = true;
              setQuery(e.target.value);
              setSelected(0);
            }}
          />
        </label>
        <div className="tab selected">
          <HistoryIcon />
          클립보드 히스토리
        </div>
        <div className="toolbar-right">
          <span className="count">{items.length}</span>
        </div>
      </div>

      <div className="list" ref={listRef}>
        {items.length === 0 && <div className="empty">아직 복사한 내용이 없어요</div>}
        {items.map((item, i) => (
          <div
            key={item.id}
            className={`card kind-${item.kind} ${i === selected ? "selected" : ""}`}
            onPointerMove={(e) => {
              const previous = pointerRef.current;
              pointerRef.current = { x: e.clientX, y: e.clientY };
              // A card moving under a stationary pointer must not steal selection.
              if (!previous || previous.x !== e.clientX || previous.y !== e.clientY) {
                revealSelectionRef.current = false;
                setSelected(i);
              }
            }}
            onClick={() => paste(item)}
            onContextMenu={(e) => {
              e.preventDefault();
              stopScrollingRef.current();
              setSelected(i);
              setMenu({ item, x: e.clientX, y: e.clientY });
            }}
            title={item.source_app ?? undefined}
          >
            <div className="card-head" style={{ background: item.app_color ?? "#8e8e93" }}>
              <div className="card-title">
                <span className="kind">{KIND_LABEL[item.kind]}</span>
                <span
                  className="time"
                  title={item.origin_device ? `${timeAgo(item.created_at)} · ${item.origin_device}` : undefined}
                >
                  {timeAgo(item.created_at)}{item.origin_device ? ` · ${item.origin_device}` : ""}
                </span>
              </div>
              <div className="app-icon">
                {item.app_icon ? (
                  <img src={convertFileSrc(item.app_icon)} alt="" draggable={false} />
                ) : (
                  <span>{(item.source_app ?? "?").trim().slice(0, 1).toUpperCase() || "?"}</span>
                )}
              </div>
            </div>
            <div className={`card-body ${item.kind === "link" ? "link-body" : ""}`}>
              {item.kind === "image" ? (
                item.content ? (
                  <img src={convertFileSrc(item.content)} alt="" draggable={false} />
                ) : (
                  <div className="image-placeholder">
                    <span>{item.width && item.height ? `${item.width} × ${item.height}` : "이미지"}</span>
                    {item.blob_size ? <small>{formatBytes(item.blob_size)}</small> : null}
                  </div>
                )
              ) : item.kind === "link" ? (
                <LinkPreview item={item} />
              ) : item.kind === "files" ? (
                <FilesList item={item} />
              ) : (
                <pre>{item.preview}</pre>
              )}
              {downloading?.id === item.id && (() => {
                const total = downloading.total || item.blob_size || 0;
                const pct = total > 0 ? Math.min(100, Math.round((downloading.loaded / total) * 100)) : 0;
                return (
                  <div className="dl-overlay" onClick={(e) => e.stopPropagation()}>
                    <div className="dl-title">
                      {item.origin_device ? `${item.origin_device}에서 받는 중…` : "받는 중…"}
                    </div>
                    <div className="dl-size">
                      {total > 0
                        ? `${formatBytes(downloading.loaded)} / ${formatBytes(total)}`
                        : formatBytes(downloading.loaded)}
                    </div>
                    <div className="dl-bar">
                      <div className="dl-bar-fill" style={{ width: `${pct}%` }} />
                    </div>
                    <button
                      type="button"
                      className="dl-cancel"
                      onClick={(e) => {
                        e.stopPropagation();
                        cancelDownload();
                      }}
                    >
                      Esc 취소
                    </button>
                  </div>
                );
              })()}
              {downloadError?.id === item.id && downloading?.id !== item.id && (
                <div className="dl-overlay" onClick={(e) => e.stopPropagation()}>
                  <div className="dl-title">받지 못했어요</div>
                  <div className="dl-error">{downloadError.message}</div>
                  <button
                    type="button"
                    className="dl-cancel"
                    onClick={(e) => {
                      e.stopPropagation();
                      setDownloadError(null);
                    }}
                  >
                    닫기
                  </button>
                </div>
              )}
            </div>
            <div className="card-foot">
              {item.blob_pending ? (
                <span className="origin" title="붙여넣을 때 서버에서 받아요">↓ {formatBytes(item.blob_size ?? item.size)}</span>
              ) : item.concealed ? (
                <span className="concealed" title="비밀번호 관리자에서 복사한 내용이라 이 기기에만 남아요">숨김</span>
              ) : null}
              <span>{footerText(item)}</span>
              {i < 9 && (
                <span className="badge">
                  <LinesIcon />
                  {i + 1}
                </span>
              )}
            </div>
          </div>
        ))}
      </div>

      {menu && (
        <div
          className="context-menu"
          role="menu"
          style={{ left: Math.min(menu.x, window.innerWidth - 168), top: Math.min(menu.y, window.innerHeight - 48) }}
          onPointerDown={(e) => e.stopPropagation()}
          onContextMenu={(e) => e.preventDefault()}
        >
          <button
            type="button"
            role="menuitem"
            className="danger"
            onClick={() => {
              remove(menu.item);
              setMenu(null);
            }}
          >
            삭제
            <span className="shortcut">{IS_MAC ? "⌘⌫" : "Ctrl+⌫"}</span>
          </button>
        </div>
      )}
    </div>
  );
}
