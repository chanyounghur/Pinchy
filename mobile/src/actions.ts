// What a card's buttons do, shared by the history and the detail screen:
// save (album, folder or share sheet), copy to the clipboard, and share.
// Each one fetches the blob first when it is not here yet.

import * as Haptics from "expo-haptics";
import { useCallback } from "react";
import { useApp } from "./app-context";
import { copyAny, saveItem, shareItem } from "./clipboard";
import type { Item } from "./store/db";
import { useToast } from "./ui/Toast";

export interface ItemActions {
  copy(item: Item): Promise<void>;
  save(item: Item): Promise<void>;
  share(item: Item): Promise<void>;
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
        toast("복사됨");
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

  return { copy, save, share };
}
