import { useTranslation } from "react-i18next";
import type { Candidate, DiagReport, DynConfig, FailoverData, FailoverReason } from "../../api/types";
import { formatBytes, formatCount, formatDuration } from "../../lib/format";
import { bytesText } from "./viewText";

/** Причина отказа кандидату: ключ подписи выбирается по значению, поэтому — литералами. */
const REASON_KEYS: Record<FailoverReason, string> = {
  unreachable: "diag.failover.reasons.unreachable",
  notStreaming: "diag.failover.reasons.notStreaming",
  lagTooHigh: "diag.failover.reasons.lagTooHigh",
  lagUnknown: "diag.failover.reasons.lagUnknown",
  noFailoverTag: "diag.failover.reasons.noFailoverTag",
  zeroPriority: "diag.failover.reasons.zeroPriority",
  notSynchronous: "diag.failover.reasons.notSynchronous",
};

function ConfigSummary({ data, config }: { data: FailoverData; config: DynConfig }) {
  const { t } = useTranslation();
  return (
    <ul className="plain-list diag-facts" aria-label={t("diag.failover.config")}>
      <li>{t("diag.failover.readyCount", { ready: formatCount(data.readyCount), total: formatCount(data.candidates.length) })}</li>
      <li>{t("diag.failover.maxLag", { value: formatBytes(config.maximumLagOnFailover) })}</li>
      <li>{t("diag.failover.syncMode", { value: config.synchronousMode })}</li>
      <li>{t("diag.failover.syncStrict", { value: config.synchronousModeStrict ? "on" : "off" })}</li>
      {config.synchronousNodeCount !== null && <li>{t("diag.failover.syncCount", { value: config.synchronousNodeCount })}</li>}
      {/* ttl, loop_wait и retry_timeout — секунды; null — настройка не прочитана. */}
      <li>{t("diag.failover.timings", { ttl: formatDuration(config.ttl), loopWait: formatDuration(config.loopWait), retryTimeout: formatDuration(config.retryTimeout) })}</li>
      <li>{t("diag.failover.permanentSlots", { names: config.permanentSlots.length > 0 ? config.permanentSlots.join(", ") : "—" })}</li>
    </ul>
  );
}

function CandidateRow({ c }: { c: Candidate }) {
  const { t } = useTranslation();
  return (
    <tr>
      <th scope="row">
        {c.member}
        {c.pendingRestart && <div className="diag-warn">{t("diag.failover.pendingRestart")}</div>}
      </th>
      <td className={c.ready ? "ok" : "bad"}>{t(c.ready ? "diag.failover.ready" : "diag.failover.notReady")}</td>
      <td>
        {c.reasons.length > 0 && (
          <ul className="plain-list">
            {c.reasons.map((r) => (
              <li key={r}>{t(REASON_KEYS[r])}</li>
            ))}
          </ul>
        )}
      </td>
      <td className="num">{bytesText(c.lagBytes)}</td>
    </tr>
  );
}

function FailoverBody({ data }: { data: FailoverData }) {
  const { t } = useTranslation();
  return (
    <div className="diag-view">
      {data.paused && <p className="diag-note diag-warn">{t("diag.failover.paused")}</p>}
      <ConfigSummary data={data} config={data.config} />
      {data.candidates.length === 0 ? (
        <p className="muted">{t("diag.failover.none")}</p>
      ) : (
        <div className="table-scroll">
          <table className="diag-table" aria-label={t("diag.failover.table")}>
            <thead>
              <tr>
                <th scope="col">{t("diag.failover.columns.member")}</th>
                <th scope="col">{t("diag.failover.columns.verdict")}</th>
                <th scope="col">{t("diag.failover.columns.reasons")}</th>
                <th scope="col">{t("diag.failover.columns.lag")}</th>
              </tr>
            </thead>
            <tbody>
              {data.candidates.map((c) => (
                <CandidateRow key={c.member} c={c} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

/**
 * Готовность к failover (спец. P2 §5.8): сводка динамической конфигурации Patroni и вердикт по каждому участнику,
 * кроме лидера, — готов ли принять роль и почему нет. Решение о переключении принимает Patroni; здесь видно, к чему он готов.
 */
export function FailoverView({ report }: { connectionId: string; report: DiagReport }) {
  if (report.data.kind !== "failover") return null;
  return <FailoverBody data={report.data} />;
}
