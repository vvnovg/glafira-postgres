import { useTranslation } from "react-i18next";
import type { PlanTree, QueryDetail, QueryStat, RelationRef } from "../../api/types";
import { formatBytes, formatCount, formatDuration, formatMs, formatPercent } from "../../lib/format";
import { useWorkspace } from "../../state/workspace";
import { CopyButton } from "../common/CopyButton";
import { FindingsPanel } from "../diag/FindingsPanel";
import { unavailableText } from "../diag/unavailable";
import { CopyLlmButton } from "./CopyLlmButton";
import { AskLlmButton } from "./llm/AskLlmButton";
import { Sparkline } from "./Sparkline";
import { BLOCK_BYTES, cpuMs } from "./queriesText";

/** Значений параметров у нормализованного текста карточки нет: снимок уходит без них. */
const NO_PARAMS: (string | null)[] = [];

/** Строки метрик: ключ подписи — литералом (правило словарей), значение — из строки pgss. */
const METRICS: { id: string; labelKey: string; of: (s: QueryStat, trackPlanning: boolean) => string; kcache?: true }[] = [
  { id: "calls", labelKey: "diag.query.metric.calls", of: (s) => formatCount(s.calls) },
  { id: "total", labelKey: "diag.query.metric.total", of: (s) => formatMs(s.totalExecMs) },
  { id: "mean", labelKey: "diag.query.metric.mean", of: (s) => formatMs(s.meanExecMs) },
  { id: "minMax", labelKey: "diag.query.metric.minMax", of: (s) => (s.minExecMs === null || s.maxExecMs === null ? "—" : `${formatMs(s.minExecMs)} / ${formatMs(s.maxExecMs)}`) },
  { id: "stddev", labelKey: "diag.query.metric.stddev", of: (s) => (s.stddevExecMs === null ? "—" : formatMs(s.stddevExecMs)) },
  { id: "rowsPerCall", labelKey: "diag.query.metric.rowsPerCall", of: (s) => formatCount(s.rowsPerCall, 1) },
  { id: "hitRatio", labelKey: "diag.query.metric.hitRatio", of: (s) => (s.hitRatio === null ? "—" : formatPercent(s.hitRatio * 100)) },
  { id: "read", labelKey: "diag.query.metric.read", of: (s) => `${formatBytes(s.sharedRead * BLOCK_BYTES)} / ${formatBytes(s.sharedWritten * BLOCK_BYTES)}` },
  { id: "temp", labelKey: "diag.query.metric.temp", of: (s) => formatBytes(s.tempWritten * BLOCK_BYTES) },
  { id: "wal", labelKey: "diag.query.metric.wal", of: (s) => formatBytes(s.walBytes) },
  { id: "planning", labelKey: "diag.query.metric.planning", of: (s, trackPlanning) => (trackPlanning ? formatMs(s.totalPlanMs) : "—") },
  { id: "jit", labelKey: "diag.query.metric.jit", of: (s) => (s.jitMs === null ? "—" : formatMs(s.jitMs)) },
  { id: "cpu", labelKey: "diag.query.metric.cpu", kcache: true, of: (s) => { const v = cpuMs(s); return v === null ? "—" : formatMs(v); } },
  {
    id: "disk",
    labelKey: "diag.query.metric.disk",
    kcache: true,
    of: (s) => (s.diskReadBytes === null || s.diskWriteBytes === null ? "—" : `${formatBytes(s.diskReadBytes)} / ${formatBytes(s.diskWriteBytes)}`),
  },
];

/**
 * Обзор карточки (спец. P3 §7.3): текст (с копированием), метрики (с момента сброса и за час), история (промежутки
 * сэмплера рвут линию), находки, разбивка, экземпляры, ожидания, таблицы (из последнего плана вкладки; `null` — плана ещё
 * не было). Ряд кнопок под текстом: «Получить план» (`onGetPlan` — открывает подвкладку «План» и запускает пробный план),
 * «Открыть в консоли», «Копировать запрос», «Скопировать для LLM» (`llmPlan` — последний план вкладки на её узле), «Передать LLM-ассистенту» (запрос на оптимизацию, спец. P4 §8.1). Без выполняющихся
 * экземпляров на сервере < 14 блок «Выполняющиеся экземпляры» скрыт целиком, пометка о причине остаётся вверху. Есть
 * предложения LLM к запросу (`proposals` > 0) — в том же ряду после «Передать LLM-ассистенту» ссылка «Предложения LLM: N —
 * открыть в консоли» (`onProposals`): отправка, ожидание и ответ — в одном месте.
 */
/**
 * Почему у запроса нет ожиданий: сэмплера активности на узле нет — подсказка открыть «Профиль ожиданий»; буфер есть, но
 * запроса в сэмплах нет — «не встречался за …» (с пометкой, если сбор остановлен); тиков ещё не было — «сэмплов ещё нет».
 */
