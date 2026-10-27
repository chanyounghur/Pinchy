import { useLocalSearchParams, useRouter } from "expo-router";
import * as Haptics from "expo-haptics";
import React, { useCallback, useState } from "react";
import { Alert, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useItemActions } from "@/actions";
import { useApp, useItem } from "@/app-context";
import { copyItem, shareFile, shareImage } from "@/clipboard";
import { Button } from "@/ui/Button";
import { fullTime, kindLabel } from "@/ui/format";
import { ItemCard } from "@/ui/ItemCard";
import { useTheme } from "@/ui/theme";
import { useToast } from "@/ui/Toast";

export default function Detail() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const item = useItem(id ?? "");
  const { engine, session } = useApp();
  const t = useTheme();
  const router = useRouter();
  const toast = useToast();
  const insets = useSafeAreaInsets();
  const [busy, setBusy] = useState(false);
  const actions = useItemActions();

  const copy = useCallback(async () => {
    if (!engine || !item) return;
    setBusy(true);
    try {
      const result = await copyItem(engine, item);
      if (result === "copied") {
        void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
        toast("복사됨");
      } else if (result === "pending") toast("아직 받는 중이에요");
    } catch (e) {
      toast(e instanceof Error ? e.message : "복사하지 못했어요");
    } finally {
      setBusy(false);
    }
  }, [engine, item, toast]);

  const share = useCallback(async () => {
    if (!item) return;
    try {
      if (item.kind === "image") await shareImage(item);
      else if (item.kind === "files") await shareFile(item, 0);
      else {
        const { Share } = await import("react-native");
        await Share.share({ message: item.text ?? "" });
      }
    } catch (e) {
      toast(e instanceof Error ? e.message : "공유하지 못했어요");
    }
  }, [item, toast]);

  const remove = useCallback(() => {
    if (!engine || !item) return;
    Alert.alert("모든 기기에서 삭제", "서버와 연결된 모든 기기의 히스토리에서 사라져요.", [
      { text: "취소", style: "cancel" },
      {
        text: "삭제",
        style: "destructive",
        onPress: () => {
          router.back();
          engine.deleteItem(item.id).catch((e: unknown) => toast(e instanceof Error ? e.message : "삭제하지 못했어요"));
        },
      },
    ]);
  }, [engine, item, router, toast]);

  if (item === undefined) return <View style={{ flex: 1, backgroundColor: t.bg }} />;
  if (item === null) {
    return (
      <View style={[styles.screen, { backgroundColor: t.bg, paddingTop: insets.top }]}>
        <Header onBack={() => router.back()} color={t.origin} />
        <Text style={{ color: t.muted, textAlign: "center", marginTop: 80 }}>이 항목은 삭제됐어요.</Text>
      </View>
    );
  }

  const mine = item.origin === session?.deviceId;
  return (
    <View style={[styles.screen, { backgroundColor: t.bg, paddingTop: insets.top }]}>
      <Header onBack={() => router.back()} color={t.origin} />
      <ScrollView contentContainerStyle={styles.content}>
        <ItemCard
          item={item}
          full
          onOpenPicture={(i) => router.push(`/view/${item.id}?index=${i}`)}
          actions={{ onSave: () => void actions.save(item), onCopy: () => void actions.copy(item), onShare: () => void actions.share(item) }}
        />
        <View style={[styles.meta, { backgroundColor: t.card }]}>
          <Row label="복사한 기기" value={item.from + (mine ? " (이 기기)" : "")} valueColor={t.origin} t={t} />
          <Row label="앱" value={item.app ?? "알 수 없음"} t={t} />
          <Row label="복사한 시각" value={fullTime(item.copied_at)} t={t} />
          <Row label="종류" value={kindLabel(item.kind, item.files)} t={t} last />
        </View>
        {item.kind === "files" && item.files && item.files.length > 1 ? (
          <View style={[styles.meta, { backgroundColor: t.card }]}>
            {item.files.map((f, i) => (
              <Pressable
                key={`${f.name}-${i}`}
                onPress={() => void shareFile(item, i).catch(() => toast("아직 받는 중이에요"))}
                style={[styles.row, i === item.files!.length - 1 ? null : { borderBottomWidth: 1, borderBottomColor: t.separator }]}
                accessibilityRole="button"
              >
                <Text style={{ color: t.fg, flex: 1 }} numberOfLines={1}>
                  {f.name}
                </Text>
                <Text style={{ color: t.origin, fontWeight: "600" }}>내보내기</Text>
              </Pressable>
            ))}
          </View>
        ) : null}
      </ScrollView>
      <View style={[styles.actions, { paddingBottom: insets.bottom + 16 }]}>
        <Button title={item.kind === "files" ? "파일 내보내기" : "복사"} onPress={() => void copy()} busy={busy} />
        <View style={styles.actionRow}>
          <Button title="공유" kind="secondary" onPress={() => void share()} style={styles.half} />
          <Button title="모든 기기에서 삭제" kind="danger" onPress={remove} style={styles.half} />
        </View>
      </View>
    </View>
  );
}

function Header({ onBack, color }: { onBack: () => void; color: string }) {
  return (
    <View style={styles.header}>
      <Pressable onPress={onBack} style={styles.back} accessibilityRole="button" accessibilityLabel="히스토리로 돌아가기">
        <Text style={{ color, fontSize: 16, fontWeight: "500" }}>‹ 히스토리</Text>
      </Pressable>
    </View>
  );
}

function Row({ label, value, valueColor, t, last }: { label: string; value: string; valueColor?: string; t: ReturnType<typeof useTheme>; last?: boolean }) {
  return (
    <View style={[styles.row, last ? null : { borderBottomWidth: 1, borderBottomColor: t.separator }]}>
      <Text style={{ color: t.muted }}>{label}</Text>
      <Text style={{ color: valueColor ?? t.fg, fontWeight: "600", flexShrink: 1, textAlign: "right" }} numberOfLines={2}>
        {value}
      </Text>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  header: { height: 44, flexDirection: "row", alignItems: "center", paddingHorizontal: 8 },
  back: { height: 44, paddingHorizontal: 8, justifyContent: "center" },
  content: { paddingHorizontal: 20, paddingTop: 6, paddingBottom: 24, gap: 14 },
  meta: { borderRadius: 16 },
  row: { minHeight: 46, flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: 16, paddingHorizontal: 16, paddingVertical: 10 },
  actions: { paddingHorizontal: 20, gap: 10 },
  actionRow: { flexDirection: "row", gap: 10 },
  half: { flex: 1, height: 48, paddingHorizontal: 12 },
});
