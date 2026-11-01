import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { Comparison, SnapshotInfo } from "../../api/types";
import { formatBytes, formatCount, formatTimestamp } from "../../lib/format";
import { ErrorBanner } from "../common/ErrorBanner";
import { SnapshotCompare } from "./SnapshotCompare";

/** Подписи нечитаемых снимков; ключи — литералами (правило словарей). */
const UNREADABLE: Record<NonNullable<SnapshotInfo["unreadable"]>, string> = {
  tooLarge: "diag.snapshots.unreadable.tooLarge",
  corrupt: "diag.snapshots.unreadable.corrupt",
};

export const snapshotsKey = (connectionId: string) => ["snapshots", connectionId] as const;

/**
 * Снимки pgss подключения (спец. P3 §9.3): список (новые первыми), «Сделать снимок» для узла инструмента, выбор двух
 * снимков одного узла и «Сравнить», удаление с подтверждением. Нечитаемый файл (слишком большой или повреждённый) в
 * списке есть, но выбрать его нельзя — только удалить. Пара передаётся старший → младший (Rust принимает и обратный порядок).
 */
export function SnapshotsPanel({
  connectionId,
  member,
  onBetween,
  onDeleted,
}: {
  connectionId: string;
  /** Узел для нового снимка; `null` — узел ещё не известен, снимать нечего. */
  member: string | null;
  /** Пара старший → младший; сведения о снимках нужны подписи режима (время, узел). */
  onBetween: (from: SnapshotInfo, to: SnapshotInfo) => void;
  onDeleted?: (file: string) => void;
}) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const key = snapshotsKey(connectionId);
  const q = useQuery({ queryKey: key, queryFn: () => api.snapshotList(connectionId) });
  const rows = q.data ?? [];
  const [pickedRaw, setPicked] = useState<string[]>([]);
  // Выбранные — только те, что ещё есть в списке и читаются.
  const picked = pickedRaw.filter((f) => rows.some((r) => r.file === f && !r.unreadable));
  const [confirmDelete, setConfirmDelete] = useState<SnapshotInfo | null>(null);
  const [compared, setCompared] = useState<{ from: SnapshotInfo; to: SnapshotInfo; data: Comparison } | null>(null);
  const take = useMutation({
    mutationFn: () => api.snapshotTake(connectionId, member!),
    onSuccess: () => void qc.invalidateQueries({ queryKey: key }),
  });
  const del = useMutation({
    mutationFn: (file: string) => api.snapshotDelete(connectionId, file),
    onSuccess: (_gone, file) => {
      setConfirmDelete(null);
      setPicked((p) => p.filter((f) => f !== file));
      setCompared((c) => (c && (c.from.file === file || c.to.file === file) ? null : c));
      onDeleted?.(file);
      void qc.invalidateQueries({ queryKey: key });
    },
  });
  const compare = useMutation({
    mutationFn: ({ from, to }: { from: SnapshotInfo; to: SnapshotInfo }) => api.snapshotCompare(connectionId, from.file, to.file),
    onSuccess: (data, { from, to }) => setCompared({ from, to, data }),
  });
  const toggle = (file: string) => setPicked(() => (picked.includes(file) ? picked.filter((f) => f !== file) : [...picked.slice(-1), file]));
  const pickedInfo = picked.map((f) => rows.find((r) => r.file === f)!);
  const mixed = pickedInfo.length === 2 && pickedInfo[0].member !== pickedInfo[1].member;
  // Старший снимок — `from`, младший — `to`.
  const pair = pickedInfo.length === 2 && !mixed ? (pickedInfo[0].takenAtMs <= pickedInfo[1].takenAtMs ? pickedInfo : [pickedInfo[1], pickedInfo[0]]) : null;
  const error = q.error ?? take.error ?? del.error ?? compare.error;
  // Подтверждение получает фокус: Escape и Enter работают сразу, без щелчка по нему.
  const confirmRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (confirmDelete) confirmRef.current?.focus();
  }, [confirmDelete]);
  const onConfirmKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && !del.isPending) setConfirmDelete(null);
  };

  return (
    <section className="snapshots-panel" aria-label={t("diag.snapshots.title")}>
      <div className="toolbar">
        <h3>{t("diag.snapshots.title")}</h3>
        <button type="button" disabled={member === null || take.isPending} onClick={() => take.mutate()}>
          {t("diag.snapshots.take")}
        </button>
        <button type="button" disabled={pair === null || compare.isPending} onClick={() => pair && compare.mutate({ from: pair[0], to: pair[1] })}>
          {t("diag.snapshots.compare")}
        </button>
        <span className="muted">{t("diag.snapshots.hint")}</span>
      </div>
      {error && <ErrorBanner error={error} />}
      {mixed && <p className="diag-note">{t("diag.snapshots.mixedMembers")}</p>}
      {rows.length === 0 && q.isSuccess && <p className="muted">{t("diag.snapshots.empty")}</p>}
      {rows.length > 0 && (
        <div className="table-scroll">
          <table className="diag-table" aria-label={t("diag.snapshots.title")}>
            <thead>
              <tr>
                <th scope="col" />
                <th scope="col">{t("diag.snapshots.takenAt")}</th>
                <th scope="col">{t("diag.header.node")}</th>
                <th scope="col" className="num">
                  {t("diag.snapshots.rows")}
                </th>
                <th scope="col" className="num">
                  {t("diag.snapshots.size")}
                </th>
                <th scope="col" />
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.file} className={r.unreadable ? "snapshot-unreadable" : undefined}>
                  <td>
                    <input
                      type="checkbox"
                      aria-label={t("diag.snapshots.pick", { at: formatTimestamp(r.takenAtMs) })}
                      checked={picked.includes(r.file)}
                      disabled={r.unreadable !== undefined}
                      title={r.unreadable ? t("diag.snapshots.unreadableHint") : undefined}
                      onChange={() => toggle(r.file)}
                    />
                  </td>
                  <td>{formatTimestamp(r.takenAtMs)}</td>
                  <td>{r.member}</td>
                  <td className="num">
                    {r.unreadable && (
                      <span className="kind-badge" title={t("diag.snapshots.unreadableHint")}>
                        {t(UNREADABLE[r.unreadable])}
                      </span>
                    )}{" "}
                    {formatCount(r.rows)}
                  </td>
                  <td className="num">{formatBytes(r.bytes)}</td>
                  <td>
                    <button type="button" className="link" onClick={() => setConfirmDelete(r)}>
                      {t("common.delete")}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {confirmDelete && (
        <div className="confirm" role="group" aria-label={t("diag.snapshots.deleteTitle")} onKeyDown={onConfirmKey}>
          <strong>{t("diag.snapshots.deleteTitle")}</strong>
          <p>{t("diag.snapshots.deleteQuestion", { at: formatTimestamp(confirmDelete.takenAtMs) })}</p>
          <div className="toolbar">
            <button ref={confirmRef} type="button" className="danger" disabled={del.isPending} onClick={() => del.mutate(confirmDelete.file)}>
              {t("diag.snapshots.deleteConfirm")}
            </button>
            <button type="button" disabled={del.isPending} onClick={() => setConfirmDelete(null)}>
              {t("common.cancel")}
            </button>
          </div>
        </div>
      )}
      {compared && (
        <SnapshotCompare
          comparison={compared.data}
          title={t("diag.snapshots.compared", { from: formatTimestamp(compared.from.takenAtMs), to: formatTimestamp(compared.to.takenAtMs) })}
          onBetween={() => onBetween(compared.from, compared.to)}
        />
      )}
    </section>
  );
}
