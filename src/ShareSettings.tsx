import { useCallback, useEffect, useState, type ChangeEvent } from "react";
import { invoke, listen } from "./tauri";

type ShareConfig = { name: string; server: string; device_id: string };
export type Policy = {
  retention_days: number;
  max_storage_mb: number;
  max_blob_mb: number;
  sync_concealed: boolean;
  usage: { items: number; bytes: number } | null;
};
export type Status =
  | { state: "off" }
  | { state: "connecting" }
  | { state: "connected"; devices: number; policy: Policy }
  | { state: "error"; message: string };
type Device = { id: string; name: string; created_at: number; last_seen: number | null; me: boolean };
type InviteEvent =
  | { state: "joined"; name: string }
  | { state: "failed"; message: string }
  | { state: "expired" };
type PhoneInvite = { link: string; svg: string; expires_in: number };

const INVITE_SECONDS = 600;

function statusText(status: Status | null) {
  if (!status || status.state === "connecting") return "연결 확인 중…";
  if (status.state === "off") return "꺼짐";
  if (status.state === "error") return "연결 문제";
  return `연결됨 · 기기 ${status.devices}대`;
}

function lastSeen(ms: number | null) {
  if (!ms) return "접속 기록 없음";
  const m = Math.floor(Math.max(0, Date.now() - ms) / 60000);
  if (m < 1) return "방금 접속";
  if (m < 60) return `${m}분 전 접속`;
  if (m < 1440) return `${Math.floor(m / 60)}시간 전 접속`;
  return `${Math.floor(m / 1440)}일 전 접속`;
}

function countdown(left: number) {
  return `${Math.floor(left / 60)}분 ${String(left % 60).padStart(2, "0")}초`;
}

function Invite({ onClose, onJoined }: { onClose: () => void; onJoined: () => void }) {
  const [code, setCode] = useState<string | null>(null);
  const [left, setLeft] = useState(INVITE_SECONDS);
  const [result, setResult] = useState<{ kind: "ok" | "err"; text: string } | null>(null);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      const stop = await listen<InviteEvent>("share-invite", ({ payload }) => {
        if (disposed) return;
        if (payload.state === "joined") { setResult({ kind: "ok", text: `${payload.name} 기기를 연결했어요.` }); onJoined(); }
        else if (payload.state === "failed") setResult({ kind: "err", text: payload.message });
        else setResult({ kind: "err", text: "코드가 만료됐어요. 필요하면 새 코드를 만들어 주세요." });
      });
      if (disposed) { stop(); return; }
      unlisten = stop;
      const next = await invoke<string>("start_invite");
      if (!disposed) setCode(next);
    })().catch((e) => { if (!disposed) setResult({ kind: "err", text: String(e) }); });
    return () => {
      disposed = true;
      unlisten?.();
      void invoke("cancel_invite");
    };
  }, [onJoined]);

  useEffect(() => {
    if (!code || result) return;
    const started = Date.now();
    const timer = setInterval(() => setLeft(Math.max(0, INVITE_SECONDS - Math.floor((Date.now() - started) / 1000))), 1000);
    return () => clearInterval(timer);
  }, [code, result]);

  return <div className="share-invite" role="group" aria-label="컴퓨터 추가">
    {result ? <p className={result.kind === "ok" ? "storage-message" : "update-error"} role="status">{result.text}</p> : <>
      <p>새 컴퓨터의 Pinchy 설정에서 서버 주소와 아래 코드를 입력하세요.</p>
      <div className="share-code" aria-live="polite">{code ?? "코드 만드는 중…"}</div>
      <p className="hint">{code ? `${countdown(left)} 안에 입력하세요. 한 번만 쓸 수 있어요.` : ""}</p>
    </>}
    <div className="row"><button onClick={onClose}>{result ? "닫기" : "취소"}</button></div>
  </div>;
}

