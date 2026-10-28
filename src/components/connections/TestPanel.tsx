import { useTranslation } from "react-i18next";
import type { ReadinessItem, ReadinessStatus, StepStatus, TestEvent, TestReport, TopologyWarning } from "../../api/types";
import { tr } from "../../i18n/text";
import { CopyButton } from "../common/CopyButton";

const STEP_ICON: Record<StepStatus, string> = { running: "⏳", ok: "✅", failed: "❌", skipped: "⏭" };
const READY_ICON: Record<ReadinessStatus, string> = { ok: "✅", warn: "⚠️", missing: "❌" };

/** Скрипт роли для диагностики (спец. P1 §10). */
export const ROLE_SCRIPT = `CREATE ROLE glafira_diag LOGIN PASSWORD '…';
GRANT pg_monitor TO glafira_diag;
GRANT pg_signal_backend TO glafira_diag;   -- cancel and terminate sessions
GRANT pg_maintain TO glafira_diag;         -- PostgreSQL 17+: VACUUM and ANALYZE
-- in every database you want to analyze (and pg_stat_statements in shared_preload_libraries):
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;`;

/** Последнее состояние каждого шага в порядке первого появления. */
function latestSteps(events: TestEvent[]): TestEvent[] {
  const order: string[] = [];
  const last = new Map<string, TestEvent>();
  for (const e of events) {
    const key = `${e.step}:${e.member ?? ""}`;
    if (!last.has(key)) order.push(key);
    last.set(key, e);
  }
  return order.map((k) => last.get(k)!);
}

type Translate = (k: string, o?: Record<string, unknown>) => string;

/** Значок статуса: эмодзи не читается скринридером как статус, поэтому у него текстовая подпись. */
function StatusIcon({ icon, label }: { icon: string; label: string }) {
  return (
    <span role="img" aria-label={label}>
      {icon}
    </span>
  );
}

/** Тексты пункта: вариант — `params.reason` (если есть), иначе статус; ключи `readiness.<id>.<вариант>`. */
function itemText(item: ReadinessItem, t: Translate) {
  const variant = item.params.reason ?? item.status;
  return {
    title: t(`readiness.${item.id}.title`, item.params),
    state: t(`readiness.${item.id}.${variant}`, item.params),
    affects: item.status === "ok" ? null : t(`readiness.${item.id}.affects`, item.params),
  };
}

export function warningText(w: TopologyWarning, t: Translate): string {
  switch (w.kind) {
    case "noPrimary":
      return t("topology.warning.noPrimary");
    case "multiplePrimaries":
      return t("topology.warning.multiplePrimaries", { members: w.members.join(", ") });
    case "systemIdMismatch":
      return t("topology.warning.systemIdMismatch", { members: w.members.join(", ") });
    case "patroniUnreachable":
      return t("topology.warning.patroniUnreachable");
    case "memberUnreachable":
      return t("topology.warning.memberUnreachable", { name: w.name });
  }
}

export function TestPanel({ events, report }: { events: TestEvent[]; report: TestReport | null }) {
  const { t } = useTranslation();
  const steps = latestSteps(events);
  const needsRole = report?.readiness.some((i) => (i.id === "monitorRole" || i.id === "signalRole") && i.status !== "ok");
  return (
    <div className="panel">
      {steps.length > 0 && (
        <ul className="steps">
          {steps.map((s) => (
            <li key={`${s.step}:${s.member ?? ""}`}>
              <StatusIcon icon={STEP_ICON[s.status]} label={t(`test.status.${s.status}`)} /> <strong>
                {t(`test.step.${s.step}`)}
                {s.member ? ` ${s.member}` : ""}
                {/* Двоеточие — только перед текстом: шаг без сообщения (readiness: ok) не должен заканчиваться пустым «Готовность:». */}
                {s.message || s.error ? ":" : ""}
              </strong>{" "}
              {s.message && <span>{tr(s.message)}</span>}
              {s.error && <span className="warn">{tr(s.error)}</span>}
              {s.error?.details && <pre className="muted">{s.error.details}</pre>}
            </li>
          ))}
        </ul>
      )}
      {report?.topology && report.topology.warnings.length > 0 && (
        <div className="confirm" role="alert">
          {report.topology.warnings.map((w, i) => (
            <span key={i}>{warningText(w, t)}</span>
          ))}
        </div>
      )}
      {report && report.readiness.length > 0 && (
        <>
          <h3>{t("test.readinessTitle")}</h3>
          <table>
            <tbody>
              {report.readiness.map((item, i) => {
                const text = itemText(item, t);
                return (
                  <tr key={`${item.id}:${i}`}>
                    <td>
                      <StatusIcon icon={READY_ICON[item.status]} label={t(`test.readyStatus.${item.status}`)} />
                    </td>
                    <td>
                      <strong>{text.title}</strong>
                      <div className="muted">{text.state}</div>
                      {text.affects && <div className="muted">{text.affects}</div>}
                    </td>
                    <td>
                      {item.fix && (
                        <span className="path-field">
                          <pre>{item.fix}</pre>
                          <CopyButton text={item.fix} label={t("test.copyFix", { name: text.title, state: text.state })} />
                        </span>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          {needsRole && (
            <details>
              <summary>{t("test.roleScript")}</summary>
              <pre>{ROLE_SCRIPT}</pre>
            </details>
          )}
        </>
      )}
    </div>
  );
}
