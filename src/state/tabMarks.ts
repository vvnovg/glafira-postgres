import { create } from "zustand";

/**
 * Пометки вкладок (спец. P4 §8.4): консоль EXPLAIN помечает свою вкладку, пока в фоне у неё есть предложение LLM, которого
 * она ещё не показала; строка вкладок рисует точку. Ключ — id вкладки (`tabId`); снятая пометка удаляется из объекта.
 */
interface TabMarksState {
  marks: Record<string, true>;
  setMark: (tabId: string, on: boolean) => void;
}

export const useTabMarks = create<TabMarksState>((set) => ({
  marks: {},
  setMark: (tabId, on) =>
    set((s) => {
      if (Boolean(s.marks[tabId]) === on) return s;
      const marks = { ...s.marks };
      if (on) marks[tabId] = true;
      else delete marks[tabId];
      return { marks };
    }),
}));
