import { useId } from "react";
import { useTranslation } from "react-i18next";
import type { DiagReport, PatroniSlot, Slot, SlotsData, Subscription } from "../../api/types";
import { formatCount, formatDuration, formatTimestamp } from "../../lib/format";
import { bytesText, metaText, uniqueKeys } from "./viewText";

/** Отметка Patroni: ключ подписи выбирается по значению, поэтому — литералами. */
const PATRONI_KEYS: Record<PatroniSlot, string> = {
  member: "diag.slots.patroni.member",
  permanent: "diag.slots.patroni.permanent",
};

/** WAL, который сервер уже не гарантирует (`unreserved`) или потерял (`lost`), выделяется цветом, но состояние названо и словом. */
const walStatusClass = (s: string | null) => (s === "lost" || s === "unreserved" ? "diag-warn" : undefined);

/** Отметки слота значками: только те, что сервер подтвердил (`false` и `null` — не отметка). */
function SlotFlags({ s }: { s: Slot }) {
  const { t } = useTranslation();
  const flags = [
    s.temporary && t("diag.slots.flags.temporary"),
    s.failover === true && t("diag.slots.flags.failover"),
    s.synced === true && t("diag.slots.flags.synced"),
    s.twoPhase === true && t("diag.slots.flags.twoPhase"),
    s.conflicting === true && t("diag.slots.flags.conflicting"),
    s.invalidationReason !== null && t("diag.slots.flags.invalidated", { reason: s.invalidationReason }),
  ].filter((f): f is string => f !== false);
  if (flags.length === 0) return null;
  return (
    <span className="badges">
      {flags.map((f) => (
        <span key={f} className="kind-badge">
          {f}
        </span>
      ))}
    </span>
  );
}

function SlotRow({ s, patroni }: { s: Slot; patroni: boolean }) {
  const { t } = useTranslation();
  const state = s.active ? (s.activePid === null ? t("diag.slots.active") : t("diag.slots.activePid", { pid: s.activePid })) : t("diag.slots.inactive");
  return (
    <tr>
      <th scope="row">{s.name}</th>
      <td>{metaText([s.slotType, s.plugin])}</td>
      <td>{s.database ?? "—"}</td>
      <td>{state}</td>
      <td className={walStatusClass(s.walStatus)}>{s.walStatus ?? "—"}</td>
      <td className="num">{bytesText(s.retainedBytes)}</td>
      <td className="num">{bytesText(s.safeWalSize)}</td>
      <td className="num">{formatCount(s.xminAge)}</td>
      <td className="num">{formatCount(s.catalogXminAge)}</td>
      <td className="num">{formatTimestamp(s.inactiveSinceMs)}</td>
      <td>
        <SlotFlags s={s} />
      </td>
      {patroni && <td>{s.patroni === null ? "" : t(PATRONI_KEYS[s.patroni])}</td>}
    </tr>
  );
}

function SlotsTable({ slots, labelId }: { slots: Slot[]; labelId: string }) {
  const { t } = useTranslation();
  // Столбец Patroni нужен, только если сервер хоть у одного слота поставил отметку (подключение `patroni`).
  const patroni = slots.some((s) => s.patroni !== null);
  return (
    <div className="table-scroll">
      <table className="diag-table" aria-labelledby={labelId}>
        <thead>
          <tr>
            <th scope="col">{t("diag.slots.columns.name")}</th>
            <th scope="col">{t("diag.slots.columns.type")}</th>
            <th scope="col">{t("diag.slots.columns.database")}</th>
            <th scope="col">{t("diag.slots.columns.state")}</th>
            <th scope="col">{t("diag.slots.columns.walStatus")}</th>
            <th scope="col">{t("diag.slots.columns.retained")}</th>
            <th scope="col">{t("diag.slots.columns.safeWal")}</th>
            <th scope="col">{t("diag.slots.columns.xminAge")}</th>
            <th scope="col">{t("diag.slots.columns.catalogXminAge")}</th>
            <th scope="col">{t("diag.slots.columns.inactiveSince")}</th>
            <th scope="col">{t("diag.slots.columns.flags")}</th>
            {patroni && <th scope="col">{t("diag.slots.columns.patroni")}</th>}
          </tr>
        </thead>
        <tbody>
          {slots.map((s) => (
            // Имя слота уникально в кластере.
            <SlotRow key={s.name} s={s} patroni={patroni} />
          ))}
        </tbody>
      </table>
    </div>
  );
}

