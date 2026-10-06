import { useCallback, useEffect, useRef, useState } from "react";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type Item = {
  id: number;
  kind: "text" | "link" | "image" | "files";
  content: string;
  preview: string;
  source_app: string | null;
  width: number | null;
  height: number | null;
  size: number;
  created_at: number;
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
  if (m < 1) return "방금";
  if (m < 60) return `${m}분 전`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}시간 전`;
  return `${Math.floor(h / 24)}일 전`;
}

export default function App() {
  const [items, setItems] = useState<Item[]>([]);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const refresh = useCallback(async (q: string) => {
    const result = await invoke<Item[]>("list_items", { query: q, limit: 200 });
    setItems(result);
    setSelected((s) => Math.min(s, Math.max(0, result.length - 1)));
  }, []);

  useEffect(() => {
    refresh(query);
  }, [query, refresh]);

  useEffect(() => {
    const unlisteners = [
      listen("clipboard-changed", () => refresh(query)),
      listen("panel-shown", () => {
        setQuery("");
        setSelected(0);
        refresh("");
        inputRef.current?.focus();
      }),
    ];
    return () => {
      unlisteners.forEach((u) => u.then((f) => f()));
    };
  }, [query, refresh]);

  useEffect(() => {
    const el = listRef.current?.children[selected] as HTMLElement | undefined;
    el?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [selected, items]);

  const paste = (item: Item | undefined) => item && invoke("paste_item", { id: item.id });
  const copy = (item: Item | undefined) => item && invoke("copy_item", { id: item.id });
  const remove = async (item: Item | undefined) => {
    if (!item) return;
    await invoke("delete_item", { id: item.id });
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    const mod = e.metaKey || e.ctrlKey;
    switch (e.key) {
      case "Escape":
        e.preventDefault();
        invoke("hide_panel");
        break;
      case "ArrowRight":
        e.preventDefault();
        setSelected((s) => Math.min(s + 1, items.length - 1));
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
        <input
          ref={inputRef}
          autoFocus
          className="search"
          placeholder="검색…"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setSelected(0);
          }}
        />
        <span className="hint">
          ↩ 붙여넣기 · ⌘↩ 복사만 · ⌘⌫ 삭제 · esc 닫기
        </span>
        <span className="count">{items.length}개</span>
      </div>
      <div className="list" ref={listRef}>
        {items.length === 0 && <div className="empty">아직 복사한 내용이 없어요</div>}
        {items.map((item, i) => (
          <div
            key={item.id}
            className={`card kind-${item.kind} ${i === selected ? "selected" : ""}`}
            onMouseEnter={() => setSelected(i)}
            onClick={() => paste(item)}
          >
            <div className="card-head">
              <span className="kind">{KIND_LABEL[item.kind]}</span>
              <span className="meta">
                {item.source_app ?? ""} · {timeAgo(item.created_at)}
              </span>
            </div>
            <div className="card-body">
              {item.kind === "image" ? (
                <img src={convertFileSrc(item.content)} alt="" draggable={false} />
              ) : (
                <pre>{item.preview}</pre>
              )}
            </div>
            <div className="card-foot">
              {item.kind === "image"
                ? `${item.width}×${item.height}`
                : item.kind === "files"
                  ? `${item.size}개 파일`
                  : `${item.size.toLocaleString()}자`}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
