// Putting something into the history from this phone: what another app
// shared with us, or what is on the clipboard. Shows the draft as the card
// it will become, then seals and uploads it.

import { Image } from "expo-image";
import { useShareIntentContext } from "expo-share-intent";
import { useLocalSearchParams, useRouter } from "expo-router";
import * as Haptics from "expo-haptics";
import React, { useEffect, useMemo, useState } from "react";
import { ActivityIndicator, ScrollView, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useApp } from "@/app-context";
import { draftFromClipboard, draftFromShare } from "@/clipboard";
import { encodeBase64 } from "@/protocol/base64";
import { tooLarge, upload, type Draft } from "@/upload";
import { Button } from "@/ui/Button";
import { bytes, domainOf } from "@/ui/format";
import { useTheme } from "@/ui/theme";
import { useToast } from "@/ui/Toast";

export default function Add() {
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
        const d = fromShare ? (share.hasShareIntent ? await draftFromShare(share.shareIntent) : null) : await draftFromClipboard();
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
    // The share intent is read once when the sheet opens.
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

  const png = draft?.png;
  const pngUri = useMemo(() => (png ? `data:image/png;base64,${encodeBase64(png)}` : null), [png]);

  return (
    <View style={[styles.screen, { backgroundColor: t.bg, paddingBottom: insets.bottom + 16 }]}>
      <View style={styles.grip}>
        <View style={[styles.gripBar, { backgroundColor: t.chipStrong }]} />
      </View>
      <View style={styles.titleRow}>
        <Text style={[styles.title, { color: t.fg }]}>Pinchy에 추가</Text>
        <Text style={{ color: t.muted, fontSize: 13, fontWeight: "500" }}>{fromShare ? "공유됨" : "클립보드에서"}</Text>
      </View>

      <ScrollView contentContainerStyle={styles.content}>
        {draft === undefined ? (
          <View style={styles.center}>
            <ActivityIndicator />
          </View>
        ) : draft === null ? (
          <View style={styles.center}>
            <Text style={{ color: t.muted, fontSize: 15, textAlign: "center", lineHeight: 22 }}>
              {error ?? (fromShare ? "공유된 내용이 없어요." : "클립보드가 비어 있어요.\n다른 앱에서 복사한 뒤 다시 눌러 주세요.")}
            </Text>
          </View>
        ) : (
          <View style={[styles.card, { backgroundColor: t.card }]}>
            <View style={[styles.head, { backgroundColor: t.accentStrong }]}>
              <Text style={styles.kind}>{draft.kind === "image" ? "이미지" : draft.kind === "files" ? "파일" : draft.kind === "link" ? "링크" : "텍스트"}</Text>
              <Text style={styles.time}>지금</Text>
            </View>
            {draft.kind === "image" && pngUri ? (
              <Image source={{ uri: pngUri }} style={[styles.image, { backgroundColor: t.chip }]} contentFit="contain" accessibilityLabel="올릴 이미지" />
            ) : draft.kind === "files" ? (
              <View style={{ padding: 14, gap: 8 }}>
                {draft.files?.map((f, i) => (
                  <View key={`${f.name}-${i}`} style={{ flexDirection: "row", justifyContent: "space-between", gap: 12 }}>
                    <Text style={{ color: t.fg, fontWeight: "600", flex: 1 }} numberOfLines={1}>
                      {f.name}
                    </Text>
                    <Text style={{ color: t.muted }}>{bytes(f.bytes.length)}</Text>
                  </View>
                ))}
              </View>
            ) : (
              <View style={{ padding: 14 }}>
                {draft.kind === "link" ? <Text style={{ color: t.muted, fontSize: 11, marginBottom: 4 }}>{domainOf(draft.text ?? "")}</Text> : null}
                <Text style={{ color: t.fg, fontSize: 14, lineHeight: 21 }} numberOfLines={12}>
                  {draft.text}
                </Text>
              </View>
            )}
            <View style={styles.foot}>
              <Text style={{ color: t.origin, fontWeight: "600", fontSize: 13, flex: 1 }}>{session?.name}</Text>
              <Text style={[styles.badge, { backgroundColor: t.chip, color: t.cardMuted }]}>
                {draft.kind === "image" && png ? bytes(png.length) : draft.kind === "files" ? `파일 ${draft.files?.length ?? 0}개` : `${[...(draft.text ?? "")].length}자`}
              </Text>
            </View>
          </View>
        )}
        {draft ? (
          <Text style={{ color: limit ? t.danger : t.muted, fontSize: 13, lineHeight: 19, paddingHorizontal: 4 }}>
            {limit ?? "이 폰에서 암호화한 뒤 올라가요. 연결된 모든 기기의 히스토리에 바로 나타나요."}
          </Text>
        ) : null}
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
  card: { borderRadius: 16, overflow: "hidden" },
  head: { height: 44, paddingTop: 7, paddingHorizontal: 14 },
  kind: { color: "#fff", fontSize: 15, fontWeight: "600", lineHeight: 18 },
  time: { color: "rgba(255,255,255,0.9)", fontSize: 12, lineHeight: 15 },
  image: { marginHorizontal: 14, marginTop: 12, height: 220, borderRadius: 10 },
  foot: { flexDirection: "row", alignItems: "center", gap: 10, paddingHorizontal: 12, paddingTop: 8, paddingBottom: 10 },
  badge: { height: 22, lineHeight: 22, paddingHorizontal: 7, borderRadius: 7, fontSize: 13, fontWeight: "600" },
  actions: { gap: 10, paddingTop: 8 },
});
