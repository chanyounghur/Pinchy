import React, { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "./tauri";
import "./settings.css";

const IS_MAC = navigator.platform.startsWith("Mac");

/** Maps a keydown to the plugin's "Mod+Mod+Key" string, or null if incomplete. */
function comboFromEvent(e: KeyboardEvent): string | null {
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Super");
  if (mods.length === 0) return null;
  let key: string | null = null;
  const code = e.code;
  if (/^Key[A-Z]$/.test(code)) key = code.slice(3);
  else if (/^Digit[0-9]$/.test(code)) key = code.slice(5);
  else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) key = code;
  else if (["Space", "Enter", "Tab", "Backspace", "Escape", "Home", "End", "PageUp", "PageDown", "Insert", "Delete"].includes(code)) key = code;
  else if (/^Arrow(Up|Down|Left|Right)$/.test(code)) key = code;
  else if (["Backquote", "Minus", "Equal", "BracketLeft", "BracketRight", "Backslash", "Semicolon", "Quote", "Comma", "Period", "Slash"].includes(code)) key = code;
  if (!key) return null;
  return [...mods, key].join("+");
}

function pretty(combo: string) {
  const sym: Record<string, string> = IS_MAC
    ? { Ctrl: "⌃", Alt: "⌥", Shift: "⇧", Super: "⌘" }
    : { Ctrl: "Ctrl", Alt: "Alt", Shift: "Shift", Super: "Win" };
  return combo
    .split("+")
    .map((p) => sym[p] ?? p)
    .join(IS_MAC ? "" : " + ");
}

function SettingsPage() {
  const [saved, setSaved] = useState("");
  const [draft, setDraft] = useState<string | null>(null);
  const [capturing, setCapturing] = useState(false);
  const [message, setMessage] = useState<{ kind: "ok" | "err"; text: string } | null>(null);

  useEffect(() => {
    invoke<{ shortcut: string }>("get_settings").then((s) => setSaved(s.shortcut));
  }, []);

  useEffect(() => {
    invoke("set_shortcut_capturing", { capturing });
  }, [capturing]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    e.preventDefault();
    const combo = comboFromEvent(e.nativeEvent);
    if (combo) setDraft(combo);
  };

  const save = async () => {
    if (!draft) return;
    try {
      await invoke("set_shortcut", { shortcut: draft });
      setSaved(draft);
      setDraft(null);
      setMessage({ kind: "ok", text: "저장했어요" });
    } catch (e) {
      setMessage({ kind: "err", text: String(e) });
    }
  };

  const shown = draft ?? saved;
  return (
    <div className="page">
      <h1>패널 열기 단축키</h1>
      <div
        className={`capture ${capturing ? "capturing" : ""}`}
        tabIndex={0}
        onFocus={() => setCapturing(true)}
        onBlur={() => setCapturing(false)}
        onKeyDown={onKeyDown}
      >
        {shown ? <span className="combo">{pretty(shown)}</span> : <span className="hint">…</span>}
        <span className="hint">{capturing ? "원하는 조합을 누르세요" : "클릭한 뒤 키를 누르세요"}</span>
      </div>
      <div className="row">
        <button className="primary" disabled={!draft || draft === saved} onClick={save}>
          저장
        </button>
        <button disabled={!draft} onClick={() => setDraft(null)}>
          되돌리기
        </button>
        <span className={`msg ${message?.kind ?? ""}`}>{message?.text}</span>
      </div>
      <p className="note">수정자 키(⌘ ⌃ ⌥ ⇧) 하나 이상과 일반 키 하나를 조합하세요.</p>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <SettingsPage />
  </React.StrictMode>,
);
