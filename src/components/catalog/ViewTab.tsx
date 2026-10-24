import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { ViewDetails } from "../../api/types";
import { formatBytes } from "../../lib/format";
import { DdlView } from "./DdlView";
import { KeyValue } from "./KeyValue";
import { ObjectTab } from "./ObjectTab";
import { ColumnsTable, IndexesTable } from "./RelationTables";
import { SubTabs } from "./SubTabs";

type Section = "definition" | "columns" | "indexes";

function ViewBody({ d }: { d: ViewDetails }) {
  const { t } = useTranslation();
  const [section, setSection] = useState<Section>("definition");
  const tabs: { id: Section; label: string }[] = [
    { id: "definition", label: t("object.definition") },
    { id: "columns", label: t("object.columns") },
    // Индексы бывают только у материализованного представления.
    ...(d.matview ? [{ id: "indexes" as const, label: t("object.indexes") }] : []),
  ];
  return (
    <>
      {d.comment && <p>{d.comment}</p>}
      <KeyValue
        items={[
          { label: t("object.owner"), value: d.owner },
          ...(d.matview
            ? [
                { label: t("object.populated"), value: d.populated === null ? null : d.populated ? t("object.yes") : t("object.no") },
                { label: t("object.size"), value: d.sizeBytes === null ? null : formatBytes(d.sizeBytes) },
              ]
            : []),
        ]}
      />
      <SubTabs label={t("object.sections")} tabs={tabs} value={section} onChange={setSection}>
        {section === "definition" && <DdlView ddl={d.definition} title={t("object.definition")} copyLabel={t("object.copyDefinition")} />}
        {section === "columns" && <ColumnsTable columns={d.columns} />}
        {section === "indexes" && <IndexesTable indexes={d.indexes} />}
      </SubTabs>
    </>
  );
}

/** Вкладка представления и материализованного представления (спец. P1 §9): определение, колонки, у материализованного — заполненность, размер, индексы. */
export function ViewTab({
  connectionId,
  database,
  schema,
  name,
  matview,
}: {
  connectionId: string;
  database: string;
  schema: string;
  name: string;
  matview: boolean;
}) {
  const { t } = useTranslation();
  const query = useQuery({
    queryKey: ["catalog", connectionId, "view", database, schema, name],
    queryFn: () => api.getView(connectionId, database, schema, name),
  });
  // Пока данных нет, вид известен по списку, из которого открыта вкладка; потом — по ответу.
  const isMat = query.data?.matview ?? matview;
  return (
    <ObjectTab title={`${schema}.${name}`} kind={t(isMat ? "object.kinds.matview" : "object.kinds.view")} query={query}>
      {(d) => <ViewBody d={d} />}
    </ObjectTab>
  );
}
