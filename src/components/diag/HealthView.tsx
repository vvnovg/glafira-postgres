import { useTranslation } from "react-i18next";
import type { DiagReport, HealthCell, HealthData, HealthMember } from "../../api/types";
import { tr } from "../../i18n/text";

/** Класс клетки: 200 — `ok`, любой иной код — `bad`, API не ответил — `none`. Код (или слова «нет ответа») остаётся в клетке: цвет не единственный признак. */
const cellClass = (c: HealthCell) => (c.status === null ? "none" : c.status === 200 ? "ok" : "bad");

function MemberRow({ m, columns }: { m: HealthMember; columns: number }) {
  const { t } = useTranslation();
  return (
    <tr>
      <td title={m.apiUrl ?? undefined}>{m.member}</td>
      {m.apiUrl === null ? (
        // Без адреса REST API спрашивать нечего: клеток нет.
        <td colSpan={columns} className="none">
          {t("diag.health.noApi")}
        </td>
      ) : (
        m.cells.map((c) => (
          <td key={c.endpoint} className={cellClass(c)} title={c.error ? tr(c.error) : undefined}>
            {c.status === null ? t("diag.health.noAnswer") : String(c.status)}
          </td>
        ))
      )}
    </tr>
  );
}

function HealthBody({ data }: { data: HealthData }) {
  const { t } = useTranslation();
  return (
    <div className="diag-view">
      {data.members.length === 0 ? (
        <p className="muted">{t("diag.health.none")}</p>
      ) : (
        <div className="table-scroll">
          <table className="diag-table health-table" aria-label={t("diag.tools.health")}>
            <thead>
              <tr>
                <th scope="col">{t("diag.health.member")}</th>
                {data.endpoints.map((e) => (
                  <th key={e} scope="col">{`/${e}`}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {data.members.map((m) => (
                <MemberRow key={m.member} m={m} columns={data.endpoints.length} />
              ))}
            </tbody>
          </table>
        </div>
      )}
      <p className="muted">{t("diag.health.note")}</p>
    </div>
  );
}

/**
 * Матрица health (спец. P2 §5.9): сетка «участники × адреса REST API» с кодами ответов — так видит кластер балансировщик
 * (HAProxy с `option httpchk`). Причина молчания клетки — в её подсказке.
 */
export function HealthView({ report }: { connectionId: string; report: DiagReport }) {
  if (report.data.kind !== "health") return null;
  return <HealthBody data={report.data} />;
}
