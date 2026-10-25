import type { Unavailable } from "../../api/types";
import { i18n } from "../../i18n";
import { tr } from "../../i18n/text";

/**
 * Подписи частей отчёта, недоступных по версии или правам. Rust передаёт в тексте внутреннее имя части (`part`),
 * а пользователю нужна человеческая подпись. Ключи — литералами (правило словарей); неизвестная часть — как есть.
 */
export const UNAVAILABLE_PARTS: Record<string, string> = {
  queryId: "diag.unavailable.parts.queryId",
  walDir: "diag.unavailable.parts.walDir",
  otherSessionsXmin: "diag.unavailable.parts.otherSessionsXmin",
  replicationDetails: "diag.unavailable.parts.replicationDetails",
  replayPauseState: "diag.unavailable.parts.replayPauseState",
  conflictActiveLogicalSlot: "diag.unavailable.parts.conflictActiveLogicalSlot",
  slotTwoPhase: "diag.unavailable.parts.slotTwoPhase",
  slotConflicting: "diag.unavailable.parts.slotConflicting",
  slotInactiveSince: "diag.unavailable.parts.slotInactiveSince",
  subscriptionErrors: "diag.unavailable.parts.subscriptionErrors",
  patroniSlots: "diag.unavailable.parts.patroniSlots",
};

/** Текст строки «недоступно»: причина из Rust с подписью части вместо её внутреннего имени. */
export function unavailableText(u: Unavailable): string {
  const key = UNAVAILABLE_PARTS[u.part];
  if (!key) return tr(u.reason);
  return tr({ ...u.reason, params: { ...u.reason.params, part: i18n.t(key) } });
}
