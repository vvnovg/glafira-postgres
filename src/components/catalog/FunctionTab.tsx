import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { FunctionDetails } from "../../api/types";
import { DdlView } from "./DdlView";
import { KeyValue } from "./KeyValue";
import { labelOr } from "./labels";
import { ObjectTab } from "./ObjectTab";

const KIND_KEY = { f: "function", p: "procedure", a: "aggregate", w: "windowFunction" } as const;

function FunctionBody({ d }: { d: FunctionDetails }) {
  const { t, i18n } = useTranslation();
  return (
    <>
      {d.comment && <p>{d.comment}</p>}
      <KeyValue
        items={[
          {
            label: t("object.signature"),
            value: (
              <code>
                {d.schema}.{d.name}({d.signature})
              </code>
            ),
          },
          { label: t("object.returns"), value: d.returns === null ? null : <code>{d.returns}</code> },
          { label: t("object.language"), value: d.language },
          { label: t("object.volatilityLabel"), value: labelOr(i18n, t, `object.volatility.${d.volatility}`, d.volatility) },
          { label: t("object.securityDefiner"), value: d.securityDefiner ? t("object.yes") : t("object.no") },
          { label: t("object.owner"), value: d.owner },
        ]}
      />
      <h3>{t("object.definition")}</h3>
      {d.definition === null ? (
        // Определение агрегата функция каталога не строит — остаётся сигнатура.
        <p className="muted">{t("object.signatureOnly")}</p>
      ) : (
        <DdlView ddl={d.definition} title={t("object.definition")} copyLabel={t("object.copyDefinition")} />
      )}
    </>
  );
}

/** Вкладка функции, процедуры, агрегата (спец. P1 §9): сигнатура, язык, изменчивость, SECURITY DEFINER, определение. */
export function FunctionTab({ connectionId, database, oid, name }: { connectionId: string; database: string; oid: number; name: string }) {
  const { t } = useTranslation();
  const query = useQuery({
    queryKey: ["catalog", connectionId, "function", database, oid],
    queryFn: () => api.getFunction(connectionId, database, oid),
  });
  return (
    <ObjectTab title={name} kind={t(`object.kinds.${KIND_KEY[query.data?.kind ?? "f"] ?? "function"}`)} query={query}>
      {(d) => <FunctionBody d={d} />}
    </ObjectTab>
  );
}
