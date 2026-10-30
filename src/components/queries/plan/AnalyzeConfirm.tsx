import { useTranslation } from "react-i18next";
import type { Confirmation, ConnectionProfile, RelationRef } from "../../../api/types";
import { ConfirmDialog } from "../../common/ConfirmDialog";

/**
 * Подтверждение яруса записи (спец. P3 §8.3 п. 4): таблицы из пробного плана и что откат не отменяет. Показывается
 * всегда, а не по `needsConfirmation`: на не-prod без имени объекта — кнопка без полей ввода, на prod — ввод имени
 * подключения (его проверит и Rust).
 */
export function AnalyzeConfirm({ connection, relations, admin, busy, onConfirm, onCancel }: { connection: ConnectionProfile; relations: RelationRef[]; admin?: boolean; busy?: boolean; onConfirm: (c: Confirmation) => void; onCancel: () => void }) {
  const { t } = useTranslation();
  return (
    <ConfirmDialog connection={connection} title={t("diag.plan.confirm.title")} confirmLabel={t("diag.plan.confirm.run")} danger busy={busy} onConfirm={onConfirm} onCancel={onCancel}>
      {/* Ярус записи из-за административной функции (`adminFunction`): откат её действие не отменит. */}
      {admin && <p className="diag-warn">{t("diag.plan.confirm.admin")}</p>}
      <p>{t("diag.plan.confirm.rollback")}</p>
      {/* Таблиц нет («как запись» после отказа самой пробы — плана нет): строки «Таблицы:» тоже нет. */}
      {relations.length > 0 && <p>{t("diag.plan.confirm.tables")}</p>}
      {relations.length > 0 && (
        <ul className="plain-list">
          {relations.map((r) => (
            <li key={`${r.schema ?? ""}.${r.name}`}>
              <code>{r.schema ? `${r.schema}.${r.name}` : r.name}</code>
            </li>
          ))}
        </ul>
      )}
      <p className="muted">{t("diag.plan.confirm.sideEffects")}</p>
    </ConfirmDialog>
  );
}
