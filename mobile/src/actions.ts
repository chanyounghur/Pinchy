// What tapping a card or its button does, shared by the history and the
// detail screen: copy text and images, save photo bundles to the album,
// hand other files to the share sheet.

import * as Haptics from "expo-haptics";
import { useCallback } from "react";
import { useApp } from "./app-context";
import { copyItem, savePictures } from "./clipboard";
import type { Item } from "./store/db";
import { cardAction } from "./ui/ItemCard";
import { useToast } from "./ui/Toast";

export function useItemActions(): { copy(item: Item): Promise<void>; act(item: Item): Promise<void> } {
  const { engine } = useApp();
  const toast = useToast();

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

  const act = useCallback(
    async (item: Item) => {
      if (!engine) return;
      if (cardAction(item) !== "save") return copy(item);
      try {
        if (item.blob_state !== "ready" && !(await engine.ensureBlob(item.id))) {
          toast("아직 받는 중이에요");
          return;
        }
        const n = await savePictures(item);
        void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
        toast(n > 1 ? `사진 ${n}장을 앨범에 저장했어요` : "앨범에 저장했어요");
      } catch (e) {
        toast(e instanceof Error ? e.message : "저장하지 못했어요");
      }
    },
    [engine, copy, toast],
  );

  return { copy, act };
}
