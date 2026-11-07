import { useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo } from "react";
import { api } from "../api/client";
import type { AgentPreset, AgentRun } from "../api/types";

/** Событие Rust (`agent::queue::RUNS_EVENT`): запуск целиком при смене статуса и хода работы (не чаще раза в секунду). */
export const AGENT_RUNS_EVENT = "agent://runs";
/** Один кэш на все запуски (их не больше 50): очередь общая для всех подключений. */
export const AGENT_RUNS_KEY = ["agentRuns"] as const;
/** Названия пресетов — имена программ, не переводятся (глоссарий). */
export const PRESET_NAMES: Record<AgentPreset, string> = { qwen: "Qwen Code", claude: "Claude Code" };

const NO_RUNS: AgentRun[] = [];

/** «Не прочитано» (в том числе при `enabled: false`), «прочитано», «не прочиталось». */
export type RunsState = "loading" | "ready" | "failed";

/** Запуск ещё ждёт или идёт: очередь, запуск процесса, работа. */
export function isActive(run: AgentRun): boolean {
  return run.status === "queued" || run.status === "starting" || run.status === "running";
}

/** Новые первыми: по времени постановки в очередь, при равенстве — по id (порядок стабилен). */
export function byQueuedDesc(a: AgentRun, b: AgentRun): number {
  return b.queuedAtMs - a.queuedAtMs || (a.id < b.id ? 1 : a.id > b.id ? -1 : 0);
}

/** Список после события: запуск с тем же id заменяется, новый добавляется; порядок — новые первыми. */
export function upsertRun(list: AgentRun[], run: AgentRun): AgentRun[] {
  return [run, ...list.filter((r) => r.id !== run.id)].sort(byQueuedDesc);
}

/** Последний запуск запроса или null. */
export function latestRunFor(runs: AgentRun[], requestId: string): AgentRun | null {
  let found: AgentRun | null = null;
  for (const r of runs) if (r.requestId === requestId && (found === null || byQueuedDesc(r, found) < 0)) found = r;
  return found;
}

/** Сколько запусков перед `run` в общей очереди: идущий и поставленные раньше (любые подключения и «Проверить»). */
export function queueAhead(all: AgentRun[], run: AgentRun): number {
  return all.filter((r) => r.id !== run.id && (r.status === "starting" || r.status === "running" || (r.status === "queued" && byQueuedDesc(r, run) > 0))).length;
}

/** Время работы «м:сс» (запуск длится не больше часа, часов не нужно). */
export function elapsedText(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/**
 * Запуски фонового ассистента (спец. P4c §3.2, §8): все запуски одним чтением `list_agent_runs` (`all`, новые первыми) и
 * запуски подключения (`runs`; `null` — все). Событие `agent://runs` заменяет запуск в кэше на месте: ход работы приходит
 * до раза в секунду, перечитывать весь список незачем. Событие новее любого ответа, запрошенного до него: идущее чтение
 * отменяется с откатом (как в `useMcpEvents`), а пока список не прочитан, событие его перечитывает. `enabled: false` — ни
 * чтения, ни подписки. Вне Tauri подписка отклоняется — это не ошибка.
 */
export function useAgentRuns(
  connectionId: string | null,
  { enabled = true }: { enabled?: boolean } = {},
): { runs: AgentRun[]; all: AgentRun[]; state: RunsState; refresh(): void } {
  const qc = useQueryClient();
  // retry: false — «не прочиталось» должно показаться сразу (кнопка предлагает «Повторить»).
  const q = useQuery({ queryKey: AGENT_RUNS_KEY, queryFn: async () => (await api.listAgentRuns({})).sort(byQueuedDesc), enabled, retry: false });
  const refresh = useCallback(() => void qc.invalidateQueries({ queryKey: AGENT_RUNS_KEY, exact: true }), [qc]);
  useEffect(() => {
    if (!enabled) return;
    // Подписка асинхронна: если хук размонтирован раньше, чем она готова, снимаем её сразу.
    let unmounted = false;
    let off: (() => void) | undefined;
    listen<AgentRun>(AGENT_RUNS_EVENT, (e) => {
      void qc.cancelQueries({ queryKey: AGENT_RUNS_KEY, exact: true }, { revert: true });
      if (qc.getQueryData(AGENT_RUNS_KEY) === undefined) void qc.invalidateQueries({ queryKey: AGENT_RUNS_KEY, exact: true });
      else qc.setQueryData<AgentRun[]>(AGENT_RUNS_KEY, (old) => (old ? upsertRun(old, e.payload) : old));
    })
      .then((unlisten) => (unmounted ? unlisten() : (off = unlisten)))
      .catch(() => {});
    return () => {
      unmounted = true;
      off?.();
    };
  }, [enabled, qc]);
  const all = q.data ?? NO_RUNS;
  const runs = useMemo(() => (connectionId === null ? all : all.filter((r) => r.connectionId === connectionId)), [all, connectionId]);
  const state: RunsState = q.data !== undefined ? "ready" : q.isError ? "failed" : "loading";
  return { runs, all, state, refresh };
}
