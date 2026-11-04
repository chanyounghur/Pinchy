import { useEffect, useRef, useState } from "react";
import { invoke, listen } from "./tauri";
import type { Policy, Status } from "./ShareSettings";

type Retention = { days_enabled: boolean; days: number; size_enabled: boolean; max_mb: number };
type Usage = { bytes: number; count: number };
type CleanupResult = { removed: number; usage: Usage };

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)}KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)}MB`;
  return `${(bytes / 1024 ** 3).toFixed(2)}GB`;
}

export default function StorageSettings() {
  const [saved, setSaved] = useState<Retention | null>(null);
  const [daysEnabled, setDaysEnabled] = useState(true);
  const [sizeEnabled, setSizeEnabled] = useState(true);
  const [days, setDays] = useState("30");
  const [maxMb, setMaxMb] = useState("1024");
  const [usage, setUsage] = useState<Usage | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState<"save" | "clear" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState("");
  const [policy, setPolicy] = useState<Policy | null>(null);
  const pending = useRef(false);

  useEffect(() => {
    let disposed = false;
    const stops: (() => void)[] = [];
    let debounce: ReturnType<typeof setTimeout> | undefined;
    const refresh = async () => {
      try {
        const next = await invoke<Usage>("storage_usage");
        if (!disposed) setUsage(next);
      } catch (e) { if (!disposed) setError(String(e)); }
    };
    const load = async () => {
      const settings = await invoke<{ retention: Retention }>("get_settings");
      if (!disposed) {
        setSaved(settings.retention);
        setDaysEnabled(settings.retention.days_enabled);
        setSizeEnabled(settings.retention.size_enabled);
        setDays(String(settings.retention.days));
        setMaxMb(String(settings.retention.max_mb));
      }
    };
    void load().catch((e) => { if (!disposed) setError(String(e)); });
    void refresh();
    const applyStatus = (status: Status) => setPolicy(status.state === "connected" ? status.policy : null);
    void invoke<Status>("share_status").then((status) => { if (!disposed) applyStatus(status); }).catch(() => {});
    void (async () => {
      const stop = await listen<Status>("share-status", ({ payload }) => { if (!disposed) applyStatus(payload); });
      if (disposed) stop(); else stops.push(stop);
    })().catch(() => {});
    void (async () => {
      for (const [name, handler] of [
        ["clipboard-changed", () => {
          clearTimeout(debounce);
          debounce = setTimeout(() => void refresh(), 250);
        }],
        ["storage-error", (event: { payload: string }) => setError(event.payload)],
      ] as const) {
        const stop = await listen<string>(name, (event) => { if (!disposed) handler(event); });
        if (disposed) stop(); else stops.push(stop);
      }
    })().catch((e) => { if (!disposed) setError(String(e)); });
    const timer = setInterval(() => void refresh(), 15_000);
    window.addEventListener("focus", refresh);
    return () => {
      disposed = true;
      clearInterval(timer);
      clearTimeout(debounce);
      stops.forEach((stop) => stop());
      window.removeEventListener("focus", refresh);
    };
  }, []);

  const validDays = /^\d+$/.test(days) && Number(days) >= 1 && Number(days) <= 36500;
  const validSize = /^\d+$/.test(maxMb) && Number(maxMb) >= 1 && Number(maxMb) <= 1048576;
  const valid = (!daysEnabled || validDays) && (!sizeEnabled || validSize);
  const retention: Retention = {
    days_enabled: daysEnabled,
    days: validDays ? Number(days) : saved?.days ?? 30,
    size_enabled: sizeEnabled,
    max_mb: validSize ? Number(maxMb) : saved?.max_mb ?? 1024,
  };
  const changed = saved && JSON.stringify(saved) !== JSON.stringify(retention);
  const resetNotice = () => { setConfirm(null); setMessage(""); };

  const run = async (action: "save" | "clear") => {
    if (pending.current || (action === "save" && !valid)) return;
    pending.current = true;
    setBusy(true);
    setError(null);
    setMessage("");
    try {
      if (action === "save") {
        const result = await invoke<CleanupResult>("set_retention", { retention });
        setSaved(retention);
        setDays(String(retention.days));
        setMaxMb(String(retention.max_mb));
        setUsage(result.usage);
        setMessage(result.removed ? `설정을 저장하고 기록 ${result.removed.toLocaleString()}개를 정리했어요.` : "보관 설정을 저장했어요.");
      } else {
        await invoke("clear_history");
        setUsage(await invoke<Usage>("storage_usage"));
        setMessage("히스토리를 비웠어요.");
      }
    } catch (e) {
      setError(String(e));
      // A cleanup error can occur after settings have been saved.
      try {
        const settings = await invoke<{ retention: Retention }>("get_settings");
        setSaved(settings.retention);
        setUsage(await invoke<Usage>("storage_usage"));
      } catch { /* Keep the original actionable error visible. */ }
    } finally {
      setConfirm(null);
      pending.current = false;
      setBusy(false);
    }
  };

  const clearButton = <button className="danger" disabled={busy || !usage || usage.count === 0}
    onClick={() => { setMessage(""); setConfirm("clear"); }}>히스토리 비우기</button>;
  const clearConfirm = confirm === "clear" && <div className="storage-confirm" role="group" aria-label="히스토리 비우기 확인">
    <p>{`${usage ? `기록 ${usage.count.toLocaleString()}개를` : "기록을"} 모두 지울까요? 이미지와 미리보기도 함께 지워져요. 복사했던 원본 파일은 그대로 남아요.`}{policy ? " 연결된 다른 기기에서도 지워져요." : ""}</p>
    <p>지운 기록은 되돌릴 수 없어요.</p>
    <div className="row"><button className="primary" disabled={busy} onClick={() => void run("clear")}>
      {busy ? "처리 중…" : "모두 지우기"}
    </button><button disabled={busy} onClick={() => setConfirm(null)}>닫기</button></div>
  </div>;

  if (policy) {
    return <section className="storage" aria-labelledby="storage-heading">
      <h1 id="storage-heading">히스토리 보관</h1>
      <div className="storage-usage" aria-live="polite">
        <span>이 기기 사용량</span>
        <strong>{usage ? formatBytes(usage.bytes) : "확인 중…"}</strong>
        <span className="hint">{usage ? `기록 ${usage.count.toLocaleString()}개` : ""}</span>
      </div>
      <div className="share-summary storage-policy">
        <div><span>보관 기간</span><strong>{policy.retention_days.toLocaleString()}일</strong></div>
        <div><span>최대 용량</span><strong>{policy.max_storage_mb.toLocaleString()}MB</strong></div>
        <div><span>서버 사용량</span><strong>{policy.usage ? `${formatBytes(policy.usage.bytes)} · 기록 ${policy.usage.items.toLocaleString()}개` : "확인 중…"}</strong></div>
        <div><span>비밀번호 관리자 항목</span><strong>{policy.sync_concealed ? "동기화함" : "이 기기에만 보관"}</strong></div>
      </div>
      <p className="note">동기화를 연결한 동안에는 서버에 정해진 보관 기간과 용량을 따라요. 마지막으로 복사한 지 보관 기간이 지나거나 최대 용량을 넘으면 오래된 기록부터 모든 기기에서 지워져요.</p>
      <div className="row storage-actions">{clearButton}</div>
      {clearConfirm}
      {busy && !confirm && <p className="note" role="status">처리 중…</p>}
      {message && <p className="storage-message" role="status">{message}</p>}
      {error && <p className="update-error" role="alert">{error}</p>}
    </section>;
  }

  return <section className="storage" aria-labelledby="storage-heading">
    <h1 id="storage-heading">히스토리 보관</h1>
    <div className="storage-usage" aria-live="polite">
      <span>현재 사용량</span>
      <strong>{usage ? formatBytes(usage.bytes) : "확인 중…"}</strong>
      <span className="hint">{usage ? `기록 ${usage.count.toLocaleString()}개` : ""}</span>
    </div>
    <fieldset disabled={busy || !saved} className="retention-fields">
      <div className="retention-row">
        <label className="retention-label"><input type="checkbox" checked={daysEnabled}
          onChange={(e) => { setDaysEnabled(e.target.checked); resetNotice(); }} />보관 기간</label>
        <div className="retention-value"><input aria-label="보관 기간 (일)" type="number" min="1" max="36500" step="1"
          disabled={!daysEnabled} value={days} aria-invalid={daysEnabled && !validDays}
          onChange={(e) => { setDays(e.target.value); resetNotice(); }} /><span>일</span></div>
      </div>
      <div className="retention-row">
        <label className="retention-label"><input type="checkbox" checked={sizeEnabled}
          onChange={(e) => { setSizeEnabled(e.target.checked); resetNotice(); }} />최대 저장 용량</label>
        <div className="retention-value"><input aria-label="최대 저장 용량 (MB)" type="number" min="1" max="1048576" step="1"
          disabled={!sizeEnabled} value={maxMb} aria-invalid={sizeEnabled && !validSize}
          onChange={(e) => { setMaxMb(e.target.value); resetNotice(); }} /><span>MB</span></div>
      </div>
    </fieldset>
    <p className="note">켜둔 조건을 하나라도 넘으면 오래된 기록부터 지워요. 둘 다 끄면 자동으로 지우지 않아요. 동기화를 연결하면 이 설정 대신 서버에 정해진 기준을 따라요.</p>
    <p className="note storage-detail">기간은 마지막으로 복사한 때부터 세고, 앱이 켜져 있는 동안 1분마다 확인해요. 용량에는 기록, 이미지, 미리보기, 앱 캐시가 모두 들어가요. 1GB는 1,024MB예요.</p>
    {!valid && <p className="update-error" role="alert">기간은 1~36,500일, 용량은 1~1,048,576MB 사이로 소수점 없이 입력해주세요.</p>}
    <div className="row storage-actions">
      <button className="primary" disabled={busy || !saved || !changed || !valid}
        onClick={() => { setMessage(""); if (daysEnabled || sizeEnabled) setConfirm("save"); else void run("save"); }}>설정 저장</button>
      {clearButton}
    </div>
    {clearConfirm}
    {confirm === "save" && <div className="storage-confirm" role="group" aria-label="보관 설정 적용 확인">
      <p>{`지금부터 ${[daysEnabled ? `${days}일이 지난 기록` : "", sizeEnabled ? `${maxMb}MB를 넘는 오래된 기록` : ""].filter(Boolean).join("과 ")}을 자동으로 지워요.`}</p>
      <p>지운 기록은 되돌릴 수 없어요.</p>
      <div className="row"><button className="primary" disabled={busy} onClick={() => void run("save")}>
        {busy ? "처리 중…" : "저장하고 정리하기"}
      </button><button disabled={busy} onClick={() => setConfirm(null)}>닫기</button></div>
    </div>}
    {busy && !confirm && <p className="note" role="status">처리 중…</p>}
    {message && <p className="storage-message" role="status">{message}</p>}
    {error && <p className="update-error" role="alert">{error}</p>}
    <p className="note storage-detail">기록을 모두 지워도 앱이 쓰는 기본 공간은 조금 남아요.</p>
  </section>;
}
