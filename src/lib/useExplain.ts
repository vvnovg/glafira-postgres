import { useMutation } from "@tanstack/react-query";
import { api } from "../api/client";
import type { ExplainRequest, ExplainResult } from "../api/types";

/** Номер параметра не растёт дальше: как в Rust, разбор цифр останавливается, когда номер достиг этого предела. */
const PARAM_LIMIT = 100_000;

/**
 * Зеркало `explain::params::count_params`: наибольший номер `$N` (число полей формы). `$$` долларовых кавычек — не
 * параметр; литерал с `$1` внутри строки даёт лишнее поле, а не ошибку.
 */
export function countParams(sql: string): number {
  let max = 0;
  let i = 0;
  while (i < sql.length) {
    if (sql[i] !== "$") {
      i += 1;
      continue;
    }
    let j = i + 1;
    let n = 0;
    while (j < sql.length && sql[j] >= "0" && sql[j] <= "9" && n < PARAM_LIMIT) {
      n = n * 10 + (sql.charCodeAt(j) - 48);
      j += 1;
    }
    if (j > i + 1 && n > max) max = n;
    i = Math.max(j, i + 1);
  }
  return max;
}

/** Мутация EXPLAIN: один запуск за раз; ошибки — `AppError` из `toAppError` на месте показа. */
export function useExplain(connectionId: string) {
  return useMutation({ mutationFn: (request: ExplainRequest): Promise<ExplainResult> => api.explain(connectionId, request) });
}
