import { useCallback, useEffect, useRef, useState } from "react";
import { invoke, convertFileSrc, listen } from "./tauri";
import { createSelectionScroller } from "./selectionScroll";
import "./App.css";

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
};

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

function footerText(item: Item) {
  if (item.kind === "link") return item.og_title || item.og_image ? "미리보기 저장됨" : linkDomain(item.content);
  if (item.kind === "image") return `${item.width} × ${item.height}`;
  if (item.kind === "files") return `파일 ${item.size}개`;
  return `${item.size.toLocaleString()}자`;
}

function linkDomain(url: string) {
  try { return new URL(url.trim()).hostname; } catch { return "링크"; }
}

function LinkPreview({ item }: { item: Item }) {
  const [failedImage, setFailedImage] = useState<string | null>(null);
  const hasImage = item.og_image && failedImage !== item.og_image;
  return (
    <div className={`link-preview ${hasImage ? "with-image" : "without-image"}`}>
      {hasImage ? (
        <img className="link-cover" src={convertFileSrc(item.og_image!)} alt="" draggable={false}
          onError={() => setFailedImage(item.og_image)} />
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
      if (e.pointerType !== "mouse") return;
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
      listen("panel-shown", () => {
        stopScrollingRef.current();
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
  const paste = (item: Item | undefined) => item && invoke("paste_item", { id: item.id });
  const copy = (item: Item | undefined) => item && invoke("copy_item", { id: item.id });
  const remove = (item: Item | undefined) => item && invoke("delete_item", { id: item.id });

  const onKeyDown = (e: React.KeyboardEvent) => {
    revealSelectionRef.current = true;
    const mod = e.metaKey || e.ctrlKey;
    if (mod && /^[1-9]$/.test(e.key)) {
      e.preventDefault();
      paste(items[Number(e.key) - 1]);
      return;
    }
    switch (e.key) {
      case "Escape":
        e.preventDefault();
        close();
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
            title={item.source_app ?? undefined}
          >
            <div className="card-head" style={{ background: item.app_color ?? "#8e8e93" }}>
              <div className="card-title">
                <span className="kind">{KIND_LABEL[item.kind]}</span>
                <span className="time">{timeAgo(item.created_at)}</span>
              </div>
              <div className="app-icon">
                {item.app_icon ? (
                  <img src={convertFileSrc(item.app_icon)} alt="" draggable={false} />
                ) : (
                  <span>{(item.source_app ?? "?").slice(0, 1)}</span>
                )}
              </div>
            </div>
            <div className={`card-body ${item.kind === "link" ? "link-body" : ""}`}>
              {item.kind === "image" ? (
                <img src={convertFileSrc(item.content)} alt="" draggable={false} />
              ) : item.kind === "link" ? (
                <LinkPreview item={item} />
              ) : (
                <pre>{item.preview}</pre>
              )}
            </div>
            <div className="card-foot">
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
    </div>
  );
}
