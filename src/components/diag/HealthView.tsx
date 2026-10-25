import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import type { DiagReport, HealthCell, HealthData, HealthGrade, HealthMember } from "../../api/types";
import { tr } from "../../i18n/text";

/**
 * Класс клетки — по оценке Rust (роль участника), а не по коду: 503 на адресе чужой роли — серый, а не красный.
 * Код (или «нет ответа») остаётся в клетке, а смысл оценки — в подсказке: цвет не единственный признак.
 */
const CELL_CLASS: Record<HealthGrade, string> = { ok: "ok", notApplicable: "na", degraded: "degraded", bad: "bad" };

/** Подсказка клетки: причина молчания API или смысл оценки. Ключи — литералами (правило словарей). */
function cellTitle(c: HealthCell, t: TFunction): string | undefined {
  if (c.error) return tr(c.error);
  switch (c.grade) {
    case "notApplicable":
      return t("diag.health.grade.notApplicable");
    case "degraded":
      return t("diag.health.grade.degraded");
    case "bad":
      return t("diag.health.grade.bad");
    default:
      return undefined;
  }
}

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
          <td key={c.endpoint} className={CELL_CLASS[c.grade]} title={cellTitle(c, t)}>
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
      <p className="muted">{t("diag.health.legend")}</p>
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