function PhoneInviteView({ onClose }: { onClose: () => void }) {
  const [invite, setInvite] = useState<PhoneInvite | null>(null);
  const [left, setLeft] = useState(INVITE_SECONDS);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    void invoke<PhoneInvite>("start_phone_invite")
      .then((next) => { if (!disposed) { setInvite(next); setLeft(next.expires_in); } })
      .catch((e) => { if (!disposed) setError(String(e)); });
    return () => { disposed = true; void invoke("cancel_phone_invite"); };
  }, []);

  useEffect(() => {
    if (!invite) return;
    const started = Date.now();
    const timer = setInterval(() => setLeft(Math.max(0, invite.expires_in - Math.floor((Date.now() - started) / 1000))), 1000);
    return () => clearInterval(timer);
  }, [invite]);

  const expired = invite !== null && left === 0;
  return <div className="share-invite" role="group" aria-label="휴대폰 추가">
    {error ? <p className="update-error" role="alert">{error}</p> : <>
      <p>휴대폰 카메라로 아래 QR 코드를 찍으세요. 서버 주소, 일회용 연결 코드, 공유 키가 들어 있으니 다른 사람에게 보이지 않게 하세요.</p>
      <div className={`share-qr ${expired ? "expired" : ""}`} aria-label="연결 QR 코드"
        dangerouslySetInnerHTML={{ __html: invite?.svg ?? "" }} />
      <p className="hint">{!invite ? "QR 코드 만드는 중…" : expired ? "코드가 만료됐어요. 닫고 다시 열어 주세요." : `${countdown(left)} 안에 찍으세요. 한 번만 쓸 수 있어요.`}</p>
    </>}
    <div className="row"><button onClick={onClose}>{expired || error ? "닫기" : "취소"}</button></div>
  </div>;
}

