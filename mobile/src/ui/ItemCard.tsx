// One history item as the desktop draws it: the source app's color on top
// with the kind and time, the content, and where it came from at the bottom.

import { Image } from "expo-image";
import React, { useEffect } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { useApp } from "../app-context";
import { imageFile } from "../blobs";
import type { Item } from "../store/db";
import { appColor, bytes, domainOf, initial, kindLabel, relativeTime } from "./format";
import { radius, useTheme, type Theme } from "./theme";

export function ItemCard({ item, onPress, onLongPress, full = false }: { item: Item; onPress?: () => void; onLongPress?: () => void; full?: boolean }) {
  const t = useTheme();
  const { engine } = useApp();
  const needsBlob = (item.kind === "image" || item.kind === "files") && item.blob_state === "missing";
  useEffect(() => {
    if (needsBlob && engine) void engine.ensureBlob(item.id);
  }, [needsBlob, engine, item.id]);
  const color = item.color ?? appColor(item.app);
  return (
    <Pressable
      onPress={onPress}
      onLongPress={onLongPress}
      accessibilityRole="button"
      accessibilityLabel={`${kindLabel(item.kind)}, ${item.from}`}
      style={({ pressed }) => [styles.card, { backgroundColor: t.card, opacity: pressed ? 0.85 : 1 }]}
    >
      <View style={[styles.head, { backgroundColor: color }]}>
        <Text style={styles.kind}>{kindLabel(item.kind)}</Text>
        <Text style={styles.time}>{relativeTime(item.copied_at)}</Text>
        {item.app ? (
          <View style={[styles.iconRing, { backgroundColor: t.card }]}>
            {item.icon ? (
              <Image source={{ uri: `data:image/png;base64,${item.icon}` }} style={styles.icon} contentFit="cover" accessibilityLabel={item.app} />
            ) : (
              <View style={[styles.icon, { backgroundColor: color }]}>
                <Text style={styles.iconText}>{initial(item.app)}</Text>
              </View>
            )}
          </View>
        ) : null}
      </View>
      <Body item={item} t={t} full={full} />
      <View style={styles.foot}>
        <Text style={[styles.origin, { color: t.origin }]} numberOfLines={1}>
          {item.from}
        </Text>
        <Text style={[styles.badge, { backgroundColor: t.chip, color: t.cardMuted }]}>{badge(item)}</Text>
      </View>
    </Pressable>
  );
}

function Body({ item, t, full }: { item: Item; t: Theme; full: boolean }) {
  switch (item.kind) {
    case "text":
      return (
        <Text style={[styles.text, { color: t.cardFg }]} numberOfLines={full ? undefined : 7} selectable={full}>
          {item.text}
        </Text>
      );
    case "link":
      return (
        <View style={styles.link}>
          <View style={[styles.linkIcon, { backgroundColor: t.chip }]}>
            <Text style={{ color: t.cardMuted, fontSize: 20 }}>↗</Text>
          </View>
          <View style={{ flex: 1, minWidth: 0 }}>
            <Text style={[styles.domain, { color: t.cardMuted }]} numberOfLines={1}>
              {domainOf(item.text ?? "")}
            </Text>
            <Text style={[styles.linkText, { color: t.cardFg }]} numberOfLines={full ? undefined : 3} selectable={full}>
              {item.text}
            </Text>
          </View>
        </View>
      );
    case "image": {
      const ratio = item.width && item.height ? item.width / item.height : 16 / 9;
      const height = full ? undefined : Math.min(260, Math.max(90, Math.round(326 / ratio)));
      return item.blob_state === "ready" ? (
        <Image
          source={{ uri: imageFile(item.id).uri }}
          style={[styles.image, { backgroundColor: t.chip, height, aspectRatio: full ? ratio : undefined }]}
          contentFit="contain"
          transition={120}
          accessibilityLabel={`${item.width ?? ""}×${item.height ?? ""} 이미지`}
        />
      ) : (
        <View style={[styles.image, { backgroundColor: t.chip, height: height ?? 160, alignItems: "center", justifyContent: "center" }]}>
          <Text style={{ color: t.cardMuted, fontSize: 13 }}>{item.blob_state === "failed" ? "이미지를 받지 못했어요" : "받는 중…"}</Text>
        </View>
      );
    }
    case "files": {
      const files = item.files ?? [];
      const shown = full ? files : files.slice(0, 3);
      return (
        <View style={styles.files}>
          {shown.map((f, i) => (
            <View key={`${f.name}-${i}`} style={styles.fileRow}>
              <View style={[styles.fileIcon, { backgroundColor: t.chip }]}>
                <Text style={{ color: t.cardMuted, fontSize: 11, fontWeight: "700" }}>{f.name.split(".").pop()?.slice(0, 4).toUpperCase() || "FILE"}</Text>
              </View>
              <View style={{ flex: 1, minWidth: 0 }}>
                <Text style={[styles.fileName, { color: t.cardFg }]} numberOfLines={1}>
                  {f.name}
                </Text>
                <Text style={{ color: t.cardMuted, fontSize: 12 }}>{bytes(f.size)}</Text>
              </View>
            </View>
          ))}
          {files.length > shown.length ? <Text style={{ color: t.cardMuted, fontSize: 12, paddingLeft: 52 }}>+{files.length - shown.length}개 더</Text> : null}
        </View>
      );
    }
    default:
      return null;
  }
}

