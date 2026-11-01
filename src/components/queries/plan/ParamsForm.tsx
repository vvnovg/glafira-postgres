import { useTranslation } from "react-i18next";

/** Полей значений не больше: нормализованный `INSERT … VALUES (…), (…)` даёт тысячи `$N`, и форма заморозила бы окно. */
export const PARAM_FIELDS_LIMIT = 100;

/**
 * Поля значений `$1…$N`; пустое поле — NULL. На 16+ «План» может идти с GENERIC_PLAN вместо значений; поля остаются на
 * экране: GENERIC_PLAN несовместим с ANALYZE, и «План с выполнением» всегда берёт значения (спец. P3 §8.2) — без лишнего
 * шага со снятием флажка. Параметров больше `PARAM_FIELDS_LIMIT` — вместо полей сообщение: запуски со значениями
 * недоступны.
 */
export function ParamsForm({
  count,
  values,
  onChange,
  generic,
  canGeneric,
  onGeneric,
}: {
  count: number;
  values: (string | null)[];
  onChange: (v: (string | null)[]) => void;
  generic: boolean;
  canGeneric: boolean;
  onGeneric: (g: boolean) => void;
}) {
  const { t } = useTranslation();
  if (count === 0) return null;
  const set = (i: number, v: string) => {
    const next = Array.from({ length: count }, (_, j) => values[j] ?? null);
    next[i] = v === "" ? null : v;
    onChange(next);
  };
  const tooMany = count > PARAM_FIELDS_LIMIT;
  return (
    <fieldset className="params-form">
      <legend>{t("diag.plan.params.title", { count })}</legend>
      {canGeneric && (
        <label className="inline generic">
          <input type="checkbox" checked={generic} onChange={(e) => onGeneric(e.target.checked)} />
          <span>{t("diag.plan.params.generic")}</span>
        </label>
      )}
      {tooMany ? (
        <p className="error-banner" role="status">
          {t("diag.plan.params.tooMany", { count, max: PARAM_FIELDS_LIMIT })}
        </p>
      ) : (
        <>
          {canGeneric && generic && <p className="muted">{t("diag.plan.params.genericHint")}</p>}
          {Array.from({ length: count }, (_, i) => (
            <label key={i} className="inline">
              <span className="mono">${i + 1}</span>
              <input type="text" aria-label={`$${i + 1}`} value={values[i] ?? ""} placeholder={t("diag.plan.params.nullPlaceholder")} onChange={(e) => set(i, e.target.value)} />
            </label>
          ))}
        </>
      )}
    </fieldset>
  );
}