export default function ShareSettings() {
  const [saved, setSaved] = useState<ShareConfig | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [status, setStatus] = useState<Status | null>(null);
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [form, setForm] = useState({ name: "", server: "", code: "" });
  const [name, setName] = useState("");
  const [inviting, setInviting] = useState<"computer" | "phone" | null>(null);
  const [confirmLeave, setConfirmLeave] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState("");

  const connected = Boolean(saved?.device_id);

  const loadDevices = useCallback(async () => {
    try { setDevices(await invoke<Device[]>("share_devices")); }
    catch (e) { setError(String(e)); }
  }, []);

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
      const share = settings.share?.device_id ? settings.share : null;
      setSaved(share);
      setName(share?.name ?? "");
      setForm((f) => ({ ...f, name: share?.name || computerName, server: settings.share?.server ?? "" }));
      setStatus(current);
      setLoaded(true);
    })().catch((e) => { if (!disposed) setError(String(e)); });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  useEffect(() => { if (connected) void loadDevices(); }, [connected, loadDevices]);

  const run = async (action: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setMessage("");
    try { await action(); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };

  const join = () => run(async () => {
    setStatus(await invoke<Status>("join_share", { name: form.name, server: form.server, code: form.code }));
    const settings = await invoke<{ share: ShareConfig | null }>("get_settings");
    setSaved(settings.share);
    setName(settings.share?.name ?? "");
    setForm((f) => ({ ...f, code: "" }));
    setMessage("연결했어요. 다른 기기의 히스토리를 받아오고 이 기기의 히스토리를 올리는 중이에요.");
  });
  const rename = () => run(async () => {
    const next = await invoke<string>("set_share_name", { name });
    setName(next);
    setSaved((s) => s && { ...s, name: next });
    await loadDevices();
    setMessage("기기 이름을 바꿨어요.");
  });
  const remove = (device: Device) => run(async () => {
    await invoke("remove_share_device", { id: device.id });
    await loadDevices();
    setMessage(`${device.name} 기기의 연결을 해제했어요.`);
  });
  const leave = () => run(async () => {
    const warning = await invoke<string | null>("disconnect_share");
    setSaved(null);
    setDevices(null);
    setConfirmLeave(false);
    setInviting(null);
    setStatus({ state: "off" });
    if (warning) setError(warning);
    else setMessage("이 기기의 연결을 해제했어요. 지금까지의 히스토리는 이 기기에 그대로 남아요.");
  });
  const onJoined = useCallback(() => { void loadDevices(); }, [loadDevices]);

  const field = (key: keyof typeof form) => ({
    value: form[key],
    onChange: (e: ChangeEvent<HTMLInputElement>) => {
      setForm({ ...form, [key]: e.target.value });
      setError(null);
      setMessage("");
    },
  });

  return <section className="share" aria-labelledby="share-heading">
    <h1 id="share-heading">기기 간 동기화 <small className={`share-state ${status?.state ?? ""}`}>{statusText(status)}</small></h1>
    {connected && saved ? <>
      <label className="share-name">이 기기 이름
        <span className="share-key-row">
          <input value={name} maxLength={64} autoComplete="off" spellCheck={false} disabled={busy}
            onChange={(e) => { setName(e.target.value); setError(null); setMessage(""); }} />
          <button disabled={busy || !name.trim() || name.trim() === saved.name} onClick={() => void rename()}>저장</button>
        </span>
      </label>
      <div className="share-summary">
        <div><span>서버</span><strong>{saved.server.replace(/^https?:\/\//, "")}</strong></div>
      </div>
      <p className="note">연결된 모든 기기가 같은 히스토리를 가져요. 어느 기기에서 복사하든 모두에 쌓이고, 지우면 모두에서 지워져요. 파일은 합쳐서 50MB까지만 올라가고, 폴더와 더 큰 파일은 이 기기에만 남아요.</p>
      {status?.state === "error" && <p className="update-error" role="alert">{status.message}</p>}
      <ul className="share-devices" aria-label="연결된 기기">
        {(devices ?? []).map((d) => <li key={d.id}>
          <div><strong>{d.name}</strong>{d.me && <span className="share-me">이 기기</span>}<small>{lastSeen(d.last_seen)}</small></div>
          {!d.me && <button className="danger" disabled={busy} onClick={() => void remove(d)}>해제</button>}
        </li>)}
        {devices === null && <li className="hint">기기 목록을 불러오는 중…</li>}
      </ul>
      {inviting === "computer" ? <Invite onClose={() => setInviting(null)} onJoined={onJoined} />
        : inviting === "phone" ? <PhoneInviteView onClose={() => { setInviting(null); void loadDevices(); }} />
        : <div className="row">
          <button className="primary" disabled={busy} onClick={() => { setMessage(""); setError(null); setInviting("computer"); }}>컴퓨터 추가</button>
          <button disabled={busy} onClick={() => { setMessage(""); setError(null); setInviting("phone"); }}>휴대폰 추가</button>
          <button className="danger" disabled={busy} onClick={() => setConfirmLeave(true)}>이 기기 연결 해제</button>
        </div>}
      {confirmLeave && <div className="storage-confirm" role="group" aria-label="연결 해제 확인">
        <p>이 기기의 연결을 해제할까요? 히스토리는 이 기기에 남지만 더는 다른 기기와 맞춰지지 않아요. 다시 연결하려면 다른 기기에서 새 코드를 받아야 하고, 마지막 기기라면 서버 로그에 새 첫 연결 코드가 나와요.</p>
        <div className="row"><button className="primary" disabled={busy} onClick={() => void leave()}>{busy ? "해제 중…" : "연결 해제"}</button>
          <button disabled={busy} onClick={() => setConfirmLeave(false)}>취소</button></div>
      </div>}
      <p className="note">내용과 기기 이름은 기기끼리만 아는 키로 암호화되어 서버는 읽을 수 없습니다. 비밀번호 관리자에서 복사한 내용은 서버 설정에서 허용하지 않으면 이 기기에만 남아요.</p>
    </> : <fieldset className="share-fields" disabled={busy || !loaded}>
      <label>이 기기 이름<input maxLength={64} placeholder="다른 기기에 표시될 이름" autoComplete="off" spellCheck={false} {...field("name")} /></label>
      <label>서버 주소<input type="url" placeholder="https://example.iptime.org" autoComplete="off" spellCheck={false} {...field("server")} /></label>
      <label>연결 코드<input inputMode="numeric" placeholder="37-1234-5678" autoComplete="off" spellCheck={false} {...field("code")} /></label>
      <div className="row">
        <button className="primary" disabled={!form.server.trim() || !form.code.trim()} onClick={() => void join()}>{busy ? "연결 중…" : "연결"}</button>
      </div>
      <p className="note">이미 연결된 기기의 설정에서 <b>컴퓨터 추가</b>를 누르면 코드가 나옵니다. 처음 연결하는 기기라면 서버를 처음 실행할 때 로그에 나온 첫 연결 코드(예: 1234-5678-9012)를 입력하세요. 연결하면 서버의 히스토리를 받아오고 이 기기의 히스토리도 올라가 모든 기기가 같아져요.</p>
    </fieldset>}
    {message && <p className="storage-message" role="status">{message}</p>}
    {error && <p className="update-error" role="alert">{error}</p>}
  </section>;
}
