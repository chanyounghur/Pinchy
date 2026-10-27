// One history item as the desktop draws it: the source app's color on top
// with the kind and time, the content, and where it came from at the bottom.
// Pictures open the viewer on tap; image and file cards carry one action
// button (copy, save or share) at the bottom left.

import { Image } from "expo-image";
import React, { useEffect } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { useApp } from "../app-context";
import { fileFor, imageFile } from "../blobs";
import type { Item } from "../store/db";
import { appColor, bytes, domainOf, initial, isPhotoBundle, kindLabel, relativeTime } from "./format";
import { radius, useTheme, type Theme } from "./theme";

const GRID_MAX = 6;

/// What the bottom-left button does for this item, if it has one.
export function cardAction(item: Item): "copy" | "save" | "share" | null {
  if (item.kind === "image") return "copy";
  if (isPhotoBundle(item.kind, item.files)) return "save";
  if (item.kind === "files") return "share";
  return null;
}

export function ItemCard({
  item,
  onPress,
  onLongPress,
  onOpenPicture,
  onAction,
  full = false,
}: {
  item: Item;
  onPress?: () => void;
  onLongPress?: () => void;
  /// Opens the viewer at that picture. Picture cards use it for the card tap too.
  onOpenPicture?: (index: number) => void;
  onAction?: () => void;
  full?: boolean;
}) {
  const t = useTheme();
  const { engine } = useApp();
  const needsBlob = (item.kind === "image" || item.kind === "files") && item.blob_state === "missing";
  useEffect(() => {
    if (needsBlob && engine) void engine.ensureBlob(item.id);
  }, [needsBlob, engine, item.id]);
  const color = item.color ?? appColor(item.app);
  const photos = isPhotoBundle(item.kind, item.files);
  const pictures = item.kind === "image" || photos;
  const action = cardAction(item);
  const press = pictures && onOpenPicture ? () => onOpenPicture(0) : onPress;
  return (
    <Pressable
      onPress={press}
      onLongPress={onLongPress}
      accessibilityRole="button"
      accessibilityLabel={`${kindLabel(item.kind, item.files)}, ${item.from}`}
      style={({ pressed }) => [styles.card, { backgroundColor: t.card, opacity: pressed ? 0.85 : 1 }]}
    >
      <View style={[styles.head, { backgroundColor: color }]}>
        <Text style={styles.kind}>{kindLabel(item.kind, item.files)}</Text>
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
      <Body item={item} t={t} full={full} photos={photos} onOpenPicture={onOpenPicture} />
      <View style={styles.foot}>
        {action && onAction ? (
          <Pressable
            onPress={onAction}
            accessibilityRole="button"
            accessibilityLabel={actionLabel(action)}
            hitSlop={6}
            style={({ pressed }) => [styles.action, { backgroundColor: t.dark ? "rgba(255,102,82,0.2)" : "rgba(216,67,43,0.12)", opacity: pressed ? 0.6 : 1 }]}
          >
            <Text style={[styles.actionText, { color: t.origin }]}>{actionLabel(action)}</Text>
          </Pressable>
        ) : null}
        <Text style={[styles.origin, { color: t.origin }]} numberOfLines={1}>
          {item.from}
        </Text>
        <Text style={[styles.badge, { backgroundColor: t.chip, color: t.cardMuted }]}>{badge(item, photos)}</Text>
      </View>
    </Pressable>
  );
}

function actionLabel(action: "copy" | "save" | "share"): string {
  return action === "copy" ? "⧉ 복사" : action === "save" ? "↓ 저장" : "↗ 공유";
}

