import { create } from "zustand";

/**
 * Запрос «Собрать диагностику…» извне экрана «О программе» (меню Help на macOS).
 * Экран сам забирает запрос (consume) и запускает тот же сбор, что и его кнопка.
 */
interface DiagnosticsRequestState {
  pending: boolean;
  request: () => void;
  /** Снимает запрос; true — запрос был. */
  consume: () => boolean;
}

export const useDiagnosticsRequest = create<DiagnosticsRequestState>((set, get) => ({
  pending: false,
  request: () => set({ pending: true }),
  consume: () => {
    if (!get().pending) return false;
    set({ pending: false });
    return true;
  },
}));