function noWaitsText(sampler: QueryDetail["waitsSampler"], t: (k: string, o?: Record<string, unknown>) => string): string {
  if (sampler === null) return t("diag.query.noWaitsBuffer");
  if (sampler.coverageS === null) return t("diag.query.noWaitsYet");
  const notSeen = t("diag.query.noWaitsInSamples", { coverage: formatDuration(sampler.coverageS) });
  return sampler.running ? notSeen : `${notSeen}. ${t("diag.query.noWaitsStopped")}`;
}

export function QueryOverview({
  connectionId,
  member,
  database,
  detail,
  queryId,
  relations,
  llmPlan,
  onGetPlan,
  onConsole,
  proposals = 0,
  onProposals,
}: {
  connectionId: string;
  member: string;
  database: string;
  detail: QueryDetail;
  queryId: string;
  relations: RelationRef[] | null;
  llmPlan: PlanTree | null;
  onGetPlan: () => void;
  onConsole: () => void;
  /** Сколько предложений LLM к запросу (P4b). */
  proposals?: number;
  onProposals?: () => void;
}) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const hour = detail.windows.find((w) => w.windowS === 3600)?.stat ?? null;
  // Находки карточки — об этом же запросе: ссылка «Открыть запрос» вела бы в эту же вкладку.
  const findings = detail.findings.map((f) => ({ ...f, links: f.links.filter((l) => l.kind !== "openQuery") }));
  const runningUnavailable = detail.unavailable.some((u) => u.part === "running");
  const sinceReset = detail.sinceReset;
  const proposalsLink = proposals > 0 && (
    <button type="button" className="link" onClick={onProposals}>
      {t("llmAssist.badge", { count: proposals })}
    </button>
  );
  return (
    <div className="query-overview">
      <section>
        {detail.text === null ? (
          <p className="muted">{t("diag.query.gone")}</p>
        ) : (
          <pre className="query-text" role="group" tabIndex={0} aria-label={t("diag.query.text")}>
            <code>{detail.text}</code>
            {detail.textTruncated && <span className="muted"> {t("diag.queries.truncated")}</span>}
          </pre>
        )}
        <div className="toolbar">
          {/* Без текста (запрос ушёл из pgss) и с обрезанным текстом плана не построить: кнопка ждала бы запуска вечно. */}
          {detail.text !== null && !detail.textTruncated && (
            <button type="button" onClick={onGetPlan}>
              {t("diag.query.getPlan")}
            </button>
          )}
          <button type="button" disabled={detail.text === null} onClick={onConsole}>
            {t("diag.query.openConsole")}
          </button>
          {detail.text !== null && <CopyButton text={detail.text} caption={t("diag.query.copyQuery")} />}
          <CopyLlmButton connectionId={connectionId} member={member} database={database} source={{ kind: "query", queryId }} plan={llmPlan} />
          {/* Обрезанный текст модель не спланирует, а исчезнувший запрос передавать нечего; ссылка на предложения остаётся. */}
          {detail.text !== null && !detail.textTruncated ? (
            <AskLlmButton connectionId={connectionId} member={member} database={database} source={{ kind: "query", queryId }} sql={detail.text} params={NO_PARAMS} plan={llmPlan} extra={proposalsLink} />
          ) : (
            proposalsLink
          )}
        </div>
      </section>
      <FindingsPanel connectionId={connectionId} findings={findings} member={member} />
      {detail.unavailable.map((u, i) => (
        <p key={`${u.part}:${i}`} className="muted">
          {unavailableText(u)}
        </p>
      ))}
      {sinceReset && (
        <section>
          <h3>{t("diag.query.metrics")}</h3>
          <div className="table-scroll">
            <table className="diag-table" aria-label={t("diag.query.metrics")}>
              <thead>
                <tr>
                  <td />
                  <th scope="col" className="num">
                    {t("diag.query.sinceReset")}
                  </th>
                  <th scope="col" className="num">
                    {t("diag.query.lastHour")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {/* Строки `pg_stat_kcache` — только когда расширение установлено; нет данных у запроса — «—». */}
                {METRICS.filter((m) => !m.kcache || detail.kcache).map((m) => (
                  <tr key={m.id}>
                    <th scope="row">{t(m.labelKey)}</th>
                    <td className="num">{m.of(sinceReset, detail.trackPlanning)}</td>
                    <td className="num">{hour ? m.of(hour, detail.trackPlanning) : "—"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}
      <section className="query-history">
        <h3>{t("diag.query.history")}</h3>
        <Sparkline points={detail.history.map((p) => ({ atMs: p.atMs, value: p.calls }))} gaps={detail.gaps} label={t("diag.query.historyCalls")} />
        <Sparkline points={detail.history.map((p) => ({ atMs: p.atMs, value: p.meanExecMs }))} gaps={detail.gaps} label={t("diag.query.historyMean")} />
        {detail.history.length === 0 && <p className="muted">{t("diag.query.noHistory")}</p>}
        {detail.gaps.length > 0 && <p className="muted">{t("diag.query.gaps", { count: detail.gaps.length })}</p>}
      </section>
      {detail.byUser.length > 1 && (
        <section>
          <h3>{t("diag.query.byUser")}</h3>
          <div className="table-scroll">
            <table className="diag-table" aria-label={t("diag.query.byUser")}>
              <thead>
                <tr>
                  <th scope="col">{t("diag.query.role")}</th>
                  <th scope="col">{t("diag.query.toplevel")}</th>
                  <th scope="col" className="num">
                    {t("diag.query.metric.calls")}
                  </th>
                  <th scope="col" className="num">
                    {t("diag.query.metric.total")}
                  </th>
                  <th scope="col" className="num">
                    {t("diag.query.metric.mean")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {detail.byUser.map((u) => (
                  <tr key={`${u.rolname ?? ""}:${u.toplevel}`}>
                    <td>{u.rolname ?? "—"}</td>
                    <td>{u.toplevel ? t("common.yes") : t("common.no")}</td>
                    <td className="num">{formatCount(u.stat.calls)}</td>
                    <td className="num">{formatMs(u.stat.totalExecMs)}</td>
                    <td className="num">{formatMs(u.stat.meanExecMs)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}
      {/* На PG 13 экземпляров не узнать: причина — в строке «недоступно» выше, блок не показывается совсем. */}
      {!runningUnavailable && (
        <section>
          <h3>{t("diag.query.running")}</h3>
          {detail.running.length > 0 ? (
            <div className="table-scroll">
              <table className="diag-table" aria-label={t("diag.query.running")}>
                <thead>
                  <tr>
                    <th scope="col">PID</th>
                    <th scope="col">{t("diag.query.state")}</th>
                    <th scope="col" className="num">
                      {t("diag.query.duration")}
                    </th>
                    <th scope="col">{t("diag.query.wait")}</th>
                    <th scope="col">{t("diag.query.who")}</th>
                  </tr>
                </thead>
                <tbody>
                  {detail.running.map((r) => (
                    <tr key={`${r.pid}:${r.backendStartUs}`}>
                      <td>{r.pid}</td>
                      <td>{r.state ?? ""}</td>
                      <td className="num">{formatDuration(r.durationS)}</td>
                      <td>{r.waitEventType ? `${r.waitEventType}${r.waitEvent ? `:${r.waitEvent}` : ""}` : ""}</td>
                      <td>{[r.usename, r.applicationName, r.clientAddr].filter(Boolean).join(" · ")}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <p className="muted">{t("diag.query.noRunning")}</p>
          )}
          <button type="button" className="link" onClick={() => open({ kind: "diag", connectionId, tool: "sessions", member })}>
            {t("diag.query.openSessions", { tool: t("diag.tools.sessions") })}
          </button>
        </section>
      )}
      <section>
        <h3>{t("diag.query.waits")}</h3>
        {detail.waits === null ? (
          <p className="muted">{noWaitsText(detail.waitsSampler, t)}</p>
        ) : (
          <>
            <p className="muted">
              {detail.waits.source === "pgWaitSampling" ? t("diag.query.waitsSource.pgWaitSampling") : t("diag.query.waitsSource.sampler", { coverage: formatDuration(detail.waits.coverageS) })}
            </p>
            <div className="table-scroll">
              <table className="diag-table" aria-label={t("diag.query.waits")}>
                <thead>
                  <tr>
                    <th scope="col">{t("diag.query.waitEvent")}</th>
                    <th scope="col" className="num">
                      {t("diag.query.share")}
                    </th>
                    <th scope="col" className="num">
                      {t("diag.query.samples")}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {detail.waits.items.map((w) => (
                    <tr key={`${w.eventType}:${w.event ?? ""}`}>
                      <td>{w.event ? `${w.eventType}:${w.event}` : w.eventType}</td>
                      <td className="num">{formatPercent(w.sharePct)}</td>
                      <td className="num">{formatCount(w.count)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </>
        )}
      </section>
      <section>
        <h3>{t("diag.query.tables")}</h3>
        {relations === null ? (
          <p className="muted">{t("diag.query.tablesAfterPlan")}</p>
        ) : relations.length === 0 ? (
          <p className="muted">{t("diag.query.noTables")}</p>
        ) : (
          <ul className="plain-list">
            {relations.map((r) => (
              <li key={`${r.schema ?? ""}.${r.name}`}>
                <button type="button" className="link" onClick={() => open({ kind: "table", connectionId, database, schema: r.schema ?? "public", name: r.name })}>
                  {r.schema ? `${r.schema}.${r.name}` : r.name}
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
