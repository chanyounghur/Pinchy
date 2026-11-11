// What a card's buttons do, shared by the history and the detail screen:
// save (album, folder or share sheet), copy to the clipboard, share, open a
// link in the browser, and delete everywhere after asking.
// Each one that needs the blob fetches it first when it is not here yet.

import * as Haptics from "expo-haptics";
import { useCallback } from "react";
import { Alert, Linking } from "react-native";
import { useApp } from "./app-context";
import { copyAny, saveItem, shareItem } from "./clipboard";
import type { Item } from "./store/db";
import { useToast } from "./ui/Toast";

export interface ItemActions {
  copy(item: Item): Promise<void>;
  save(item: Item): Promise<void>;
  share(item: Item): Promise<void>;
  open(item: Item): Promise<void>;
  /// Asks first, then deletes the item on every device. `before` runs once
  /// the user agrees, before the delete starts (the detail screen goes back).
  remove(item: Item, before?: () => void): void;
}

export function useItemActions(): ItemActions {
  const { engine } = useApp();
  const toast = useToast();

  const withBlob = useCallback(
    async (item: Item): Promise<Item | null> => {
      if (item.kind !== "image" && item.kind !== "files") return item;
      if (item.blob_state === "ready") return item;
      if (!engine || !(await engine.ensureBlob(item.id))) {
        toast("아직 받는 중이에요");
        return null;
      }
      return { ...item, blob_state: "ready" };
    },
    [engine, toast],
  );

  const copy = useCallback(
    async (item: Item) => {
      try {
        const ready = await withBlob(item);
        if (!ready) return;
        await copyAny(ready);
        void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
        toast("복사했어요");
      } catch (e) {
        toast(e instanceof Error ? e.message : "복사하지 못했어요");
      }
    },
    [withBlob, toast],
  );

  const save = useCallback(
    async (item: Item) => {
      try {
        const ready = await withBlob(item);
        if (!ready) return;
        const note = await saveItem(ready);
        if (note) {
          void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
          toast(note);
        }
      } catch (e) {
        toast(e instanceof Error ? e.message : "저장하지 못했어요");
      }
    },
    [withBlob, toast],
  );

  const share = useCallback(
    async (item: Item) => {
      try {
        const ready = await withBlob(item);
        if (!ready) return;
        await shareItem(ready);
      } catch (e) {
        toast(e instanceof Error ? e.message : "공유하지 못했어요");
      }
    },
    [withBlob, toast],
  );

  const open = useCallback(
    async (item: Item) => {
      const url = item.text?.trim();
      if (!url) return;
      try {
        await Linking.openURL(url);
      } catch {
        toast("링크를 열지 못했어요");
      }
    },
    [toast],
  );

  const remove = useCallback(
    (item: Item, before?: () => void) => {
      if (!engine) return;
      Alert.alert("모든 기기에서 지울까요?", "연결된 모든 기기의 히스토리에서 사라지고, 되돌릴 수 없어요.", [
        { text: "닫기", style: "cancel" },
        {
          text: "지우기",
          style: "destructive",
          onPress: () => {
            before?.();
            engine.deleteItem(item.id).catch((e: unknown) => toast(e instanceof Error ? e.message : "지우지 못했어요"));
          },
        },
      ]);
    },
    [engine, toast],
  );

  return { copy, save, share, open, remove };
}
