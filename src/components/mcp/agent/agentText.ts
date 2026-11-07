import type { TFunction } from "i18next";
import type { AgentRun, FailReason } from "../../../api/types";
import { PRESET_NAMES } from "../../../lib/useAgentRuns";

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
