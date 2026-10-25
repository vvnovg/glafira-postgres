import { useTranslation } from "react-i18next";
import type { DiagReport, TopEvent, TopName, TopQuery, WaitsData } from "../../api/types";
import { formatCount, formatPercent, formatTimestamp } from "../../lib/format";
import { useSampler } from "../../lib/useSampler";
import { ErrorBanner } from "../common/ErrorBanner";
import { CLASS_LABEL_KEYS, WaitsChart } from "./WaitsChart";
import { uniqueKeys } from "./viewText";

/** Доля сэмплов (0..1) как «60 %». */
const share = (x: number) => formatPercent(x * 100);

function TopEvents({ events }: { events: TopEvent[] }) {
  const { t } = useTranslation();
  const keys = uniqueKeys(events.map((e) => `${e.class}:${e.event ?? ""}`));
  return (
    <section>
      <h3>{t("diag.waits.topEvents")}</h3>
      <div className="table-scroll">
        <table className="diag-table" aria-label={t("diag.waits.topEvents")}>
          <thead>
            <tr>
              <th scope="col">{t("diag.waits.columns.class")}</th>
              <th scope="col">{t("diag.waits.columns.event")}</th>
              <th scope="col">{t("diag.waits.columns.share")}</th>
              <th scope="col">{t("diag.waits.columns.samples")}</th>
            </tr>
          </thead>
          <tbody>
            {events.map((e, i) => (
              <tr key={keys[i]}>
                <td>{t(CLASS_LABEL_KEYS[e.class])}</td>
                <td>{e.event ?? "—"}</td>
                <td className="num">{share(e.share)}</td>
                <td className="num">{formatCount(e.samples)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}

function TopQueries({ queries }: { queries: TopQuery[] }) {
  const { t } = useTranslation();
  const keys = uniqueKeys(queries.map((q) => q.key));
  return (
    <section>
      <h3>{t("diag.waits.topQueries")}</h3>
      <div className="table-scroll">
        <table className="diag-table" aria-label={t("diag.waits.topQueries")}>
          <thead>
            <tr>
              <th scope="col">{t("diag.waits.columns.query")}</th>
              <th scope="col">{t("diag.waits.columns.queryId")}</th>
              <th scope="col">{t("diag.waits.columns.share")}</th>
              <th scope="col">{t("diag.waits.columns.samples")}</th>
              <th scope="col">{t("diag.waits.columns.topEvent")}</th>
            </tr>
          </thead>
          <tbody>
            {queries.map((q, i) => (
              <tr key={keys[i]}>
                <td>
                  {/* Текст в таблице обрезан двумя строками; целиком он — в подсказке. */}
                  <code className="waits-query" title={q.text}>
                    {q.text}
                  </code>
                </td>
                <td className="num">{q.queryId ?? "—"}</td>
                <td className="num">{share(q.share)}</td>
                <td className="num">{formatCount(q.samples)}</td>
                <td>{q.topEvent ?? "—"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}

/** Маленькая таблица группировки (приложение, роль, БД, клиент); пустое имя — «(не задано)», как в списке сессий. */
function Group({ title, rows }: { title: string; rows: TopName[] }) {
  const { t } = useTranslation();
  const keys = uniqueKeys(rows.map((r) => r.name));
  return (
    <section>
      <h3>{title}</h3>
      <div className="table-scroll">
        <table className="diag-table" aria-label={title}>
          <thead>
            <tr>
              <th scope="col">{t("diag.waits.columns.name")}</th>
              <th scope="col">{t("diag.waits.columns.share")}</th>
              <th scope="col">{t("diag.waits.columns.samples")}</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r, i) => (
              <tr key={keys[i]}>
                <td>{r.name === "" ? t("diag.sessions.groupUnset") : r.name}</td>
                <td className="num">{share(r.share)}</td>
                <td className="num">{formatCount(r.samples)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}

/** Начало работающей части истории: первый интервал, в котором сэмплер успел поработать. */
const sampledSince = (data: WaitsData) => data.buckets.find((b) => b.sampledS > 0)?.startMs ?? null;

function WaitsBody({ connectionId, member, data }: { connectionId: string; member: string | null; data: WaitsData }) {
  const { t } = useTranslation();
  // Подписка держит сэмплер узла отчёта, пока вид смонтирован (спец. P2 §6): окно и интервал отчёта её не затрагивают.
  const { live, error } = useSampler(connectionId, member, true);
  const since = data.truncated ? sampledSince(data) : null;
  const groups: [string, TopName[]][] = [
    [t("diag.waits.byApplication"), data.byApplication],
    [t("diag.waits.byUser"), data.byUser],
    [t("diag.waits.byDatabase"), data.byDatabase],
    [t("diag.waits.byClient"), data.byClient],
  ];
  return (
    <div className="diag-view waits-view">
      <div className="waits-status">
        {live && <p className="waits-live">{t("diag.waits.live", { active: live.active, waiting: live.waiting })}</p>}
        {error && <ErrorBanner error={error} />}
        {/* Отчёт получен раньше подписки: пока идут тики, «не работает» из отчёта устарело. */}
        {!data.samplerRunning && !live && <p className="diag-warn">{t("diag.waits.notRunning")}</p>}
        {since !== null && <p className="muted">{t("diag.waits.truncated", { time: formatTimestamp(since) })}</p>}
      </div>
      <WaitsChart data={data} />
      <p className="muted">{t("diag.waits.cpuNote")}</p>
      {data.totalSamples === 0 ? (
        // Пустые срезы тоже приходят: сервер простаивал — это не «сэмплов нет».
        <p className="muted">{data.buckets.some((b) => b.sampledS > 0) ? t("diag.waits.noActive") : t("diag.waits.noSamples")}</p>
      ) : (
        <>
          {data.topEvents.length > 0 && <TopEvents events={data.topEvents} />}
          {data.topQueries.length > 0 && <TopQueries queries={data.topQueries} />}
          <div className="waits-groups">
            {groups.map(([title, rows]) => rows.length > 0 && <Group key={title} title={title} rows={rows} />)}
          </div>
        </>
      )}
    </div>
  );
}

/**
 * Профиль ожиданий (спец. P2 §5.3, §7.4): график среднего числа активных сессий по классам ожиданий, топ ожиданий и
 * запросов и разрезы по приложению, роли, БД и клиенту. Узел — тот, чей отчёт на экране (в режиме «за лидером» — текущий
 * лидер): вид подписывается на его сэмплер.
 */
export function WaitsView({ connectionId, report }: { connectionId: string; report: DiagReport }) {
  if (report.data.kind !== "waits") return null;
  return <WaitsBody connectionId={connectionId} member={report.member} data={report.data} />;
}
