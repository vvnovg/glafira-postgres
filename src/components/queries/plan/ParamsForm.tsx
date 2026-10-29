import { useTranslation } from "react-i18next";

/**
 * Поля значений `$1…$N`; пустое поле — NULL. На 16+ «План» может идти с GENERIC_PLAN вместо значений; поля остаются на
 * экране: GENERIC_PLAN несовместим с ANALYZE, и «План с выполнением» всегда берёт значения (спец. P3 §8.2).
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
  return (
    <fieldset className="params-form">
      <legend>{t("diag.plan.params.title", { count })}</legend>
      {canGeneric && (
        <label className="inline">
          <input type="checkbox" checked={generic} onChange={(e) => onGeneric(e.target.checked)} />
          <span>{t("diag.plan.params.generic")}</span>
        </label>
      )}
      {Array.from({ length: count }, (_, i) => (
        <label key={i} className="inline">
          <span className="mono">${i + 1}</span>
          <input type="text" aria-label={`$${i + 1}`} value={values[i] ?? ""} placeholder={t("diag.plan.params.nullPlaceholder")} onChange={(e) => set(i, e.target.value)} />
        </label>
      ))}
    </fieldset>
  );
}