function SubscriptionsTable({ subscriptions, labelId }: { subscriptions: Subscription[]; labelId: string }) {
  const { t } = useTranslation();
  // Имя подписки уникально только в своей базе, а отчёт перечисляет подписки всех баз: повторы нумеруются.
  const keys = uniqueKeys(subscriptions.map((s) => s.name));
  return (
    <div className="table-scroll">
      <table className="diag-table" aria-labelledby={labelId}>
        <thead>
          <tr>
            <th scope="col">{t("diag.slots.subColumns.name")}</th>
            <th scope="col">{t("diag.slots.subColumns.worker")}</th>
            <th scope="col">{t("diag.slots.subColumns.receivedLsn")}</th>
            <th scope="col">{t("diag.slots.subColumns.lastMsg")}</th>
            <th scope="col">{t("diag.slots.subColumns.applyErrors")}</th>
            <th scope="col">{t("diag.slots.subColumns.syncErrors")}</th>
          </tr>
        </thead>
        <tbody>
          {subscriptions.map((s, i) => (
            <tr key={keys[i]}>
              <th scope="row">{s.name}</th>
              {/* Нет процесса применения — подписка не работает. */}
              <td className={s.pid === null ? "diag-warn" : "num"}>{s.pid === null ? t("diag.slots.noWorker") : s.pid}</td>
              <td>{s.receivedLsn ?? "—"}</td>
              <td className="num">{formatDuration(s.lastMsgAgeS)}</td>
              <td className="num">{formatCount(s.applyErrorCount)}</td>
              <td className="num">{formatCount(s.syncErrorCount)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function SlotsBody({ data }: { data: SlotsData }) {
  const { t } = useTranslation();
  const slotsId = useId();
  const subsId = useId();
  const keep = data.maxSlotWalKeepSizeMb;
  return (
    <div className="diag-view">
      <section>
        <h3 id={slotsId}>{t("diag.slots.table")}</h3>
        {data.slots.length === 0 ? <p className="muted">{t("diag.slots.none")}</p> : <SlotsTable slots={data.slots} labelId={slotsId} />}
        {keep !== null && (
          // Мегабайты из настройки; -1 — слоты могут удерживать WAL без предела.
          <p className="muted">{t("diag.slots.keepSize", { value: keep < 0 ? t("diag.slots.unlimited") : bytesText(keep * 1024 * 1024) })}</p>
        )}
      </section>
      {data.subscriptions.length > 0 && (
        <section>
          <h3 id={subsId}>{t("diag.slots.subscriptions")}</h3>
          <SubscriptionsTable subscriptions={data.subscriptions} labelId={subsId} />
          {data.subscriptionErrorsGrowth !== null && <p className="muted">{t("diag.slots.errorsGrowth", { value: `+${formatCount(data.subscriptionErrorsGrowth)}` })}</p>}
        </section>
      )}
    </div>
  );
}

/**
 * Слоты репликации и подписки (спец. P2 §5.7): сколько WAL держит каждый слот и в каком он состоянии, отметка слотов
 * Patroni (участник или постоянный), `max_slot_wal_keep_size`; ниже — подписки логической репликации.
 */
export function SlotsView({ report }: { connectionId: string; report: DiagReport }) {
  if (report.data.kind !== "slots") return null;
  return <SlotsBody data={report.data} />;
}
