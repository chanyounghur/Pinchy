// One history item as the desktop draws it: the source app's color on top
// with the kind and time, the content, and where it came from at the bottom.
// The header names the kind and the device; the bottom row holds save, copy
// and share buttons, then the time and a badge. Pictures open the viewer.

import Ionicons from "@expo/vector-icons/Ionicons";
import { Image } from "expo-image";
import React, { useEffect } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { useApp } from "../app-context";
import { fileFor, imageFile } from "../blobs";
import type { Item } from "../store/db";
import { appColor, bytes, domainOf, initial, isPhotoBundle, kindLabel, relativeTime } from "./format";
import { radius, useTheme, type Theme } from "./theme";

const GRID_MAX = 6;

export interface CardActions {
  onSave?: () => void;
  onCopy?: () => void;
  onShare?: () => void;
}

export function ItemCard({
  item,
  onPress,
  onLongPress,
  onOpenPicture,
  actions,
  full = false,
}: {
  item: Item;
  onPress?: () => void;
  onLongPress?: () => void;
  /// Opens the viewer at that picture. Picture cards use it for the card tap too.
  onOpenPicture?: (index: number) => void;
  actions?: CardActions;
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
  const copyable = item.kind !== "files" || photos;
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
        <Text style={styles.kind} numberOfLines={1}>
          {kindLabel(item.kind, item.files)}
          <Text style={styles.dot}> · </Text>
          <Text style={styles.from}>{item.from}</Text>
        </Text>
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
        {actions ? (
          <>
            <IconButton name="download-outline" label="저장" onPress={actions.onSave} t={t} />
            <IconButton name="copy-outline" label="복사" onPress={actions.onCopy} disabled={!copyable} t={t} />
            <IconButton name="share-outline" label="공유" onPress={actions.onShare} t={t} />
          </>
        ) : null}
        <View style={{ flex: 1 }} />
        <Text style={[styles.time, { color: t.cardMuted }]}>{relativeTime(item.copied_at)}</Text>
        <Text style={[styles.badge, { backgroundColor: t.chip, color: t.cardMuted }]}>{badge(item, photos)}</Text>
      </View>
    </Pressable>
  );
}

function IconButton({ name, label, onPress, disabled = false, t }: { name: React.ComponentProps<typeof Ionicons>["name"]; label: string; onPress?: () => void; disabled?: boolean; t: Theme }) {
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled || !onPress}
      accessibilityRole="button"
      accessibilityLabel={label}
      hitSlop={4}
      style={({ pressed }) => [styles.iconButton, { backgroundColor: t.chip, opacity: disabled ? 0.35 : pressed ? 0.6 : 1 }]}
    >
      <Ionicons name={name} size={17} color={t.origin} />
    </Pressable>
  );
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
  dot: { color: "rgba(255,255,255,0.7)", fontWeight: "400" },
  from: { color: "rgba(255,255,255,0.92)", fontSize: 13, fontWeight: "600" },
  time: { fontSize: 12 },
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
  foot: { flexDirection: "row", alignItems: "center", gap: 6, paddingHorizontal: 12, paddingTop: 8, paddingBottom: 10 },
  iconButton: { width: 34, height: 34, borderRadius: 17, alignItems: "center", justifyContent: "center" },
  badge: { height: 22, lineHeight: 22, paddingHorizontal: 7, borderRadius: radius.chip, fontSize: 13, fontWeight: "600" },
});
