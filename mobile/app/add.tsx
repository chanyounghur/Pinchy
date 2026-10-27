// The add sheet: pick photos or files to upload, or send what is on the
// clipboard with one tap on its preview.

import { useRouter } from "expo-router";
import * as Haptics from "expo-haptics";
import React, { useEffect, useMemo, useState } from "react";
import { ActivityIndicator, Platform, Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useApp } from "@/app-context";
import { draftFromClipboard } from "@/clipboard";
import { pickFiles, pickPhotos, setPendingDraft } from "@/pick";
import { tooLarge, upload, type Draft } from "@/upload";
import { DraftCard } from "@/ui/DraftCard";
import { radius, useTheme } from "@/ui/theme";
import { useToast } from "@/ui/Toast";

type Busy = "photos" | "files" | "clipboard" | null;

export default function Add() {
  const { session, engine, policy } = useApp();
  const t = useTheme();
  const router = useRouter();
  const toast = useToast();
  const insets = useSafeAreaInsets();
  const [clip, setClip] = useState<Draft | null | undefined>(undefined);
  const [busy, setBusy] = useState<Busy>(null);
  const maxMb = policy?.max_blob_mb ?? 64;

  useEffect(() => {
    let cancelled = false;
    draftFromClipboard()
      .then((d) => {
        if (!cancelled) setClip(d);
      })
      .catch(() => {
        if (!cancelled) setClip(null);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const limit = useMemo(() => (clip ? tooLarge(clip, maxMb) : null), [clip, maxMb]);

  const pick = async (which: "photos" | "files") => {
    setBusy(which);
    try {
      const draft = which === "photos" ? await pickPhotos() : await pickFiles();
      if (!draft) return;
      setPendingDraft(draft);
      router.replace("/upload?source=picked");
    } catch (e) {
      toast(e instanceof Error ? e.message : "고른 것을 읽지 못했어요");
    } finally {
      setBusy(null);
    }
  };

  const sendClipboard = async () => {
    if (!engine || !session || !clip || limit) return;
    setBusy("clipboard");
    try {
      await upload(engine, session.name, clip);
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
      toast("올렸어요");
      router.back();
    } catch (e) {
      toast(e instanceof Error ? e.message : "올리지 못했어요");
    } finally {
      setBusy(null);
    }
  };

  const disabled = busy !== null || !engine;

  return (
    <View style={[styles.sheet, { backgroundColor: t.bg, paddingBottom: insets.bottom + 16 }]}>
      <View style={[styles.group, { backgroundColor: t.card }]}>
        <Row
          glyph="◫"
          title="사진 올리기"
          hint="앨범에서 고르기 · 여러 장도 돼요"
          busy={busy === "photos"}
          disabled={disabled}
          onPress={() => void pick("photos")}
        />
        <View style={[styles.separator, { backgroundColor: t.separator }]} />
        <Row
          glyph="▤"
          title="파일 올리기"
          hint={`파일 앱에서 고르기 · 최대 ${maxMb}MB`}
          busy={busy === "files"}
          disabled={disabled}
          onPress={() => void pick("files")}
        />
      </View>

      <View style={styles.sectionRow}>
        <Text style={[styles.section, { color: t.muted }]}>클립보드</Text>
        {clip ? <Text style={{ color: t.muted, fontSize: 12 }}>터치하면 바로 올라가요</Text> : null}
      </View>

      {clip === undefined ? (
        <View style={[styles.empty, { borderColor: t.chipStrong }]}>
          <ActivityIndicator />
        </View>
      ) : clip === null ? (
        <View style={[styles.empty, { borderColor: t.chipStrong }]}>
          <Text style={{ color: t.muted, fontSize: 15, fontWeight: "600" }}>클립보드가 비어 있어요</Text>
          <Text style={{ color: t.muted, fontSize: 13, lineHeight: 19, textAlign: "center" }}>다른 앱에서 복사한 뒤 다시 열면{"\n"}여기에 미리보기가 나와요.</Text>
        </View>
      ) : (
        <>
          <DraftCard draft={clip} from={session?.name} onPress={() => void sendClipboard()} busy={busy === "clipboard"} disabled={disabled || !!limit} />
          {limit ? <Text style={[styles.note, { color: t.danger }]}>{limit}</Text> : null}
        </>
      )}

      {Platform.OS === "ios" ? (
        <Text style={[styles.note, { color: t.muted }]}>iOS가 붙여넣기를 물어보면 허용해 주세요. 설정 → Pinchy → 다른 앱에서 붙여넣기를 "허용"으로 두면 다시 묻지 않아요.</Text>
      ) : null}
    </View>
  );
}

function Row({ glyph, title, hint, busy, disabled, onPress }: { glyph: string; title: string; hint: string; busy: boolean; disabled: boolean; onPress: () => void }) {
  const t = useTheme();
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled}
      accessibilityRole="button"
      accessibilityLabel={title}
      style={({ pressed }) => [styles.row, { opacity: pressed ? 0.7 : disabled && !busy ? 0.5 : 1 }]}
    >
      <View style={[styles.glyphBox, { backgroundColor: t.dark ? "rgba(255,102,82,0.2)" : "rgba(255,102,82,0.14)" }]}>
        {busy ? <ActivityIndicator color={t.accentStrong} /> : <Text style={{ color: t.accentStrong, fontSize: 18, fontWeight: "700" }}>{glyph}</Text>}
      </View>
      <View style={{ flex: 1, gap: 2 }}>
        <Text style={{ color: t.cardFg, fontSize: 16, fontWeight: "600" }}>{title}</Text>
        <Text style={{ color: t.cardMuted, fontSize: 12 }}>{hint}</Text>
      </View>
      <Text style={{ color: t.cardMuted, fontSize: 18 }}>›</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  sheet: { paddingTop: 14, paddingHorizontal: 20, gap: 0 },
  group: { borderRadius: radius.card, overflow: "hidden" },
  row: { flexDirection: "row", alignItems: "center", gap: 14, height: 60, paddingHorizontal: 16 },
  glyphBox: { width: 36, height: 36, borderRadius: 10, alignItems: "center", justifyContent: "center" },
  separator: { height: 1, marginLeft: 66 },
  sectionRow: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", marginTop: 22, marginBottom: 10, paddingHorizontal: 4 },
  section: { fontSize: 13, fontWeight: "700", letterSpacing: 0.2 },
  empty: { alignItems: "center", gap: 6, paddingVertical: 26, paddingHorizontal: 20, borderRadius: radius.card, borderWidth: 1.5, borderStyle: "dashed" },
  note: { marginTop: 10, paddingHorizontal: 4, fontSize: 12, lineHeight: 18 },
});
