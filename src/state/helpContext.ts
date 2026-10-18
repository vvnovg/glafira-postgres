import { useEffect } from "react";
import { create } from "zustand";
import type { ChapterId } from "../guide/chapters";

/** Уточнение главы справки для вкладки (по id вкладки): F1 берёт его раньше таблицы helpChapterFor. */
interface HelpContextState {
  byTab: Record<string, ChapterId>;
}

export const useHelpContext = create<HelpContextState>(() => ({ byTab: {} }));

/** Пока компонент смонтирован, F1 на вкладке tabId открывает главу chapter. */
export function useHelpChapter(tabId: string, chapter: ChapterId): void {
  useEffect(() => {
    useHelpContext.setState((s) => ({ byTab: { ...s.byTab, [tabId]: chapter } }));
    return () =>
      useHelpContext.setState((s) => {
        const { [tabId]: _removed, ...rest } = s.byTab;
        return { byTab: rest };
      });
  }, [tabId, chapter]);
}
