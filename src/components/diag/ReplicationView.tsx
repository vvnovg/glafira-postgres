import { type ReactNode, useId } from "react";
import { useTranslation } from "react-i18next";
import type { Conflicts, DiagReport, Receiver, ReplicationData, Sender } from "../../api/types";
import { formatCount, formatDuration } from "../../lib/format";
import { Hint } from "../common/Hint";
import { bytesText, uniqueKeys } from "./viewText";

/** Состояние паузы применения WAL (`pg_get_wal_replay_pause_state`): ключи подписей — литералами. */
const PAUSE_KEYS: Record<string, string> = {
  paused: "diag.replication.pause.paused",
  "pause requested": "diag.replication.pause.pauseRequested",
  "not paused": "diag.replication.pause.notPaused",
};

function SenderRow({ s }: { s: Sender }) {
  const { t } = useTranslation();
  const name = s.member ?? s.application ?? String(s.pid);
  // Время отставания записи и сброса — в подсказке ячейки: в строке остаются байты и время применения.
  const lagTitle = (seconds: number | null) => (seconds === null ? undefined : t("diag.replication.lagTime", { value: formatDuration(seconds) }));
  return (
    <tr>
      <th scope="row">
        {name}
        {s.member !== null && s.application !== null && s.application !== s.member && <div className="muted">{s.application}</div>}
      </th>
      <td>{s.clientAddr ?? "—"}</td>
      <td>{s.state ?? "—"}</td>
      <td>{s.syncState ?? "—"}</td>
      <td className="num">{bytesText(s.sentLagBytes)}</td>
      <td className="num" title={lagTitle(s.writeLagS)}>
        {bytesText(s.writeLagBytes)}
      </td>
      <td className="num" title={lagTitle(s.flushLagS)}>
        {bytesText(s.flushLagBytes)}
      </td>
      <td className="num">{bytesText(s.replayLagBytes)}</td>
      <td className="num">{formatDuration(s.replayLagS)}</td>
      <td className="num">{formatDuration(s.replyAgeS)}</td>
      <td>{s.slotName ?? "—"}</td>
      <td className="num">{formatCount(s.xminAge)}</td>
    </tr>
  );
}

