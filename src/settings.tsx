import React, { useEffect, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import { invoke, listen } from "./tauri";
import "./settings.css";

const IS_MAC = navigator.platform.startsWith("Mac");

function Autostart() {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pending = useRef(false);
  useEffect(() => {
    let disposed = false;
    let refreshing = false;
    const refresh = async () => {
      if (pending.current || refreshing) return;
      refreshing = true;
      setBusy(true);
      try {
        const value = await invoke<boolean>("get_autostart");
        if (!disposed) { setEnabled(value); setError(null); }
      } catch (e) {
        if (!disposed) { setEnabled(null); setError(String(e)); }
      } finally {
        refreshing = false;
        if (!disposed) setBusy(false);
      }
    };
    void refresh();
    window.addEventListener("focus", refresh);
    return () => { disposed = true; window.removeEventListener("focus", refresh); };
  }, []);
  const toggle = async () => {
    if (pending.current || enabled === null) return;
    pending.current = true;
    setBusy(true);
    setError(null);
    try { setEnabled(await invoke<boolean>("set_autostart", { enabled: !enabled })); }
    catch (e) { setError(String(e)); }
    finally { pending.current = false; setBusy(false); }
  };
  return <section className="startup">
    <div className="startup-row">
      <label htmlFor="autostart">{IS_MAC ? "로그인 시 자동 실행" : "Windows 시작 시 자동 실행"}</label>
      <div className="startup-control">
        <span className="hint" aria-live="polite">{busy ? "확인 중…" : enabled === null ? "확인 실패" : enabled ? "ON" : "OFF"}</span>
        <input id="autostart" className="switch" type="checkbox" role="switch"
          checked={enabled ?? false} disabled={busy || enabled === null}
          onChange={() => void toggle()} aria-describedby="autostart-note" />
      </div>
    </div>
    <p className="note" id="autostart-note">로그인하면 창을 띄우지 않고 {IS_MAC ? "메뉴 막대" : "트레이"}에서 실행합니다.</p>
    {error && <p className="update-error" role="alert">자동 실행 설정을 확인하거나 변경하지 못했어요. 설정 창을 다시 열어 주세요.<br />{error}</p>}
  </section>;
}

type UpdateStatus = {
  phase: "idle" | "checking" | "downloading" | "ready" | "installing" | "latest" | "error";
  current_version: string;
  version: string | null;
  error: string | null;
};

function Updates() {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const refresh = async () => {
      const next = await invoke<UpdateStatus>("update_status");
      if (!disposed) setStatus(next);
    };
    void (async () => {
      const stop = await listen<UpdateStatus>("update-status", ({ payload }) => {
        if (!disposed) { setStatus(payload); setError(null); }
      });
      if (disposed) { stop(); return; }
      unlisten = stop;
      await refresh();
    })().catch((e) => { if (!disposed) setError(String(e)); });
    return () => { disposed = true; unlisten?.(); };
  }, []);
  const run = async (command: string) => {
    setError(null);
    try { await invoke(command); }
    catch (e) { setError(String(e)); }
  };
  const busy = !status || ["checking", "downloading", "installing"].includes(status.phase);
  const messages = {
    idle: "자동으로 새 버전을 확인합니다.",
    checking: "새 버전을 확인하고 있어요…",
    downloading: `${status?.version ?? "새 버전"} 다운로드 중…`,
    ready: `${status?.version ?? "새 버전"} 업데이트가 준비됐어요.`,
    installing: "설치 후 앱을 다시 시작합니다…",
    latest: "최신 버전입니다.",
    error: "업데이트하지 못했어요. 잠시 후 다시 확인해 주세요.",
  };
  return <section className="updates">
    <h1>앱 업데이트 <small>{status?.current_version ? `v${status.current_version}` : ""}</small></h1>
    <p role="status">{status ? messages[status.phase] : "업데이트 상태 확인 중…"}</p>
    <div className="row">
      {status?.phase === "ready"
        ? <button className="primary" onClick={() => void run("install_update")}>설치하고 재시작</button>
        : <button disabled={busy} onClick={() => void run("check_update")}>업데이트 확인</button>}
    </div>
    <p className="note">실행 후 및 6시간마다 확인해 자동으로 다운로드합니다.<br />설치는 버튼을 눌렀을 때 진행됩니다.</p>
    {(error || status?.error) && <details className="update-error"><summary>오류 자세히 보기</summary>{error || status?.error}</details>}
  </section>;
}

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
      <div className="settings-brand">
        <img src="/brand/pinchy-icon.png" alt="" draggable={false} />
        <span>Pinchy <small>설정</small></span>
      </div>
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
      <p className="note">{IS_MAC ? "수정자 키(⌘ ⌃ ⌥ ⇧)" : "Ctrl, Alt, Shift, Win 중"} 하나 이상과 일반 키 하나를 조합하세요.</p>
      <Autostart />
      <Updates />
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <SettingsPage />
  </React.StrictMode>,
);
