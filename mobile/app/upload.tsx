// Confirming an upload: what another app shared with us, or what was picked
// on the add sheet. Shows the draft as the card it will become, then seals
// and uploads it.

import { useShareIntentContext } from "expo-share-intent";
import { useLocalSearchParams, useRouter } from "expo-router";
import * as Haptics from "expo-haptics";
import React, { useEffect, useMemo, useState } from "react";
import { ActivityIndicator, ScrollView, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useApp } from "@/app-context";
import { draftFromShare } from "@/clipboard";
import { takePendingDraft } from "@/pick";
import { tooLarge, upload, type Draft } from "@/upload";
import { Button } from "@/ui/Button";
import { DraftCard } from "@/ui/DraftCard";
import { useTheme } from "@/ui/theme";
import { useToast } from "@/ui/Toast";

export default function Upload() {
  const { source } = useLocalSearchParams<{ source?: string }>();
  const { session, engine, policy } = useApp();
  const share = useShareIntentContext();
  const t = useTheme();
  const router = useRouter();
  const toast = useToast();
  const insets = useSafeAreaInsets();
  const [draft, setDraft] = useState<Draft | null | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const fromShare = source === "share";

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const d = fromShare ? (share.hasShareIntent ? await draftFromShare(share.shareIntent) : null) : takePendingDraft();
        if (!cancelled) setDraft(d);
      } catch (e) {
        if (!cancelled) {
          setDraft(null);
          setError(e instanceof Error ? e.message : "내용을 읽지 못했어요.");
        }
      }
    })();
    return () => {
      cancelled = true;
    };
    // The share intent is read once when the screen opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fromShare]);

  const limit = useMemo(() => (draft ? tooLarge(draft, policy?.max_blob_mb ?? 64) : null), [draft, policy]);

  const close = () => {
    if (fromShare) share.resetShareIntent();
    router.back();
  };

  const send = async () => {
    if (!engine || !session || !draft) return;
    setBusy(true);
    setError(null);
    try {
      await upload(engine, session.name, draft);
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
      toast("올렸어요");
      close();
    } catch (e) {
      setError(e instanceof Error ? e.message : "올리지 못했어요.");
    } finally {
      setBusy(false);
    }
  };

  const hint =
    draft?.kind === "files" && !fromShare
      ? "이 폰에서 암호화한 뒤 올라가요. 연결된 모든 기기의 히스토리에 바로 나타나요. 사진 한 장은 이미지로, 여러 장은 사진 묶음으로 올라가요."
      : "이 폰에서 암호화한 뒤 올라가요. 연결된 모든 기기의 히스토리에 바로 나타나요.";

  return (
    <View style={[styles.screen, { backgroundColor: t.bg, paddingBottom: insets.bottom + 16 }]}>
      <View style={styles.grip}>
        <View style={[styles.gripBar, { backgroundColor: t.chipStrong }]} />
      </View>
      <View style={styles.titleRow}>
        <Text style={[styles.title, { color: t.fg }]}>Pinchy에 추가</Text>
        <Text style={{ color: t.muted, fontSize: 13, fontWeight: "500" }}>{fromShare ? "공유됨" : "이 폰에서"}</Text>
      </View>

      <ScrollView contentContainerStyle={styles.content}>
        {draft === undefined ? (
          <View style={styles.center}>
            <ActivityIndicator />
          </View>
        ) : draft === null ? (
          <View style={styles.center}>
            <Text style={{ color: t.muted, fontSize: 15, textAlign: "center", lineHeight: 22 }}>{error ?? (fromShare ? "공유된 내용이 없어요." : "고른 것이 없어요.")}</Text>
          </View>
        ) : (
          <DraftCard draft={draft} from={session?.name} />
        )}
        {draft ? <Text style={{ color: limit ? t.danger : t.muted, fontSize: 13, lineHeight: 19, paddingHorizontal: 4 }}>{limit ?? hint}</Text> : null}
        {error && draft ? <Text style={{ color: t.danger, fontSize: 13, paddingHorizontal: 4 }}>{error}</Text> : null}
      </ScrollView>

      <View style={styles.actions}>
        <Button title="히스토리에 올리기" onPress={() => void send()} busy={busy} disabled={!draft || !!limit || !engine} />
        <Button title="취소" kind="secondary" onPress={close} style={{ height: 48 }} />
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, paddingHorizontal: 20 },
  grip: { alignItems: "center", paddingTop: 10, paddingBottom: 8 },
  gripBar: { width: 40, height: 5, borderRadius: 3 },
  titleRow: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", marginBottom: 14 },
  title: { fontSize: 20, fontWeight: "800", letterSpacing: -0.4 },
  content: { gap: 14, paddingBottom: 16 },
  center: { paddingVertical: 60, paddingHorizontal: 20 },
  actions: { gap: 10, paddingTop: 8 },
});
