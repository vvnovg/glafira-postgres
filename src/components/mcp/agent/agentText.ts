import type { TFunction } from "i18next";
import type { AgentRun, FailReason, McpAuditEntry, RunStatus } from "../../../api/types";
import { formatDuration } from "../../../lib/format";
import { isActive, PRESET_NAMES } from "../../../lib/useAgentRuns";

/** Причины ошибки запуска без параметров — литералами (правило словарей); варианты с параметром — в `failReasonText`. */
const REASON_KEYS: Record<FailReason, string> = {
  cliNotFound: "llmAssist.agent.reason.cliNotFoundPlain",
  spawnFailed: "llmAssist.agent.reason.spawnFailed",
  timeout: "llmAssist.agent.reason.timeoutPlain",
  limit: "llmAssist.agent.reason.limit",
  exit: "llmAssist.agent.reason.exitPlain",
  notConnected: "llmAssist.agent.reason.notConnected",
  mcpOff: "llmAssist.agent.reason.mcpOff",
  accessOff: "llmAssist.agent.reason.accessOff",
  policy: "llmAssist.agent.reason.policy",
  notConfigured: "llmAssist.agent.reason.notConfigured",
  unsafeTools: "llmAssist.agent.reason.unsafeToolsPlain",
};

/** «Qwen Code, qwen3.6-64k:35b-a3b» — агент и модель запуска; без модели (модель клиента по умолчанию) — только агент. */
export function agentLabel(run: Pick<AgentRun, "preset" | "model">): string {
  return run.model ? `${PRESET_NAMES[run.preset]}, ${run.model}` : PRESET_NAMES[run.preset];
}

/**
 * Предупреждение проверки инструментов (спец. P4c §4.1): `unsafe no-init` — агент не прислал событие `init`;
 * `unsafe <имена через запятую>` — лишние инструменты. Не такое предупреждение — null.
 */
function unsafeWarning(w: string): "noInit" | string[] | null {
  const m = /^unsafe (.+)$/.exec(w);
  if (!m) return null;
  if (m[1] === "no-init") return "noInit";
  const names = m[1]
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s !== "");
  return names.length > 0 ? names : null;
}

function unsafeText(u: "noInit" | string[], t: TFunction): string {
  return u === "noInit" ? t("llmAssist.agent.reason.unsafeNoInit") : t("llmAssist.agent.reason.unsafeTools", { tools: u.join(", ") });
}

/**
 * Причина ошибки запуска (спец. P4c §3.2, §8.1). Срока в записи запуска нет: «превышен срок N мин» считает минуты от
 * старта до конца (срок Glafira отсчитывается от `starting`, остановка добавляет секунды — округление их снимает).
 * `unsafeTools` называет инструменты из предупреждения `unsafe …` запуска.
 */
export function failReasonText(run: AgentRun, t: TFunction): string {
  switch (run.reason) {
    case "cliNotFound":
      return run.path ? t("llmAssist.agent.reason.cliNotFound", { path: run.path }) : t(REASON_KEYS.cliNotFound);
    case "timeout": {
      const min = run.startedAtMs != null && run.endedAtMs != null ? Math.round((run.endedAtMs - run.startedAtMs) / 60_000) : 0;
      return min > 0 ? t("llmAssist.agent.reason.timeout", { min }) : t(REASON_KEYS.timeout);
    }
    case "exit":
      return run.exitCode != null ? t("llmAssist.agent.reason.exit", { code: run.exitCode }) : t(REASON_KEYS.exit);
    case "unsafeTools": {
      for (const w of run.warnings) {
        const u = unsafeWarning(w);
        if (u !== null) return unsafeText(u, t);
      }
      return t(REASON_KEYS.unsafeTools);
    }
    case null:
    case undefined:
      return t("llmAssist.agent.reason.unknown");
    default:
      return t(REASON_KEYS[run.reason]);
  }
}

/**
 * Предупреждение запуска текстом словаря: `evict <другие> <модель>` (спец. P4c §6.3) и `unsafe …` (§4.1); незнакомое —
 * как прислал Rust.
 */
