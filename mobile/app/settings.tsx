import { useRouter } from "expo-router";
import React, { useCallback, useEffect, useState } from "react";
import { Alert, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useApp } from "@/app-context";
import type { Device } from "@/protocol/relay";
import { Button } from "@/ui/Button";
import { bytes, relativeTime } from "@/ui/format";
import { useTheme } from "@/ui/theme";
import { useToast } from "@/ui/Toast";

export default function Settings() {
  const { session, engine, status, policy, rename, disconnect } = useApp();
  const t = useTheme();
  const router = useRouter();
  const toast = useToast();
  const insets = useSafeAreaInsets();
  const [name, setName] = useState(session?.name ?? "");
  const [devices, setDevices] = useState<{ me: string; devices: Device[] } | null>(null);
  const [count, setCount] = useState<number | null>(null);

  useEffect(() => {
    if (!engine) return;
    void engine.relay.devices().then(setDevices).catch(() => setDevices(null));
    void engine.refreshPolicy(true).catch(() => {});
    void engine.db.count().then(setCount);
  }, [engine]);

  const commitName = useCallback(async () => {
    if (!session || name.trim() === session.name) return;
    try {
      await rename(name);
      toast("이름을 바꿨어요");
    } catch (e) {
      toast(e instanceof Error ? e.message : "이름을 바꾸지 못했어요");
      setName(session.name);
    }
  }, [name, session, rename, toast]);

  const leave = useCallback(() => {
    Alert.alert("연결 해제", "이 폰의 키와 받아 둔 히스토리를 지워요. 다른 기기의 히스토리는 그대로예요.", [
      { text: "취소", style: "cancel" },
      {
        text: "연결 해제",
        style: "destructive",
        onPress: () => {
          void disconnect().then(() => router.replace("/join"));
        },
      },
    ]);
  }, [disconnect, router]);

  const clearAll = useCallback(() => {
    if (!engine) return;
    Alert.alert("히스토리 비우기", "서버와 모든 기기의 히스토리를 비워요. 되돌릴 수 없어요.", [
      { text: "취소", style: "cancel" },
      {
        text: "모두 삭제",
        style: "destructive",
        onPress: () => {
          engine
            .deleteAll()
            .then(() => toast("비웠어요"))
            .catch((e: unknown) => toast(e instanceof Error ? e.message : "비우지 못했어요"));
        },
      },
    ]);
  }, [engine, toast]);

  if (!session || !engine) return <View style={{ flex: 1, backgroundColor: t.bg }} />;

  const usage = policy?.usage;
  const cap = (policy?.max_storage_mb ?? 0) * 1024 * 1024;
  const ratio = usage && cap > 0 ? Math.min(1, usage.bytes / cap) : 0;
  const statusText =
    status.state === "live"
      ? "동기화됨"
      : status.state === "syncing"
        ? "받는 중…"
        : status.state === "offline"
          ? `오프라인 · ${status.message}`
          : status.state === "unauthorized"
            ? "이 기기의 연결이 해제됐어요"
            : "대기";

  return (
    <View style={[styles.screen, { backgroundColor: t.bg, paddingTop: insets.top }]}>
      <View style={styles.header}>
        <Pressable onPress={() => router.back()} style={styles.back} accessibilityRole="button" accessibilityLabel="히스토리로 돌아가기">
          <Text style={{ color: t.origin, fontSize: 16, fontWeight: "500" }}>‹ 히스토리</Text>
        </Pressable>
        <Text style={{ color: t.fg, fontSize: 17, fontWeight: "700", marginRight: 12 }}>설정</Text>
      </View>
      <ScrollView contentContainerStyle={[styles.content, { paddingBottom: insets.bottom + 24 }]} keyboardDismissMode="on-drag">
        <Section title="이 기기" t={t}>
          <View style={styles.row}>
            <Text style={{ color: t.fg }}>이름</Text>
            <TextInput
              value={name}
              onChangeText={setName}
              onBlur={() => void commitName()}
              onSubmitEditing={() => void commitName()}
              returnKeyType="done"
              maxLength={40}
              style={[styles.input, { color: t.fg }]}
              accessibilityLabel="기기 이름"
            />
          </View>
        </Section>

        <Section title="서버" t={t}>
          <Row label="주소" value={session.server.replace(/^https?:\/\//, "")} t={t} />
          <Row label="상태" value={statusText} dot={status.state === "live" ? t.ok : status.state === "offline" || status.state === "unauthorized" ? t.danger : t.muted} t={t} />
          <Row label="받아 둔 항목" value={count === null ? "…" : `${count}개`} t={t} last />
        </Section>

        <Section title={devices ? `연결된 기기 ${devices.devices.length}대` : "연결된 기기"} t={t}>
          {devices ? (
            devices.devices.map((d, i) => (
              <Row
                key={d.id}
                label={engine.key.openLabel(d.label) ?? "이름 없는 기기"}
                value={d.id === devices.me ? "이 기기" : d.last_seen ? relativeTime(d.last_seen) : ""}
                valueColor={d.id === devices.me ? t.origin : t.muted}
                t={t}
                last={i === devices.devices.length - 1}
              />
            ))
          ) : (
            <Row label="불러오는 중…" value="" t={t} last />
          )}
        </Section>

        <Section title="보관 · 서버가 정해요" t={t}>
          <Row label="보관 기간" value={policy ? `${policy.retention_days}일` : "…"} t={t} />
          <View style={[styles.row, { flexDirection: "column", alignItems: "stretch", gap: 8, borderBottomWidth: 1, borderBottomColor: t.separator }]}>
            <View style={{ flexDirection: "row", justifyContent: "space-between" }}>
              <Text style={{ color: t.fg }}>사용량</Text>
              <Text style={{ color: t.fg, fontWeight: "600" }}>
                {usage ? bytes(usage.bytes) : "…"} <Text style={{ color: t.muted, fontWeight: "500" }}>/ {policy ? `${policy.max_storage_mb} MB` : "…"}</Text>
              </Text>
            </View>
            <View style={[styles.bar, { backgroundColor: t.chip }]} accessibilityRole="progressbar" accessibilityValue={{ min: 0, max: 100, now: Math.round(ratio * 100) }}>
              <View style={[styles.barFill, { width: `${Math.round(ratio * 100)}%`, backgroundColor: t.accent }]} />
            </View>
          </View>
          <Row label="항목 최대 크기" value={policy ? `${policy.max_blob_mb} MB` : "…"} t={t} />
          <Row label="비밀번호 관리자 항목" value={policy ? (policy.sync_concealed ? "동기화함" : "동기화하지 않음") : "…"} t={t} last />
        </Section>

        <Section title="" t={t}>
          <Pressable onPress={clearAll} style={[styles.row, { justifyContent: "center", borderBottomWidth: 1, borderBottomColor: t.separator }]} accessibilityRole="button">
            <Text style={{ color: t.danger, fontSize: 15, fontWeight: "600" }}>모든 기기의 히스토리 비우기</Text>
          </Pressable>
          <Pressable onPress={leave} style={[styles.row, { justifyContent: "center" }]} accessibilityRole="button">
            <Text style={{ color: t.danger, fontSize: 15, fontWeight: "600" }}>연결 해제</Text>
          </Pressable>
        </Section>
        <Text style={{ color: t.muted, fontSize: 12, lineHeight: 18, marginHorizontal: 14 }}>
          연결을 해제하면 이 폰의 키와 받아 둔 히스토리를 지워요. 다른 기기의 히스토리는 그대로예요.
        </Text>
        {engine.locked > 0 ? (
          <Text style={{ color: t.muted, fontSize: 12, lineHeight: 18, marginHorizontal: 14 }}>
            다른 키로 잠긴 항목 {engine.locked}개는 건너뛰었어요.
          </Text>
        ) : null}
      </ScrollView>
    </View>
  );
}

function Section({ title, t, children }: { title: string; t: ReturnType<typeof useTheme>; children: React.ReactNode }) {
  return (
    <View style={{ gap: 6 }}>
      {title ? <Text style={[styles.sectionTitle, { color: t.muted }]}>{title}</Text> : null}
      <View style={[styles.group, { backgroundColor: t.card }]}>{children}</View>
    </View>
  );
}

function Row({ label, value, valueColor, dot, t, last }: { label: string; value: string; valueColor?: string; dot?: string; t: ReturnType<typeof useTheme>; last?: boolean }) {
  return (
    <View style={[styles.row, last ? null : { borderBottomWidth: 1, borderBottomColor: t.separator }]}>
      <Text style={{ color: t.fg, flexShrink: 1 }} numberOfLines={1}>
        {label}
      </Text>
      <View style={{ flexDirection: "row", alignItems: "center", gap: 6, flexShrink: 1 }}>
        {dot ? <View style={{ width: 8, height: 8, borderRadius: 4, backgroundColor: dot }} /> : null}
        <Text style={{ color: valueColor ?? t.fg, fontWeight: "600", textAlign: "right" }} numberOfLines={2}>
          {value}
        </Text>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  header: { height: 44, flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingHorizontal: 8 },
  back: { height: 44, paddingHorizontal: 8, justifyContent: "center" },
  content: { paddingHorizontal: 20, paddingTop: 6, gap: 18 },
  sectionTitle: { marginLeft: 14, fontSize: 12, fontWeight: "600", letterSpacing: 0.2 },
  group: { borderRadius: 16, overflow: "hidden" },
  row: { minHeight: 48, flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: 16, paddingHorizontal: 16, paddingVertical: 10 },
  input: { width: 180, fontSize: 15, fontWeight: "600", textAlign: "right", paddingVertical: 0 },
  bar: { height: 6, borderRadius: 3, overflow: "hidden" },
  barFill: { height: 6, borderRadius: 3 },
});
