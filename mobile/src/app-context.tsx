// Wires the session, the database and the sync engine into React. The engine
// runs while the app is in the foreground and pauses in the background.

import * as Crypto from "expo-crypto";
import { fetch as expoFetch } from "expo/fetch";
import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { AppState } from "react-native";
import { Key } from "./protocol/key";
import { parseJoinLink } from "./protocol/join";
import { Relay, normalizeServer, type FetchLike, type Policy } from "./protocol/relay";
import { Db, type Item } from "./store/db";
import { clearSession, defaultDeviceName, loadSession, saveSession, type Session } from "./store/session";
import { SyncEngine, type SyncStatus } from "./sync";
import { removeAllBlobs } from "./blobs";
import { checkForUpdate, configureUpdates, type UpdateCheck } from "./updates";

const random = (n: number) => Crypto.getRandomBytes(n);
const fetchImpl = expoFetch as unknown as FetchLike;

export interface AppContextValue {
  ready: boolean;
  session: Session | null;
  engine: SyncEngine | null;
  status: SyncStatus;
  policy: Policy | null;
  /// A newer app update is downloaded and applies on the next cold start.
  updateReady: boolean;
  /// Joins the relay a desktop QR points at. Throws with a Korean message.
  connect(link: string): Promise<void>;
  disconnect(): Promise<void>;
  rename(name: string): Promise<void>;
  checkUpdates(force?: boolean): Promise<UpdateCheck>;
}

const Ctx = createContext<AppContextValue | null>(null);

export function AppProvider({ children }: { children: React.ReactNode }) {
  const [ready, setReady] = useState(false);
  const [session, setSession] = useState<Session | null>(null);
  const [engine, setEngine] = useState<SyncEngine | null>(null);
  const [status, setStatus] = useState<SyncStatus>({ state: "idle" });
  const [policy, setPolicy] = useState<Policy | null>(null);
  const [updateReady, setUpdateReady] = useState(false);
  const dbRef = useRef<Promise<Db> | null>(null);

  const checkUpdates = useCallback(async (force = false) => {
    const result = await checkForUpdate(force);
    if (result === "downloaded") setUpdateReady(true);
    return result;
  }, []);

  const openDb = useCallback(() => (dbRef.current ??= Db.open()), []);

  const build = useCallback(
    async (s: Session): Promise<SyncEngine> => {
      const key = Key.parse(s.key, random);
      if (!key) throw new Error("저장된 공유 키가 손상됐어요. 연결을 해제하고 다시 연결해 주세요.");
      const relay = new Relay(s.server, s.token, fetchImpl);
      return new SyncEngine(relay, key, await openDb(), s.deviceId);
    },
    [openDb],
  );

  useEffect(() => {
    let cancelled = false;
    (async () => {
      const s = await loadSession();
      if (cancelled) return;
      if (s) {
        try {
          setEngine(await build(s));
          setSession(s);
          configureUpdates(s);
          void checkUpdates();
        } catch {
          setSession(null);
        }
      }
      setReady(true);
    })();
    return () => {
      cancelled = true;
    };
  }, [build, checkUpdates]);

  useEffect(() => {
    if (!engine) return;
    const offStatus = engine.on("status", () => setStatus(engine.status));
    const offPolicy = engine.on("policy", () => setPolicy(engine.policy));
    setStatus(engine.status);
    setPolicy(engine.policy);
    if (AppState.currentState === "active" || AppState.currentState === "unknown") engine.start();
    const sub = AppState.addEventListener("change", (state) => {
      if (state === "active") {
        engine.start();
        void checkUpdates();
      } else engine.stop();
    });
    return () => {
      sub.remove();
      offStatus();
      offPolicy();
      engine.stop();
    };
  }, [engine, checkUpdates]);

  const connect = useCallback(
    async (link: string) => {
      const parsed = parseJoinLink(link);
      if (!parsed) throw new Error("Pinchy QR이 아니에요. 데스크톱 설정 → 공유 → 휴대폰 추가의 QR을 찍어 주세요.");
      const server = normalizeServer(parsed.server);
      if (!server) throw new Error("서버 주소를 읽지 못했어요.");
      const key = Key.parse(parsed.key, random);
      if (!key) throw new Error("QR의 공유 키가 올바르지 않아요.");
      const name = defaultDeviceName();
      const joined = await new Relay(server, null, fetchImpl).join(parsed.code, key.sealLabel(name));
      const s: Session = { server, deviceId: joined.device_id, token: joined.token, key: key.toString(), name };
      // A new relay means a new history: forget whatever an earlier one left.
      const db = await openDb();
      await db.clear();
      removeAllBlobs();
      await saveSession(s);
      setEngine(await build(s));
      setSession(s);
      configureUpdates(s);
      void checkUpdates(true);
    },
    [build, openDb, checkUpdates],
  );

  const disconnect = useCallback(async () => {
    const current = engine;
    engine?.stop();
    setEngine(null);
    setSession(null);
    setStatus({ state: "idle" });
    setPolicy(null);
    configureUpdates(null);
    await clearSession();
    try {
      await current?.relay.removeDevice(current.deviceId);
    } catch {}
    const db = await openDb();
    await db.clear();
    removeAllBlobs();
  }, [engine, openDb]);

  const rename = useCallback(
    async (name: string) => {
      if (!session || !engine) return;
      const clean = name.trim().slice(0, 40) || defaultDeviceName();
      await engine.relay.rename(engine.key.sealLabel(clean));
      const s = { ...session, name: clean };
      await saveSession(s);
      setSession(s);
    },
    [session, engine],
  );

  const value = useMemo<AppContextValue>(
    () => ({ ready, session, engine, status, policy, updateReady, connect, disconnect, rename, checkUpdates }),
    [ready, session, engine, status, policy, updateReady, connect, disconnect, rename, checkUpdates],
  );
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useApp(): AppContextValue {
  const v = useContext(Ctx);
  if (!v) throw new Error("AppProvider missing");
  return v;
}

/// The history, re-read whenever the engine applies changes.
export function useItems(search: string, limit = 500): { items: Item[]; refresh(): void } {
  const { engine } = useApp();
  const [items, setItems] = useState<Item[]>([]);
  const tick = useRef(0);
  const refresh = useCallback(() => {
    if (!engine) {
      setItems([]);
      return;
    }
    const my = ++tick.current;
    void engine.db.list(search, limit).then((rows) => {
      if (my === tick.current) setItems(rows);
    });
  }, [engine, search, limit]);
  useEffect(() => {
    refresh();
    return engine?.on("items", refresh);
  }, [engine, refresh]);
  return { items, refresh };
}

export function useItem(id: string): Item | null | undefined {
  const { engine } = useApp();
  const [item, setItem] = useState<Item | null | undefined>(undefined);
  useEffect(() => {
    if (!engine) {
      setItem(null);
      return;
    }
    const load = () => void engine.db.get(id).then(setItem);
    load();
    return engine.on("items", load);
  }, [engine, id]);
  return item;
}
