import { useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect } from "react";
import { api } from "../api/client";
import type { LlmProposal, OptimizationRequest, ProposalFilter, ProposalsEvent, RequestSource } from "../api/types";

/** Событие Rust (`mcp::llm::emit_proposals_event`): новое предложение или смена статуса запроса подключения. */
export const LLM_PROPOSALS_EVENT = "mcp://proposals";

/** Пустые списки — одни и те же объекты: зависимости эффектов и мемо не меняются, пока данных нет. */
const NO_REQUESTS: OptimizationRequest[] = [];
const NO_PROPOSALS: LlmProposal[] = [];

type RequestsState = "loading" | "ready" | "failed";

/**
 * Перечитывает `refresh` по событию `mcp://proposals` своего подключения; вне Tauri подписка отклоняется — не ошибка.
 */
function useProposalsEvents(connectionId: string, refresh: () => void): void {
  useEffect(() => {
    // Подписка асинхронна: если хук размонтирован раньше, чем она готова, снимаем её сразу.
    let unmounted = false;
    let off: (() => void) | undefined;
    listen<ProposalsEvent>(LLM_PROPOSALS_EVENT, (e) => {
      if (e.payload.connectionId === connectionId) refresh();
    })
      .then((unlisten) => (unmounted ? unlisten() : (off = unlisten)))
      .catch(() => {});
    return () => {
      unmounted = true;
      off?.();
    };
  }, [connectionId, refresh]);
}

/**
 * Лёгкое чтение для кнопки «Передать LLM-ассистенту» (она не нуждается в предложениях): только запросы подключения.
 * Кэш общий с `useLlmProposals` (`["llmRequests", connectionId]`). Состояние различает «читается», «прочитано» и
 * «не прочиталось»: неудача не равна «ничего не ждёт» — кнопка не должна разрешать дубль запроса вслепую.
 * Ошибка фонового перечитывания при уже полученных данных состояния не меняет: данные остаются.
 */
export function useOptimizationRequests(connectionId: string): { requests: OptimizationRequest[]; state: RequestsState; refresh(): void; retry(): void } {
  const qc = useQueryClient();
  // retry: false — «не прочиталось» должно показаться сразу, а не после автоповторов (кнопка без пояснения не должна долго быть неактивной).
  const q = useQuery({ queryKey: ["llmRequests", connectionId], queryFn: () => api.listOptimizationRequests(connectionId), retry: false });
  const refresh = useCallback(() => {
    void qc.invalidateQueries({ queryKey: ["llmRequests", connectionId] });
  }, [qc, connectionId]);
  useProposalsEvents(connectionId, refresh);
  const state: RequestsState = q.data !== undefined ? "ready" : q.isError ? "failed" : "loading";
  const refetch = q.refetch;
  const retry = useCallback(() => void refetch(), [refetch]);
  return { requests: q.data ?? NO_REQUESTS, state, refresh, retry };
}

/**
 * Запросы на оптимизацию подключения и предложения LLM по фильтру (спец. P4 §8) — для панели предложений. Списки
 * перечитываются по событию `mcp://proposals` этого подключения и по `refresh` — после своих действий (создание и отмена
 * запроса, ответ, сравнение). Вне Tauri (тесты, браузер) подписка отклоняется — это не ошибка. Ошибку чтения списки не
 * показывают: панель работает с тем, что есть. `filter` — объединение условий (см. `ProposalFilter`).
 */
export function useLlmProposals(connectionId: string, filter: ProposalFilter): { requests: OptimizationRequest[]; proposals: LlmProposal[]; refresh(): void } {
  const qc = useQueryClient();
  const requests = useQuery({ queryKey: ["llmRequests", connectionId], queryFn: () => api.listOptimizationRequests(connectionId) });
  const proposals = useQuery({ queryKey: ["llmProposals", connectionId, filter], queryFn: () => api.listLlmProposals(filter) });
  const refresh = useCallback(() => {
    void qc.invalidateQueries({ queryKey: ["llmRequests", connectionId] });
    void qc.invalidateQueries({ queryKey: ["llmProposals", connectionId] });
  }, [qc, connectionId]);
  useProposalsEvents(connectionId, refresh);
  return { requests: requests.data ?? NO_REQUESTS, proposals: proposals.data ?? NO_PROPOSALS, refresh };
}

/** Новые (без ответа) первыми, внутри — свежие первыми (спец. §8.2: «сначала новые»). */
export function byNewest(a: LlmProposal, b: LlmProposal): number {
  return Number(b.status === "new") - Number(a.status === "new") || b.createdAtMs - a.createdAtMs;
}

/** Запрос того же источника: консоль одна на подключение; карточка — тот же queryid в той же базе. */
export function sourceMatches(r: OptimizationRequest, source: RequestSource, database: string): boolean {
  if (source.kind === "console") return r.source.kind === "console";
  return r.source.kind === "query" && r.source.queryId === source.queryId && r.database === database;
}

/** Последний запрос источника, который ждёт ответа модели, или null. */
export function waitingRequest(requests: OptimizationRequest[], source: RequestSource, database: string): OptimizationRequest | null {
  let found: OptimizationRequest | null = null;
  for (const r of requests) {
    if (r.status === "waiting" && sourceMatches(r, source, database) && (found === null || r.createdAtMs > found.createdAtMs)) found = r;
  }
  return found;
}

/**
 * Предложения консоли (спец. §8.4): на запросы этой консоли — в любой базе; без запроса — по базе консоли; `pinned` —
 * открытые значком карточки (их запрос — карточки, но показать их нужно здесь).
 */
export function consoleProposals(proposals: LlmProposal[], requests: OptimizationRequest[], database: string | null, pinned: readonly string[]): LlmProposal[] {
  const own = new Set(requests.filter((r) => r.source.kind === "console").map((r) => r.id));
  return proposals.filter((p) => pinned.includes(p.id) || (p.requestId ? own.has(p.requestId) : p.database === database)).sort(byNewest);
}

/** Предложения карточки (спец. §8.4): по `originalQueryId` и по запросам из этой карточки — та же база. */
export function queryProposals(proposals: LlmProposal[], requests: OptimizationRequest[], database: string, queryId: string): LlmProposal[] {
  const own = new Set(requests.filter((r) => sourceMatches(r, { kind: "query", queryId }, database)).map((r) => r.id));
  return proposals
    .filter((p) => p.database === database && (p.originalQueryId === queryId || (!!p.requestId && own.has(p.requestId))))
    .sort(byNewest);
}
