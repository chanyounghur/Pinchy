import { useEffect, useState, type ChangeEvent } from "react";
import { invoke, listen } from "./tauri";

type ShareConfig = { name: string; server: string; token: string; key: string };
type Status =
  | { state: "off" }
  | { state: "connecting" }
  | { state: "connected"; name: string; peers: string[] }
  | { state: "error"; message: string };

const IS_MAC = navigator.platform.startsWith("Mac");

function statusText(status: Status | null) {
  if (!status || status.state === "connecting") return "연결 확인 중…";
  if (status.state === "off") return "꺼짐";
  if (status.state === "error") return "연결 문제";
  return `연결됨 · ${status.name}`;
}

export default function ShareSettings() {
  const [saved, setSaved] = useState<ShareConfig | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [status, setStatus] = useState<Status | null>(null);
  const [form, setForm] = useState<ShareConfig>({ name: "", server: "", token: "", key: "" });
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState("");

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      const stop = await listen<Status>("share-status", ({ payload }) => { if (!disposed) setStatus(payload); });
      if (disposed) { stop(); return; }
      unlisten = stop;
      const [settings, current, computerName] = await Promise.all([
        invoke<{ share: ShareConfig | null }>("get_settings"),
        invoke<Status>("share_status"),
        invoke<string>("default_device_name"),
      ]);
      if (disposed) return;
      setSaved(settings.share);
      if (settings.share) { setForm(settings.share); setName(settings.share.name); }
      else setForm((f) => ({ ...f, name: f.name || computerName }));
      setStatus(current);
      setLoaded(true);
    })().catch((e) => { if (!disposed) setError(String(e)); });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  const field = (name: keyof ShareConfig) => ({
    value: form[name],
    onChange: (e: ChangeEvent<HTMLInputElement>) => {
      setForm({ ...form, [name]: e.target.value });
      setError(null);
      setMessage("");
    },
  });

  const run = async (action: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setMessage("");
    try { await action(); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };

  const connect = () => run(async () => {
    const next = await invoke<Status>("connect_share", { config: form });
    const settings = await invoke<{ share: ShareConfig | null }>("get_settings");
    setSaved(settings.share);
    if (settings.share) { setForm(settings.share); setName(settings.share.name); }
    setStatus(next);
    setMessage("연결했어요.");
  });
  const disconnect = () => run(async () => {
    await invoke("disconnect_share");
    setSaved(null);
    setStatus({ state: "off" });
    setMessage("연결을 해제했어요. 다시 연결하려면 아래 정보를 확인하고 연결을 누르세요.");
  });
  const newKey = () => run(async () => {
    setForm({ ...form, key: await invoke<string>("generate_share_key") });
    setMessage("새 공유 키를 만들었어요. 다른 기기에는 이 키를 입력하세요.");
  });
  const rename = () => run(async () => {
    const next = await invoke<string>("set_share_name", { name });
    setName(next);
    setSaved((s) => s && { ...s, name: next });
    setMessage("기기 이름을 바꿨어요. 이제부터 보내는 항목에 이 이름이 표시돼요.");
  });
  const copyKey = (key: string) => run(async () => {
    await invoke("copy_secret", { text: key });
    setMessage("공유 키를 복사했어요. 히스토리에는 남지 않아요.");
  });

  const peers = status?.state === "connected" ? status.peers : [];
  const complete = form.server.trim() && form.token.trim() && form.key.trim();

  return <section className="share" aria-labelledby="share-heading">
    <h1 id="share-heading">기기 간 공유 <small className={`share-state ${status?.state ?? ""}`}>{statusText(status)}</small></h1>
    {saved ? <>
      <label className="share-name">이 기기 이름
        <span className="share-key-row">
          <input value={name} maxLength={64} autoComplete="off" spellCheck={false} disabled={busy}
            onChange={(e) => { setName(e.target.value); setError(null); setMessage(""); }} />
          <button disabled={busy || !name.trim() || name.trim() === saved.name}
            onClick={() => void rename()}>저장</button>
        </span>
      </label>
      <div className="share-summary">
        <div><span>서버</span><strong>{saved.server.replace(/^https?:\/\//, "")}</strong></div>
        <div><span>받는 기기</span><strong>{peers.length ? peers.join(", ") : status?.state === "connected" ? "아직 없음" : "—"}</strong></div>
      </div>
      {status?.state === "error" && <p className="update-error" role="alert">{status.message}</p>}
      <div className="row">
        <button disabled={busy} onClick={() => void copyKey(saved.key)}>공유 키 복사</button>
        <button className="danger" disabled={busy} onClick={() => void disconnect()}>연결 해제</button>
      </div>
      <p className="note">다른 기기를 연결할 때 이 공유 키를 입력하세요. 패널에서 항목을 고르고 {IS_MAC ? "⇧↩" : "Shift+Enter"}를 누르면 연결된 다른 기기로 보냅니다.</p>
    </> : <fieldset className="share-fields" disabled={busy || !loaded}>
      <label>이 기기 이름<input maxLength={64} placeholder="다른 기기에 표시될 이름" autoComplete="off" spellCheck={false} {...field("name")} /></label>
      <label>서버 주소<input type="url" placeholder="https://example.iptime.org" autoComplete="off" spellCheck={false} {...field("server")} /></label>
      <label>기기 토큰<input type="password" placeholder="pr1_…" autoComplete="off" spellCheck={false} {...field("token")} /></label>
      <label>공유 키
        <span className="share-key-row">
          <input type="password" placeholder="첫 기기는 새 키 만들기" autoComplete="off" spellCheck={false} {...field("key")} />
          <button type="button" onClick={() => void newKey()}>새 키 만들기</button>
        </span>
      </label>
      <div className="row">
        <button className="primary" disabled={!complete} onClick={() => void connect()}>{busy ? "확인 중…" : "연결"}</button>
        {form.key.trim() && <button type="button" onClick={() => void copyKey(form.key.trim())}>공유 키 복사</button>}
      </div>
      <p className="note">이 기기 이름은 보내는 항목과 함께 암호화되어 받는 기기에 표시됩니다. 기기 토큰은 서버에서 <code>pinchy-relay add-device 이름</code>으로 기기마다 만듭니다. 공유 키는 첫 기기에서 새로 만들고, 다른 기기에는 같은 키를 입력하세요. 내용은 이 키로 암호화돼 서버는 읽을 수 없습니다.</p>
    </fieldset>}
    {message && <p className="storage-message" role="status">{message}</p>}
    {error && <p className="update-error" role="alert">{error}</p>}
  </section>;
}
