import { useEffect } from "react";
import { create } from "zustand";
import type { ChapterId } from "../guide/chapters";

/** Уточнение главы справки для вкладки (по id вкладки): F1 берёт его раньше таблицы helpChapterFor. */
interface HelpContextState {
  byTab: Record<string, ChapterId>;
  /** Раздел уточнённой главы (якорь `{#…}`), если экран задал и его. */
  anchorByTab: Record<string, string>;
}

export const useHelpContext = create<HelpContextState>(() => ({ byTab: {}, anchorByTab: {} }));

/** Пока компонент смонтирован, F1 на вкладке tabId открывает главу chapter (и её раздел anchor, если задан). */
export function useHelpChapter(tabId: string, chapter: ChapterId, anchor?: string): void {
  useEffect(() => {
    useHelpContext.setState((s) => {
      const { [tabId]: _old, ...anchors } = s.anchorByTab;
      return { byTab: { ...s.byTab, [tabId]: chapter }, anchorByTab: anchor ? { ...anchors, [tabId]: anchor } : anchors };
    });
    return () =>
      useHelpContext.setState((s) => {
        const { [tabId]: _removed, ...rest } = s.byTab;
        const { [tabId]: _anchor, ...anchors } = s.anchorByTab;
        return { byTab: rest, anchorByTab: anchors };
      });
  }, [tabId, chapter, anchor]);
}