function SendersTable({ senders, labelId }: { senders: Sender[]; labelId: string }) {
  const { t } = useTranslation();
  // pid переиспользуется, а у реплик бывает один `application_name`: ключ — pid и приложение, повторы нумеруются.
  const keys = uniqueKeys(senders.map((s) => `${s.pid}:${s.application ?? ""}`));
  return (
    <div className="table-scroll">
      <table className="diag-table" aria-labelledby={labelId}>
        <thead>
          <tr>
            <th scope="col">{t("diag.replication.columns.replica")}</th>
            <th scope="col">{t("diag.replication.columns.client")}</th>
            <th scope="col">{t("diag.replication.columns.state")}</th>
            <th scope="col">{t("diag.replication.columns.sync")}</th>
            <th scope="col">{t("diag.replication.columns.sentLag")}</th>
            <th scope="col">{t("diag.replication.columns.writeLag")}</th>
            <th scope="col">{t("diag.replication.columns.flushLag")}</th>
            <th scope="col">{t("diag.replication.columns.replayLag")}</th>
            <th scope="col">{t("diag.replication.columns.replayDelay")}</th>
            <th scope="col">{t("diag.replication.columns.replyAge")}</th>
            <th scope="col">{t("diag.replication.columns.slot")}</th>
            <th scope="col">{t("diag.replication.columns.xminAge")}</th>
          </tr>
        </thead>
        <tbody>
          {senders.map((s, i) => (
            <SenderRow key={keys[i]} s={s} />
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** Конфликты запросов по видам (нулевые и отсутствующие в версии сервера не показываются) и рост с прошлого обновления. */
function ConflictsValue({ c }: { c: Conflicts }) {
  const { t } = useTranslation();
  const kinds: [string, number | null][] = [
    ["tablespace", c.tablespace],
    ["lock", c.lock],
    ["snapshot", c.snapshot],
    ["bufferpin", c.bufferpin],
    ["deadlock", c.deadlock],
    ["activeLogicalslot", c.activeLogicalslot],
  ];
  const text = kinds
    .filter(([, n]) => n !== null && n > 0)
    .map(([k, n]) => `${k}: ${formatCount(n)}`)
    .join(", ");
  return (
    <>
      {text || formatCount(0)}
      {c.growth !== null && <span className="muted diag-sub">{t("diag.replication.conflictsGrowth", { value: `+${formatCount(c.growth)}` })}</span>}
    </>
  );
}

function ReceiverCard({ data, receiver }: { data: ReplicationData; receiver: Receiver }) {
  const { t } = useTranslation();
  const labelId = useId();
  const { replay, conflicts } = data;
  // Нет статуса: приёмник не работает. Без права читать статистику статус скрыт — это не поломка.
  const down = receiver.status === null && !data.detailsHidden;
  const source = receiver.senderHost === null ? null : receiver.senderPort === null ? receiver.senderHost : `${receiver.senderHost}:${receiver.senderPort}`;
  const pause = replay?.pause ?? null;
  const fields: { id: string; label: string; value: ReactNode | null; hint?: string }[] = [
    ...(down
      ? []
      : [
          { id: "status", label: t("diag.replication.fields.status"), value: receiver.status },
          { id: "source", label: t("diag.replication.fields.source"), value: source },
          { id: "slot", label: t("diag.replication.fields.slot"), value: receiver.slotName },
          { id: "timeline", label: t("diag.replication.fields.timeline"), value: receiver.receivedTli === null ? null : String(receiver.receivedTli) },
          { id: "lastMsg", label: t("diag.replication.fields.lastMsg"), value: receiver.lastMsgAgeS === null ? null : formatDuration(receiver.lastMsgAgeS) },
          // На простаивающем кластере позиция не двигается и это время растёт часами: без пояснения похоже на сбой.
          { id: "lastEnd", label: t("diag.replication.fields.lastEnd"), value: receiver.latestEndAgeS === null ? null : formatDuration(receiver.latestEndAgeS), hint: "hints.diag.lastEnd" },
        ]),
    { id: "gap", label: t("diag.replication.fields.gap"), value: replay?.receiveReplayGapBytes == null ? null : bytesText(replay.receiveReplayGapBytes) },
    { id: "replayDelay", label: t("diag.replication.fields.replayDelay"), value: replay?.replayDelayS == null ? null : formatDuration(replay.replayDelayS) },
    { id: "replayPause", label: t("diag.replication.fields.replayPause"), value: pause === null ? null : PAUSE_KEYS[pause] ? t(PAUSE_KEYS[pause]) : pause },
    { id: "conflicts", label: t("diag.replication.fields.conflicts"), value: conflicts === null ? null : <ConflictsValue c={conflicts} /> },
  ];
  // Скрытое правом не выдаётся прочерком: показывается только известное.
  const shown = fields.filter((f) => f.value !== null || !data.detailsHidden);
  const delay = data.maxStandbyStreamingDelayMs;
  return (
    <section role="region" aria-labelledby={labelId} className="panel">
      <h3 id={labelId}>{t("diag.replication.receiver")}</h3>
      {down && <p className="diag-warn">{t("diag.replication.receiverDown")}</p>}
      <dl className="stats">
        {shown.map((f) => (
          <div key={f.id}>
            <dt>
              {f.label}
              {f.hint && <Hint k={f.hint} label={f.label} />}
            </dt>
            <dd>{f.value ?? "—"}</dd>
          </div>
        ))}
      </dl>
      {data.hotStandbyFeedback !== null && <p className="muted">{t("diag.replication.feedback", { value: data.hotStandbyFeedback ? "on" : "off" })}</p>}
      {delay !== null && (
        <p className="muted">
          {t("diag.replication.maxDelay", {
            value: delay < 0 ? t("diag.replication.unlimited") : delay % 1000 === 0 ? formatDuration(delay / 1000) : `${formatCount(delay)} ms`,
          })}
        </p>
      )}
    </section>
  );
}

function ReplicationBody({ data }: { data: ReplicationData }) {
  const { t } = useTranslation();
  const sendersId = useId();
  // На реплике без каскада отправителей нет и раздел не нужен; на primary «никого нет» — тоже сведение.
  const showSenders = !data.inRecovery || data.senders.length > 0;
  return (
    <div className="diag-view">
      {data.detailsHidden && <p className="diag-note">{t("diag.replication.hidden")}</p>}
      {showSenders && (
        <section>
          <h3 id={sendersId}>{t("diag.replication.senders")}</h3>
          {data.senders.length === 0 ? (
            <p className="muted">{t("diag.replication.none")}</p>
          ) : data.detailsHidden ? (
            // Без pg_read_all_stats в строках только pid: пустые поля были бы похожи на неработающие реплики.
            <p className="muted">{t("diag.replication.hiddenSenders", { count: data.senders.length })}</p>
          ) : (
            <SendersTable senders={data.senders} labelId={sendersId} />
          )}
          {data.synchronousStandbyNames !== null && <p className="muted">{t("diag.replication.syncNames", { value: data.synchronousStandbyNames || "—" })}</p>}
          {data.synchronousCommit !== null && <p className="muted">{t("diag.replication.syncCommit", { value: data.synchronousCommit || "—" })}</p>}
        </section>
      )}
      {data.receiver !== null && <ReceiverCard data={data} receiver={data.receiver} />}
    </div>
  );
}

/**
 * Реплики (спец. P2 §5.6): на primary и каскадной реплике — отправители WAL с лагами по стадиям, настройки синхронной
 * репликации; на реплике — карточка приёмника (источник, разрыв receive и replay, пауза, конфликты запросов).
 */
export function ReplicationView({ report }: { connectionId: string; report: DiagReport }) {
  if (report.data.kind !== "replication") return null;
  return <ReplicationBody data={report.data} />;
}
