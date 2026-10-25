import { useQuery } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { Fragment, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { ActionRecord } from "../../api/types";
import { formatTimestamp } from "../../lib/format";
import { ErrorBanner } from "../common/ErrorBanner";

/** Сколько последних записей читает вкладка (предел команды `list_actions`). */
const JOURNAL_LIMIT = 1000;

/** Подпись действия: ключи литералами (правило словарей). Неизвестное действие (запись новой версии) показывается как есть. */
const ACTION_KEYS = new Map<string, string>([
  ["cancel", "actions.kind.cancel"],
  ["terminate", "actions.kind.terminate"],
  ["dropSlot", "actions.kind.dropSlot"],
  ["switchover", "actions.kind.switchover"],
  ["cancelSwitchover", "actions.kind.cancelSwitchover"],
  ["failover", "actions.kind.failover"],
  ["restart", "actions.kind.restart"],
  ["cancelRestart", "actions.kind.cancelRestart"],
  ["pause", "actions.kind.pause"],
  ["resume", "actions.kind.resume"],
]);
const OUTCOME_KEYS = new Map<string, string>([
  ["ok", "actions.outcome.ok"],
  ["partial", "actions.outcome.partial"],
  ["error", "actions.outcome.error"],
]);

/** Объект действия: слот, число сессий или адрес запроса к Patroni. */
function objectText(r: ActionRecord, t: TFunction): string {
  if (r.slot) return r.slot.name;
  if (r.sessions?.length) return t("actions.journal.sessions", { count: r.sessions.length });
  if (r.patroni) return r.patroni.path;
  return "";
}

/**
 * Ключи строк для раскрытия. Индекс не годится: новая запись сверху сдвигает его, и раскрытой оказалась бы чужая строка.
 * Ключ — время, действие и узел; совпадения (одна миллисекунда) различаются порядковым номером.
 */
function rowKeys(rows: ActionRecord[]): string[] {
  const seen = new Map<string, number>();
  return rows.map((r) => {
    const base = `${r.ts}:${r.action}:${r.member ?? ""}`;
    const n = seen.get(base) ?? 0;
    seen.set(base, n + 1);
    return `${base}:${n}`;
  });
}

/**
 * Журнал действий подключения (спец. P2 §8.8): время, действие, узел, объект, итог; строка раскрывается в полную запись.
 * active — вкладка сейчас видна: скрытая журнал не читает, а при возвращении читает заново (действие могли выполнить на другой вкладке).
 */
export function ActionsJournal({ connectionId, active = true }: { connectionId: string; active?: boolean }) {
  const { t } = useTranslation();
  const q = useQuery({ queryKey: ["actions", connectionId], queryFn: () => api.listActions(connectionId, JOURNAL_LIMIT), enabled: active });
  const [openKey, setOpenKey] = useState<string | null>(null);
  const rows = q.data ?? [];
  const keys = rowKeys(rows);
  return (
    <div className="diag-view">
      <div className="toolbar">
        <h2>{t("actions.journal.title")}</h2>
        <button type="button" onClick={() => void q.refetch()}>
          {t("diag.header.refresh")}
        </button>
      </div>
      {q.error && <ErrorBanner error={q.error} />}
      {q.isLoading &&<p className="muted">{t("common.loading")}</p>}
      {q.isSuccess && rows.length === 0 && <p className="muted">{t("actions.journal.empty")}</p>}
      {rows.length > 0 && (
        <div className="table-scroll">
          <table className="diag-table" aria-label={t("actions.journal.title")}>
            <thead>
              <tr>
                <th scope="col">{t("actions.journal.columns.time")}</th>
                <th scope="col">{t("actions.journal.columns.action")}</th>
                <th scope="col">{t("actions.journal.columns.member")}</th>
                <th scope="col">{t("actions.journal.columns.object")}</th>
                <th scope="col">{t("actions.journal.columns.outcome")}</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r, i) => {
                const key = keys[i];
                const actionKey = ACTION_KEYS.get(r.action);
                const outcomeKey = OUTCOME_KEYS.get(r.outcome);
                return (
                  <Fragment key={key}>
                    <tr>
                      <td>{formatTimestamp(r.ts)}</td>
                      <td>
                        <button type="button" className="link" aria-expanded={openKey === key} onClick={() => setOpenKey(openKey === key ? null : key)}>
                          {actionKey ? t(actionKey) : r.action}
                        </button>
                      </td>
                      <td>{r.member ?? ""}</td>
                      <td>{objectText(r, t)}</td>
                      <td className={r.outcome === "ok" ? undefined : "diag-warn"}>{outcomeKey ? t(outcomeKey) : r.outcome}</td>
                    </tr>
                    {openKey === key && (
                      <tr>
                        <td colSpan={5}>
                          {/* Полная запись как её записал Rust: код ошибки, текст запроса и итог каждой сессии, ответ Patroni. */}
                          <pre>{JSON.stringify(r, null, 2)}</pre>
                        </td>
                      </tr>
                    )}
                  </Fragment>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
