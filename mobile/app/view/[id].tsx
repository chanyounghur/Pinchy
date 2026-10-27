// Pictures full screen: pinch to zoom, swipe between the photos of a
// bundle, drag down to close. Save, share and copy at the bottom.

import Ionicons from "@expo/vector-icons/Ionicons";
import { Image } from "expo-image";
import { useLocalSearchParams, useRouter } from "expo-router";
import * as Haptics from "expo-haptics";
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ActivityIndicator, FlatList, Pressable, StyleSheet, Text, useWindowDimensions, View } from "react-native";
import { Gesture, GestureDetector, GestureHandlerRootView } from "react-native-gesture-handler";
import Animated, { runOnJS, useAnimatedStyle, useSharedValue, withSpring, withTiming } from "react-native-reanimated";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useApp, useItem } from "@/app-context";
import { copyPicture, imageUris, savePictures, sharePicture } from "@/clipboard";

const MAX_SCALE = 4;
const DISMISS_DRAG = 120;

export default function Viewer() {
  const { id, index } = useLocalSearchParams<{ id: string; index?: string }>();
  const item = useItem(id ?? "");
  const { engine } = useApp();
  const router = useRouter();
  const insets = useSafeAreaInsets();
  const { width, height } = useWindowDimensions();
  const [current, setCurrent] = useState(Math.max(0, Number(index) || 0));
  const [zoomed, setZoomed] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState<"save" | "share" | "copy" | null>(null);
  const noteTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const ready = item?.blob_state === "ready";
  const uris = useMemo(() => (item && ready ? imageUris(item) : []), [item, ready]);

  useEffect(() => {
    if (item && item.blob_state === "missing" && engine) void engine.ensureBlob(item.id);
  }, [item, engine]);
  useEffect(() => () => {
    if (noteTimer.current) clearTimeout(noteTimer.current);
  }, []);

  const say = useCallback((text: string) => {
    setNote(text);
    if (noteTimer.current) clearTimeout(noteTimer.current);
    noteTimer.current = setTimeout(() => setNote(null), 1600);
  }, []);

  const close = useCallback(() => router.back(), [router]);

  const run = async (which: "save" | "share" | "copy") => {
    if (!item || !ready) return;
    setBusy(which);
    try {
      if (which === "save") {
        await savePictures(item, current);
        void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
        say("앨범에 저장했어요");
      } else if (which === "copy") {
        await copyPicture(item, current);
        void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
        say("복사됨");
      } else {
        await sharePicture(item, current);
      }
    } catch (e) {
      say(e instanceof Error ? e.message : "하지 못했어요");
    } finally {
      setBusy(null);
    }
  };

  const total = uris.length;

  return (
    <GestureHandlerRootView style={styles.screen}>
      {total === 0 ? (
        <View style={styles.center}>
          {item === null ? (
            <Text style={styles.muted}>이 항목은 삭제됐어요.</Text>
          ) : item?.blob_state === "failed" ? (
            <Text style={styles.muted}>사진을 받지 못했어요</Text>
          ) : (
            <ActivityIndicator color="#fff" />
          )}
        </View>
      ) : (
        <FlatList
          data={uris}
          keyExtractor={(uri, i) => `${i}-${uri}`}
          horizontal
          pagingEnabled
          scrollEnabled={!zoomed}
          showsHorizontalScrollIndicator={false}
          initialScrollIndex={Math.min(current, total - 1)}
          getItemLayout={(_, i) => ({ length: width, offset: width * i, index: i })}
          onMomentumScrollEnd={(e) => setCurrent(Math.round(e.nativeEvent.contentOffset.x / width))}
          renderItem={({ item: uri }) => <Zoomable uri={uri} width={width} height={height} onZoom={setZoomed} onDismiss={close} />}
        />
      )}

      <View style={[styles.top, { paddingTop: insets.top + 8 }]} pointerEvents="box-none">
        <Pressable onPress={close} accessibilityRole="button" accessibilityLabel="닫기" hitSlop={8} style={({ pressed }) => [styles.round, { opacity: pressed ? 0.6 : 1 }]}>
          <Text style={{ color: "#fff", fontSize: 18, fontWeight: "700", lineHeight: 20 }}>✕</Text>
        </Pressable>
        {total > 1 ? (
          <Text style={styles.counter}>
            {Math.min(current, total - 1) + 1} / {total}
          </Text>
        ) : null}
        <View style={{ width: 36 }} />
      </View>

      <View style={[styles.bottom, { paddingBottom: insets.bottom + 22 }]} pointerEvents="box-none">
        {note ? <Text style={styles.note}>{note}</Text> : null}
        {total > 1 && total <= 12 ? (
          <View style={styles.dots}>
            {uris.map((_, i) => (
              <View key={i} style={[styles.dot, { opacity: i === current ? 1 : 0.4 }]} />
            ))}
          </View>
        ) : null}
        <View style={styles.actions}>
          <Action icon="download-outline" title="저장" onPress={() => void run("save")} busy={busy === "save"} disabled={!ready || busy !== null} />
          <Action icon="copy-outline" title="복사" onPress={() => void run("copy")} busy={busy === "copy"} disabled={!ready || busy !== null} />
          <Action icon="share-outline" title="공유" onPress={() => void run("share")} busy={busy === "share"} disabled={!ready || busy !== null} />
        </View>
      </View>
    </GestureHandlerRootView>
  );
}

function Action({ icon, title, onPress, busy, disabled }: { icon: React.ComponentProps<typeof Ionicons>["name"]; title: string; onPress: () => void; busy: boolean; disabled: boolean }) {
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled}
      accessibilityRole="button"
      accessibilityLabel={title}
      style={({ pressed }) => [styles.button, { opacity: pressed ? 0.7 : disabled && !busy ? 0.5 : 1 }]}
    >
      {busy ? (
        <ActivityIndicator color="#fff" />
      ) : (
        <>
          <Ionicons name={icon} size={18} color="#fff" />
          <Text style={styles.buttonText}>{title}</Text>
        </>
      )}
    </Pressable>
  );
}

