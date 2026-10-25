import { create } from "zustand";

interface ConnectionEpochs {
  epochs: Record<string, number>;
  /** Пользователь подключил или переподключил подключение `id`. */
  bump: (id: string) => void;
}

/**
 * Номер «подключено по действию пользователя» для каждого подключения (подключить, переподключить). Растёт в
 * `refreshAfterConnect`; зависящее от него (подписка вкладки на сэмплер) перезапускается так же, как перечитываются
 * отчёты: переподключение снимает остановку после отказа во входе, а сам пароль автоматически не перебирается.
 */
export const useConnectionEpochs = create<ConnectionEpochs>((set) => ({
  epochs: {},
  bump: (id) => set((s) => ({ epochs: { ...s.epochs, [id]: (s.epochs[id] ?? 0) + 1 } })),
}));

/** Номер подключения `id` (0, пока пользователь его не переподключал). */
export const useConnectionEpoch = (id: string): number => useConnectionEpochs((s) => s.epochs[id] ?? 0);
