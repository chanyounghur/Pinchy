// A draft drawn as the card it will become in the history, so the person
// sees what they are about to upload. With onPress the whole card is the
// upload button and says so.

import { Image } from "expo-image";
import React, { useMemo } from "react";
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from "react-native";
import { encodeBase64 } from "../protocol/base64";
import type { Draft } from "../upload";
import { bytes, domainOf } from "./format";
import { radius, useTheme } from "./theme";

export function DraftCard({
  draft,
  from,
  onPress,
  busy = false,
  disabled = false,
}: {
  draft: Draft;
  from: string | undefined;
  onPress?: () => void;
  busy?: boolean;
  disabled?: boolean;
}) {
  const t = useTheme();
  const png = draft.png;
  const pngUri = useMemo(() => (png ? `data:image/png;base64,${encodeBase64(png)}` : null), [png]);
  const kind = draft.kind === "image" ? "이미지" : draft.kind === "files" ? "파일" : draft.kind === "link" ? "링크" : "텍스트";
  const badge = draft.kind === "image" && png ? bytes(png.length) : draft.kind === "files" ? `파일 ${draft.files?.length ?? 0}개` : `${[...(draft.text ?? "")].length}자`;
  const pressable = !!onPress;

  return (
    <Pressable
      onPress={onPress}
      disabled={!pressable || disabled || busy}
      accessibilityRole={pressable ? "button" : undefined}
      accessibilityLabel={pressable ? `${kind} 바로 올리기` : undefined}
      style={({ pressed }) => [
        styles.card,
        { backgroundColor: t.card, opacity: pressed ? 0.85 : 1 },
        pressable && !disabled ? { borderWidth: 2, borderColor: t.dark ? "rgba(255,102,82,0.45)" : "rgba(216,67,43,0.35)" } : null,
      ]}
    >
      <View style={[styles.head, { backgroundColor: t.accentStrong }]}>
        <Text style={styles.kind}>{kind}</Text>
        <Text style={styles.time}>지금</Text>
      </View>
      {draft.kind === "image" && pngUri ? (
        <Image source={{ uri: pngUri }} style={[styles.image, { backgroundColor: t.chip }]} contentFit="contain" accessibilityLabel="올릴 이미지" />
      ) : draft.kind === "files" ? (
        <View style={{ padding: 14, gap: 8 }}>
          {draft.files?.map((f, i) => (
            <View key={`${f.name}-${i}`} style={{ flexDirection: "row", justifyContent: "space-between", gap: 12 }}>
              <Text style={{ color: t.cardFg, fontWeight: "600", flex: 1 }} numberOfLines={1}>
                {f.name}
              </Text>
              <Text style={{ color: t.cardMuted }}>{bytes(f.bytes.length)}</Text>
            </View>
          ))}
        </View>
      ) : (
        <View style={{ padding: 14 }}>
          {draft.kind === "link" ? <Text style={{ color: t.cardMuted, fontSize: 11, marginBottom: 4 }}>{domainOf(draft.text ?? "")}</Text> : null}
          <Text style={{ color: t.cardFg, fontSize: 14, lineHeight: 21 }} numberOfLines={pressable ? 6 : 12}>
            {draft.text}
          </Text>
        </View>
      )}
      <View style={styles.foot}>
        <Text style={{ color: t.origin, fontWeight: "600", fontSize: 13, flex: 1 }} numberOfLines={1}>
          {from}
        </Text>
        <Text style={[styles.badge, { backgroundColor: t.chip, color: t.cardMuted }]}>{badge}</Text>
      </View>
      {pressable ? (
        <View style={[styles.band, { backgroundColor: t.accentStrong, opacity: disabled ? 0.5 : 1 }]}>
          {busy ? <ActivityIndicator color="#fff" /> : <Text style={styles.bandText}>↑ 이대로 올리기</Text>}
        </View>
      ) : null}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  card: { borderRadius: radius.card, overflow: "hidden" },
  head: { height: 44, paddingTop: 7, paddingHorizontal: 14 },
  kind: { color: "#fff", fontSize: 15, fontWeight: "600", lineHeight: 18 },
  time: { color: "rgba(255,255,255,0.9)", fontSize: 12, lineHeight: 15 },
  image: { marginHorizontal: 14, marginTop: 12, height: 220, borderRadius: 10 },
  foot: { flexDirection: "row", alignItems: "center", gap: 10, paddingHorizontal: 12, paddingTop: 8, paddingBottom: 10 },
  badge: { height: 22, lineHeight: 22, paddingHorizontal: 7, borderRadius: radius.chip, fontSize: 13, fontWeight: "600" },
  band: { height: 48, marginHorizontal: 14, marginBottom: 14, borderRadius: 24, alignItems: "center", justifyContent: "center" },
  bandText: { color: "#fff", fontSize: 15, fontWeight: "700" },
});
