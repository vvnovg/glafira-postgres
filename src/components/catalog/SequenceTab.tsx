import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { SequenceDetails } from "../../api/types";
import { formatInt64, formatPercent } from "../../lib/format";
import { KeyValue } from "./KeyValue";
import { ObjectTab } from "./ObjectTab";

function SequenceBody({ d }: { d: SequenceDetails }) {
  const { t } = useTranslation();
  // Без права SELECT/USAGE каталог отдаёт last_value пустым при любом использовании: прочерк означал бы
  // «ещё не использовалась», поэтому причина называется словами.
  const unreadable = !d.readable ? t("object.noPrivilege") : null;
  return (
    <KeyValue
      items={[
        { label: t("object.dataType"), value: d.dataType },
        { label: t("object.lastValue"), value: unreadable ?? (d.lastValue === null ? null : formatInt64(d.lastValue)) },
        { label: t("object.percentUsed"), value: unreadable ?? (d.percentUsed === null ? null : formatPercent(d.percentUsed)) },
        { label: t("object.start"), value: formatInt64(d.start) },
        { label: t("object.min"), value: formatInt64(d.min) },
        { label: t("object.max"), value: formatInt64(d.max) },
        { label: t("object.increment"), value: formatInt64(d.increment) },
        { label: t("object.cycle"), value: d.cycle ? t("object.yes") : t("object.no") },
        { label: t("object.cache"), value: formatInt64(d.cache) },
        { label: t("object.ownedBy"), value: d.ownedBy === null ? null : <code>{d.ownedBy}</code> },
      ]}
    />
  );
}

/** Вкладка последовательности (спец. P1 §9): тип, последнее значение, границы, шаг, доля использованного диапазона, владеющая колонка. */
export function SequenceTab({ connectionId, database, schema, name }: { connectionId: string; database: string; schema: string; name: string }) {
  const { t } = useTranslation();
  const query = useQuery({
    queryKey: ["catalog", connectionId, "sequence", database, schema, name],
    queryFn: () => api.getSequence(connectionId, database, schema, name),
  });
  return (
    <ObjectTab connectionId={connectionId} title={`${schema}.${name}`} kind={t("object.kinds.sequence")} query={query}>
      {(d) => <SequenceBody d={d} />}
    </ObjectTab>
  );
}
