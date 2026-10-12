import { useEffect, useRef, useState } from "react";
import { invoke, listen } from "./tauri";

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
        setMessage("히스토리를 모두 제거했어요.");
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
    <p className="note">선택한 조건 중 하나라도 초과하면 오래된 기록부터 정리합니다. 둘 다 끄면 자동으로 삭제하지 않습니다.</p>
    <p className="note storage-detail">기간은 마지막 복사 시점 기준이며, 실행 중 1분마다 확인합니다. 용량은 기록·이미지·미리보기·앱 캐시를 포함합니다. 1,024MB = 1GB.</p>
    {!valid && <p className="update-error" role="alert">기간은 1~36,500일, 용량은 1~1,048,576MB 사이의 정수로 입력해 주세요.</p>}
    <div className="row storage-actions">
      <button className="primary" disabled={busy || !saved || !changed || !valid}
        onClick={() => { setMessage(""); if (daysEnabled || sizeEnabled) setConfirm("save"); else void run("save"); }}>설정 저장</button>
      <button className="danger" disabled={busy || !usage || usage.count === 0}
        onClick={() => { setMessage(""); setConfirm("clear"); }}>히스토리 제거</button>
    </div>
    {confirm && <div className="storage-confirm" role="group" aria-label={confirm === "clear" ? "히스토리 제거 확인" : "보관 설정 적용 확인"}>
      <p>{confirm === "clear"
        ? `기록 ${usage?.count.toLocaleString() ?? "전체"}개와 저장된 이미지·미리보기를 모두 제거할까요? 복사했던 원본 파일은 유지됩니다.`
        : `${[daysEnabled ? `${days}일이 지난 기록` : "", sizeEnabled ? `${maxMb}MB를 초과하는 오래된 기록` : ""].filter(Boolean).join(" 또는 ")}을 지금부터 자동으로 삭제합니다.`}</p>
      <p>삭제한 기록은 되돌릴 수 없습니다.</p>
      <div className="row"><button className="primary" disabled={busy} onClick={() => void run(confirm)}>
        {busy ? "처리 중…" : confirm === "clear" ? "모두 제거" : "적용하고 정리"}
      </button><button disabled={busy} onClick={() => setConfirm(null)}>취소</button></div>
    </div>}
    {busy && !confirm && <p className="note" role="status">처리 중…</p>}
    {message && <p className="storage-message" role="status">{message}</p>}
    {error && <p className="update-error" role="alert">{error}</p>}
    <p className="note storage-detail">기록을 모두 제거해도 앱 실행에 필요한 기본 저장 공간은 남습니다.</p>
  </section>;
}
