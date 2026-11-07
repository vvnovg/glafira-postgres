import { create } from "zustand";

/**
 * Предложения LLM, которые панель «Предложения LLM» уже показала (спец. P4c §8.1): строка статуса фонового ассистента
 * пишет «Ассистент прислал N предложений», пока панель их не показала. Ключ — id предложения; только в памяти.
 */
interface ShownProposalsState {
  shown: Record<string, true>;
  markShown: (ids: readonly string[]) => void;
}

export const useShownProposals = create<ShownProposalsState>((set) => ({
  shown: {},
  markShown: (ids) =>
    set((s) => {
      const fresh = ids.filter((id) => !s.shown[id]);
      if (fresh.length === 0) return s;
      const shown = { ...s.shown };
      for (const id of fresh) shown[id] = true;
      return { shown };
    }),
}));
