// The history: every device's clipboard, newest first. Tap copies to this
// phone; long press opens the item.

import { useShareIntentContext } from "expo-share-intent";
import { Redirect, useRouter } from "expo-router";
import * as Haptics from "expo-haptics";
import React, { useCallback, useEffect, useState } from "react";
import { ActivityIndicator, FlatList, Image, Pressable, RefreshControl, StyleSheet, Text, TextInput, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useApp, useItems } from "@/app-context";
import { copyItem } from "@/clipboard";
import type { Item } from "@/store/db";
import { ItemCard } from "@/ui/ItemCard";
import { useTheme } from "@/ui/theme";
import { useToast } from "@/ui/Toast";

export default function History() {
  const { ready, session, engine, status, updateReady } = useApp();
  const t = useTheme();
  const router = useRouter();
  const toast = useToast();
  const insets = useSafeAreaInsets();
  const [search, setSearch] = useState("");
  const { items, refresh } = useItems(search);
  const [refreshing, setRefreshing] = useState(false);
  const share = useShareIntentContext();

  useEffect(() => {
    if (session && share.hasShareIntent) router.push("/upload?source=share");
  }, [session, share.hasShareIntent, router]);

  useEffect(() => {
    if (updateReady) toast("새 버전을 받았어요. 다시 열면 적용돼요");
  }, [updateReady, toast]);

  const copy = useCallback(
    async (item: Item) => {
      if (!engine) return;
      try {
        const result = await copyItem(engine, item);
        if (result === "copied") {
          void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
          toast("복사됨");
        } else if (result === "pending") {
          toast("아직 받는 중이에요");
        }
      } catch (e) {
        toast(e instanceof Error ? e.message : "복사하지 못했어요");
      }
    },
    [engine, toast],
  );

  const onRefresh = useCallback(() => {
    setRefreshing(true);
    engine?.poke();
    refresh();
    setTimeout(() => setRefreshing(false), 600);
  }, [engine, refresh]);

  if (!ready) {
    return (
      <View style={[styles.center, { backgroundColor: t.bg }]}>
        <ActivityIndicator />
      </View>
    );
  }
  if (!session) return <Redirect href="/join" />;

  const statusText =
    status.state === "live"
      ? "동기화됨"
      : status.state === "syncing"
        ? status.initial
          ? "처음 받는 중…"
          : "동기화 중…"
        : status.state === "offline"
          ? "오프라인"
          : status.state === "unauthorized"
            ? "연결 해제됨"
            : "대기";
  const dot = status.state === "live" ? t.ok : status.state === "offline" || status.state === "unauthorized" ? t.danger : t.muted;

  return (
    <View style={[styles.screen, { backgroundColor: t.bg, paddingTop: insets.top + 6 }]}>
      <View style={styles.header}>
        <View style={styles.brand}>
          <Image source={require("../assets/icon-small.png")} style={{ width: 30, height: 30 }} accessibilityIgnoresInvertColors />
          <Text style={[styles.title, { color: t.fg }]}>Pinchy</Text>
        </View>
        <View style={styles.headerRight}>
          <Pressable onPress={() => router.push("/settings")} style={[styles.status, { backgroundColor: t.chip }]} accessibilityRole="button" accessibilityLabel={`상태 ${statusText}`}>
            <View style={[styles.dot, { backgroundColor: dot }]} />
            <Text style={[styles.statusText, { color: t.muted }]}>{statusText}</Text>
          </Pressable>
          <Pressable onPress={() => router.push("/settings")} style={[styles.gear, { backgroundColor: t.chip }]} accessibilityRole="button" accessibilityLabel="설정">
            <Text style={{ color: t.fg, fontSize: 18 }}>⚙︎</Text>
          </Pressable>
        </View>
      </View>
      <View style={[styles.search, { backgroundColor: t.chip }]}>
        <Text style={{ color: t.muted, fontSize: 15 }}>⌕</Text>
        <TextInput
          value={search}
          onChangeText={setSearch}
          placeholder="검색"
          placeholderTextColor={t.muted}
          style={[styles.searchInput, { color: t.fg }]}
          autoCorrect={false}
          clearButtonMode="while-editing"
          accessibilityLabel="히스토리 검색"
        />
      </View>
      <FlatList
        data={items}
        keyExtractor={(item) => item.id}
        renderItem={({ item }) => <ItemCard item={item} onPress={() => void copy(item)} onLongPress={() => router.push(`/item/${item.id}`)} />}
        contentContainerStyle={[styles.list, { paddingBottom: insets.bottom + 120 }]}
        ItemSeparatorComponent={() => <View style={{ height: 12 }} />}
        refreshControl={<RefreshControl refreshing={refreshing} onRefresh={onRefresh} tintColor={t.muted} />}
        keyboardDismissMode="on-drag"
        ListEmptyComponent={
          <View style={styles.empty}>
            <Text style={{ color: t.muted, fontSize: 15, textAlign: "center", lineHeight: 22 }}>
              {search
                ? "검색 결과가 없어요"
                : status.state === "syncing"
                  ? "히스토리를 받고 있어요"
                  : "아직 비어 있어요.\n컴퓨터에서 무언가 복사하면 여기 나타나요."}
            </Text>
          </View>
        }
      />
      <View pointerEvents="box-none" style={[styles.fabWrap, { bottom: insets.bottom + 16 }]}>
        <Pressable
          onPress={() => router.push("/add")}
          style={({ pressed }) => [styles.fab, { backgroundColor: t.accentStrong, opacity: pressed ? 0.85 : 1 }]}
          accessibilityRole="button"
          accessibilityLabel="추가"
        >
          <Text style={styles.fabPlus}>+</Text>
          <Text style={styles.fabText}>추가</Text>
        </Pressable>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  center: { flex: 1, alignItems: "center", justifyContent: "center" },
  header: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingHorizontal: 20 },
  brand: { flexDirection: "row", alignItems: "center", gap: 8 },
  title: { fontSize: 22, fontWeight: "800", letterSpacing: -0.6 },
  headerRight: { flexDirection: "row", alignItems: "center", gap: 8 },
  status: { flexDirection: "row", alignItems: "center", gap: 6, height: 26, paddingHorizontal: 10, borderRadius: 13 },
  dot: { width: 7, height: 7, borderRadius: 4 },
  statusText: { fontSize: 12, fontWeight: "600" },
  gear: { width: 36, height: 36, borderRadius: 18, alignItems: "center", justifyContent: "center" },
  search: { flexDirection: "row", alignItems: "center", gap: 8, height: 38, marginHorizontal: 20, marginTop: 12, paddingHorizontal: 12, borderRadius: 12 },
  searchInput: { flex: 1, fontSize: 15, paddingVertical: 0 },
  list: { paddingHorizontal: 20, paddingTop: 14 },
  empty: { paddingTop: 120, paddingHorizontal: 30 },
  fabWrap: { position: "absolute", left: 20, right: 20, alignItems: "center" },
  fab: { height: 52, paddingHorizontal: 22, borderRadius: 26, flexDirection: "row", alignItems: "center", justifyContent: "center", gap: 6, shadowColor: "#d8432b", shadowOpacity: 0.35, shadowRadius: 22, shadowOffset: { width: 0, height: 8 }, elevation: 6 },
  fabPlus: { color: "#fff", fontSize: 22, fontWeight: "700", lineHeight: 24, marginTop: -2 },
  fabText: { color: "#fff", fontSize: 16, fontWeight: "700" },
});