function Body({ item, t, full, photos, onOpenPicture }: { item: Item; t: Theme; full: boolean; photos: boolean; onOpenPicture?: (index: number) => void }) {
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
      if (photos) {
        const shown = full ? files : files.slice(0, GRID_MAX);
        const more = files.length - shown.length;
        const ready = item.blob_state === "ready";
        return (
          <View style={styles.grid}>
            {shown.map((f, i) => {
              const last = !full && more > 0 && i === shown.length - 1;
              return (
                <Pressable
                  key={`${f.name}-${i}`}
                  onPress={onOpenPicture ? () => onOpenPicture(i) : undefined}
                  disabled={!onOpenPicture}
                  accessibilityRole="imagebutton"
                  accessibilityLabel={`사진 ${i + 1}`}
                  style={[styles.tile, { backgroundColor: t.chip }]}
                >
                  {ready ? <Image source={{ uri: fileFor(item.id, files, i).uri }} style={StyleSheet.absoluteFill} contentFit="cover" transition={120} /> : null}
                  {last ? (
                    <View style={styles.more}>
                      <Text style={styles.moreText}>+{more}</Text>
                    </View>
                  ) : null}
                </Pressable>
              );
            })}
            {!ready ? (
              <Text style={{ color: t.cardMuted, fontSize: 12, width: "100%" }}>{item.blob_state === "failed" ? "사진을 받지 못했어요" : "받는 중…"}</Text>
            ) : null}
          </View>
        );
      }
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

function badge(item: Item, photos: boolean): string {
  switch (item.kind) {
    case "text":
      return `${[...(item.text ?? "")].length}자`;
    case "link":
      return "링크";
    case "image":
      return item.width && item.height ? `${item.width}×${item.height}` : item.blob_size ? bytes(item.blob_size) : "이미지";
    case "files": {
      const n = item.files?.length ?? 0;
      if (photos) {
        const size = item.files?.reduce((sum, f) => sum + f.size, 0) ?? 0;
        return `사진 ${n}장 · ${bytes(size)}`;
      }
      return `파일 ${n}개`;
    }
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
  head: { height: 32, paddingHorizontal: 14, flexDirection: "row", alignItems: "center", gap: 8 },
  kind: { flex: 1, color: "#fff", fontSize: 14, fontWeight: "700", lineHeight: 18 },
  time: { color: "rgba(255,255,255,0.85)", fontSize: 12, lineHeight: 15 },
  iconRing: { width: 24, height: 24, borderRadius: 12, alignItems: "center", justifyContent: "center", marginRight: -6 },
  icon: { width: 18, height: 18, borderRadius: 5, alignItems: "center", justifyContent: "center" },
  iconText: { color: "#fff", fontSize: 10, fontWeight: "800" },
  text: { paddingHorizontal: 14, paddingTop: 12, fontSize: 14, lineHeight: 21 },
  link: { flexDirection: "row", gap: 12, paddingHorizontal: 14, paddingTop: 12 },
  linkIcon: { width: 44, height: 44, borderRadius: 10, alignItems: "center", justifyContent: "center" },
  domain: { fontSize: 11, lineHeight: 16 },
  linkText: { fontSize: 14, fontWeight: "600", lineHeight: 20, marginTop: 2 },
  image: { marginHorizontal: 14, marginTop: 12, borderRadius: 10, width: undefined },
  grid: { flexDirection: "row", flexWrap: "wrap", gap: 4, paddingHorizontal: 14, paddingTop: 12 },
  tile: { width: "32.3%", aspectRatio: 1, borderRadius: 8, overflow: "hidden" },
  more: { position: "absolute", left: 0, top: 0, right: 0, bottom: 0, backgroundColor: "rgba(0,0,0,0.45)", alignItems: "center", justifyContent: "center" },
  moreText: { color: "#fff", fontSize: 18, fontWeight: "800" },
  files: { paddingHorizontal: 14, paddingTop: 12, gap: 8 },
  fileRow: { flexDirection: "row", alignItems: "center", gap: 12 },
  fileIcon: { width: 40, height: 40, borderRadius: 10, alignItems: "center", justifyContent: "center" },
  fileName: { fontSize: 14, fontWeight: "600" },
  foot: { flexDirection: "row", alignItems: "center", gap: 10, paddingHorizontal: 12, paddingTop: 10, paddingBottom: 10 },
  action: { height: 30, paddingHorizontal: 12, borderRadius: 15, alignItems: "center", justifyContent: "center" },
  actionText: { fontSize: 13, fontWeight: "700" },
  origin: { flex: 1, fontSize: 13, fontWeight: "600" },
  badge: { height: 22, lineHeight: 22, paddingHorizontal: 7, borderRadius: radius.chip, fontSize: 13, fontWeight: "600" },
});