function Zoomable({ uri, width, height, onZoom, onDismiss }: { uri: string; width: number; height: number; onZoom: (zoomed: boolean) => void; onDismiss: () => void }) {
  const scale = useSharedValue(1);
  const savedScale = useSharedValue(1);
  const tx = useSharedValue(0);
  const ty = useSharedValue(0);
  const savedTx = useSharedValue(0);
  const savedTy = useSharedValue(0);
  const drop = useSharedValue(0);
  const [zoomed, setZoomedLocal] = useState(false);

  const setZoomed = useCallback(
    (z: boolean) => {
      setZoomedLocal(z);
      onZoom(z);
    },
    [onZoom],
  );

  const gesture = useMemo(() => {
    const clampTranslation = (s: number) => {
      "worklet";
      const maxX = (width * (s - 1)) / 2;
      const maxY = (height * (s - 1)) / 2;
      tx.value = Math.min(maxX, Math.max(-maxX, tx.value));
      ty.value = Math.min(maxY, Math.max(-maxY, ty.value));
    };
    const settle = () => {
      "worklet";
      if (scale.value < 1.02) {
        scale.value = withSpring(1);
        tx.value = withSpring(0);
        ty.value = withSpring(0);
        savedScale.value = 1;
        savedTx.value = 0;
        savedTy.value = 0;
        runOnJS(setZoomed)(false);
      } else {
        if (scale.value > MAX_SCALE) scale.value = withSpring(MAX_SCALE);
        clampTranslation(Math.min(scale.value, MAX_SCALE));
        savedScale.value = Math.min(scale.value, MAX_SCALE);
        savedTx.value = tx.value;
        savedTy.value = ty.value;
        runOnJS(setZoomed)(true);
      }
    };

    const pinch = Gesture.Pinch()
      .onUpdate((e) => {
        scale.value = savedScale.value * e.scale;
      })
      .onEnd(() => {
        settle();
      });

    const panZoomed = Gesture.Pan()
      .enabled(zoomed)
      .onUpdate((e) => {
        tx.value = savedTx.value + e.translationX;
        ty.value = savedTy.value + e.translationY;
      })
      .onEnd(() => {
        settle();
      });

    const dragDown = Gesture.Pan()
      .enabled(!zoomed)
      .activeOffsetY([-16, 16])
      .failOffsetX([-16, 16])
      .onUpdate((e) => {
        drop.value = e.translationY;
      })
      .onEnd(() => {
        if (Math.abs(drop.value) > DISMISS_DRAG) {
          drop.value = withTiming(drop.value > 0 ? height : -height, { duration: 160 });
          runOnJS(onDismiss)();
        } else {
          drop.value = withSpring(0);
        }
      });

    const doubleTap = Gesture.Tap()
      .numberOfTaps(2)
      .onEnd(() => {
        if (scale.value > 1.02) {
          scale.value = withSpring(1);
          tx.value = withSpring(0);
          ty.value = withSpring(0);
          savedScale.value = 1;
          savedTx.value = 0;
          savedTy.value = 0;
          runOnJS(setZoomed)(false);
        } else {
          scale.value = withSpring(2.5);
          savedScale.value = 2.5;
          runOnJS(setZoomed)(true);
        }
      });

    return Gesture.Simultaneous(pinch, panZoomed, dragDown, doubleTap);
  }, [zoomed, width, height, onDismiss, setZoomed, scale, savedScale, tx, ty, savedTx, savedTy, drop]);

  const style = useAnimatedStyle(() => ({
    transform: [{ translateX: tx.value }, { translateY: ty.value + drop.value }, { scale: scale.value }],
    opacity: 1 - Math.min(0.6, Math.abs(drop.value) / (height * 0.8)),
  }));

  return (
    <GestureDetector gesture={gesture}>
      <Animated.View style={[{ width, height, alignItems: "center", justifyContent: "center" }, style]}>
        <Image source={{ uri }} style={{ width, height }} contentFit="contain" transition={120} accessibilityLabel="사진" />
      </Animated.View>
    </GestureDetector>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: "#000" },
  center: { flex: 1, alignItems: "center", justifyContent: "center" },
  muted: { color: "rgba(255,255,255,0.7)", fontSize: 15 },
  top: { position: "absolute", left: 0, right: 0, top: 0, flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingHorizontal: 16 },
  round: { width: 36, height: 36, borderRadius: 18, backgroundColor: "rgba(255,255,255,0.16)", alignItems: "center", justifyContent: "center" },
  counter: { color: "#fff", fontSize: 15, fontWeight: "600" },
  bottom: { position: "absolute", left: 0, right: 0, bottom: 0, paddingHorizontal: 20, gap: 14, alignItems: "center" },
  note: { color: "#fff", fontSize: 14, fontWeight: "600", backgroundColor: "rgba(0,0,0,0.6)", paddingHorizontal: 14, paddingVertical: 8, borderRadius: 16, overflow: "hidden" },
  dots: { flexDirection: "row", gap: 6 },
  dot: { width: 6, height: 6, borderRadius: 3, backgroundColor: "#fff" },
  actions: { flexDirection: "row", gap: 10, alignSelf: "stretch" },
  button: { flex: 1, height: 44, borderRadius: 22, flexDirection: "row", gap: 6, alignItems: "center", justifyContent: "center", backgroundColor: "rgba(255,255,255,0.16)" },
  buttonText: { color: "#fff", fontSize: 15, fontWeight: "700" },
});