function badge(item: Item): string {
  switch (item.kind) {
    case "text":
      return `${[...(item.text ?? "")].length}자`;
    case "link":
      return "링크";
    case "image":
      return item.width && item.height ? `${item.width}×${item.height}` : item.blob_size ? bytes(item.blob_size) : "이미지";
    case "files":
      return `파일 ${item.files?.length ?? 0}개`;
    default:
      return item.kind;
  }
}

const styles = StyleSheet.create({
  card: {
    borderRadius: radius.card,
    overflow: "hidden",
    shadowColor: "#000",
    shadowOpacity: 0.08,
    shadowRadius: 2,
    shadowOffset: { width: 0, height: 1 },
    elevation: 1,
  },
  head: { height: 44, paddingTop: 7, paddingHorizontal: 14, position: "relative" },
  kind: { color: "#fff", fontSize: 15, fontWeight: "600", lineHeight: 18 },
  time: { color: "rgba(255,255,255,0.9)", fontSize: 12, lineHeight: 15 },
  iconRing: { position: "absolute", top: 2, right: 2, width: 44, height: 44, borderRadius: 22, alignItems: "center", justifyContent: "center" },
  icon: { width: 34, height: 34, borderRadius: 9, alignItems: "center", justifyContent: "center" },
  iconText: { color: "#fff", fontSize: 15, fontWeight: "700" },
  text: { paddingHorizontal: 14, paddingTop: 12, fontSize: 14, lineHeight: 21 },
  link: { flexDirection: "row", gap: 12, paddingHorizontal: 14, paddingTop: 12 },
  linkIcon: { width: 44, height: 44, borderRadius: 10, alignItems: "center", justifyContent: "center" },
  domain: { fontSize: 11, lineHeight: 16 },
  linkText: { fontSize: 14, fontWeight: "600", lineHeight: 20, marginTop: 2 },
  image: { marginHorizontal: 14, marginTop: 12, borderRadius: 10, width: undefined },
  files: { paddingHorizontal: 14, paddingTop: 12, gap: 8 },
  fileRow: { flexDirection: "row", alignItems: "center", gap: 12 },
  fileIcon: { width: 40, height: 40, borderRadius: 10, alignItems: "center", justifyContent: "center" },
  fileName: { fontSize: 14, fontWeight: "600" },
  foot: { flexDirection: "row", alignItems: "center", gap: 10, paddingHorizontal: 12, paddingTop: 8, paddingBottom: 10 },
  origin: { flex: 1, fontSize: 13, fontWeight: "600" },
  badge: { height: 22, lineHeight: 22, paddingHorizontal: 7, borderRadius: radius.chip, fontSize: 13, fontWeight: "600" },
});