export function warningText(w: string, t: TFunction): string {
  const m = /^evict (\S+) (\S+)$/.exec(w);
  if (m) return t("llmAssist.agent.evict", { other: m[1].split(",").join(", "), model: m[2] });
  const u = unsafeWarning(w);
  return u !== null ? unsafeText(u, t) : w;
}

/** «Qwen Code 0.26.5, qwen3.6-64k:35b-a3b» — агент с версией программы и модель запуска. */
export function agentVersionLabel(run: AgentRun): string {
  const name = run.cliVersion ? `${PRESET_NAMES[run.preset]} ${run.cliVersion}` : PRESET_NAMES[run.preset];
  return run.model ? `${name}, ${run.model}` : name;
}

/** Длительность запуска в секундах (от старта до конца) или null. */
export function runSeconds(run: AgentRun): number | null {
  return run.startedAtMs != null && run.endedAtMs != null ? Math.max(0, Math.round((run.endedAtMs - run.startedAtMs) / 1000)) : null;
}

/** Итог запуска в «Запусках» (спец. P4c §8.3): предложений: N / без предложений / ошибка / отменён / прерван. */
export function runResultText(run: AgentRun, t: TFunction): string {
  switch (run.status) {
    case "queued":
      return t("settings.mcp.agent.runs.result.queued");
    case "starting":
      return t("settings.mcp.agent.runs.result.starting");
    case "running":
      return t("settings.mcp.agent.runs.result.running");
    case "done":
      if (run.kind === "check") return t("settings.mcp.agent.runs.result.checkOk");
      return run.proposalIds.length > 0 ? t("settings.mcp.agent.runs.result.proposals", { n: run.proposalIds.length }) : t("settings.mcp.agent.runs.result.none");
    case "failed":
      return t("settings.mcp.agent.runs.result.failed", { reason: failReasonText(run, t) });
    case "cancelled":
      return t("settings.mcp.agent.runs.result.cancelled");
    case "interrupted":
      return t("settings.mcp.agent.runs.result.interrupted");
    case "skipped":
      return t("settings.mcp.agent.runs.result.skipped");
  }
}

/** Итог «Проверить» (спец. P4c §8.3): «Работает: Qwen Code 0.26.5, модель, 23 с» или «Не работает: причина». */
export function checkResultText(run: AgentRun, t: TFunction): string {
  if (isActive(run)) return t("settings.mcp.agent.checking");
  if (run.status === "done") return t("settings.mcp.agent.checkOk", { agent: agentVersionLabel(run), duration: formatDuration(runSeconds(run)) });
  return t("settings.mcp.agent.checkFailed", { reason: run.status === "failed" ? failReasonText(run, t) : runResultText(run, t) });
}

const RUN_STATUSES: readonly RunStatus[] = ["queued", "starting", "running", "done", "failed", "cancelled", "interrupted", "skipped"];

/**
 * Итог записи журнала MCP `agentRunFinished` (спец. P4c §9.2: `status`, `reason`, `exitCode`, `ms`, `proposals`) — тем же
 * текстом, что в «Запусках»; срок — по `ms`. Статус, которого эта версия не знает, — как в файле.
 */
export function finishedRunText(e: McpAuditEntry, t: TFunction): string {
  const status = RUN_STATUSES.find((s) => s === e.status);
  if (status === undefined) return e.status ?? "";
  const run: AgentRun = {
    id: e.runId ?? "",
    kind: e.requestId ? "optimize" : "check",
    requestId: e.requestId ?? null,
    connectionId: e.connection?.id ?? null,
    preset: "qwen",
    status,
    reason: (e.reason as FailReason | undefined) ?? null,
    exitCode: e.exitCode ?? null,
    queuedAtMs: e.ts,
    startedAtMs: e.ms != null ? 0 : null,
    endedAtMs: e.ms ?? null,
    calls: e.calls ?? 0,
    proposalIds: Array.from({ length: e.proposals ?? 0 }, (_, i) => String(i)),
    warnings: [],
  };
  return runResultText(run, t);
}
