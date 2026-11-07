import { create } from "zustand";

/**
 * Итоги фоновых запусков, закрытые у отвеченного запроса ссылкой «Закрыть запрос» (смоук P4c, R20). Строка статуса консоли и
 * карточки держит итог запуска («Ассистент прислал 2 предложения · 48 с · ≈ 0,007 $») до следующей отправки, до закрытия или до
 * подстановки другого текста. Отвеченный запрос Rust не закрывает (`cancel_optimization_request` меняет только ждущий), поэтому
 * закрытие помнит интерфейс: id запросов — в localStorage (`glafira.llm.closedResults`, последние `MAX_CLOSED`), закрытое
 * остаётся закрытым и после перезапуска. Хранилище недоступно — закрытие живёт до перезапуска; испорченное значение или другая
 * версия — как «ничего не закрыто».
 */
const KEY = "glafira.llm.closedResults";
const VERSION = 1;
/** Сколько последних закрытых запросов помнится. */
export const MAX_CLOSED = 200;

function load(): string[] {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw === null) return [];
    const v: unknown = JSON.parse(raw);
    if (typeof v !== "object" || v === null) return [];
    const d = v as Record<string, unknown>;
    if (d.v !== VERSION || !Array.isArray(d.ids)) return [];
    return d.ids.filter((x): x is string => typeof x === "string").slice(-MAX_CLOSED);
  } catch {
    return [];
  }
}

interface ClosedResultsState {
  /** Id запросов, итог запуска которых закрыт; старые первыми. */
  closed: string[];
  close: (requestId: string) => void;
}

export const useClosedResults = create<ClosedResultsState>((set) => ({
  closed: load(),
  close: (requestId) =>
    set((s) => {
      if (s.closed.includes(requestId)) return s;
      const closed = [...s.closed, requestId].slice(-MAX_CLOSED);
      try {
        localStorage.setItem(KEY, JSON.stringify({ v: VERSION, ids: closed }));
      } catch {
        // Хранилище недоступно — закрытие помнится до перезапуска.
      }
      return { closed };
    }),
}));
