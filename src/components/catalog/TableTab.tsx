import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { RelationRef, TableDetails } from "../../api/types";
import { formatBytes, formatCount, formatTimestamp } from "../../lib/format";
import { useWorkspace } from "../../state/workspace";
import { DdlView } from "./DdlView";
import { KeyValue } from "./KeyValue";
import { ObjectTab } from "./ObjectTab";
import { labelOr } from "./labels";
import { ColumnsTable, IndexesTable, ScrollTable } from "./RelationTables";
import { SubTabs } from "./SubTabs";

type Section = "columns" | "indexes" | "constraints" | "stats" | "partitioning" | "triggers" | "ddl";

/** Вид таблицы для подписи: партиция, партиционированная, внешняя или обычная. */
function kindKey(d: TableDetails | undefined): string {
  if (!d) return "table";
  if (d.partitioning.parent !== null) return "partition";
  if (d.relkind === "p") return "partitionedTable";
  if (d.relkind === "f") return "foreignTable";
  return "table";
}

/** Счётчик строкой; `null` (нет данных) остаётся `null` — `KeyValue` покажет прочерк. */
const count = (n: number | null) => (n === null ? null : formatCount(n));

function ConstraintsPane({ d }: { d: TableDetails }) {
  const { t, i18n } = useTranslation();
  return (
    <>
      {d.constraints.length === 0 ? (
        <p className="empty">{t("object.empty")}</p>
      ) : (
        <ScrollTable label={t("object.constraints")}>
          <thead>
            <tr>
              <th scope="col">{t("object.name")}</th>
              <th scope="col">{t("object.type")}</th>
              <th scope="col">{t("object.definition")}</th>
            </tr>
          </thead>
          <tbody>
            {d.constraints.map((c) => (
              <tr key={c.name}>
                <td>{c.name}</td>
                {/* Вид неизвестной версии показывается кодом, а не пустой ячейкой. */}
                <td>{labelOr(i18n, t, `object.constraintKinds.${c.kind}`, c.kind)}</td>
                <td>
                  <code>{c.definition}</code>{" "}
                  {!c.validated && <span className="kind-badge invalid">{t("object.notValid")}</span>}
                </td>
              </tr>
            ))}
          </tbody>
        </ScrollTable>
      )}
      <h3>{t("object.incomingFks")}</h3>
      {d.incomingFks.length === 0 ? (
        <p className="empty">{t("object.empty")}</p>
      ) : (
        <ScrollTable label={t("object.incomingFks")}>
          <thead>
            <tr>
              <th scope="col">{t("object.name")}</th>
              <th scope="col">{t("object.referencingTable")}</th>
              <th scope="col">{t("object.definition")}</th>
            </tr>
          </thead>
          <tbody>
            {d.incomingFks.map((fk) => (
              <tr key={`${fk.schema}.${fk.table}.${fk.constraint}`}>
                <td>{fk.constraint}</td>
                <td>
                  {fk.schema}.{fk.table}
                </td>
                <td>
                  <code>{fk.definition}</code>
                </td>
              </tr>
            ))}
          </tbody>
        </ScrollTable>
      )}
    </>
  );
}

function StatsPane({ d }: { d: TableDetails }) {
  const { t } = useTranslation();
  const s = d.stats;
  return (
    <>
      {/* У партиционированной таблицы цифры — по всему дереву партиций (отступление 7 плана P1b). */}
      {d.relkind === "p" && <p className="muted">{t("object.partitionedStatsNote")}</p>}
      <KeyValue
        items={[
          { label: t("object.heap"), value: formatBytes(s.heapBytes) },
          { label: t("object.indexesSize"), value: formatBytes(s.indexBytes) },
          { label: t("object.toast"), value: formatBytes(s.toastBytes) },
          { label: t("object.total"), value: formatBytes(s.totalBytes) },
          // -1 — таблицу ещё не анализировали: оценки нет.
          { label: t("object.reltuples"), value: s.reltuples < 0 ? null : formatCount(Math.round(s.reltuples)) },
          { label: t("object.seqScan"), value: count(s.seqScan) },
          { label: t("object.idxScan"), value: count(s.idxScan) },
          { label: t("object.liveTup"), value: count(s.nLiveTup) },
          { label: t("object.deadTup"), value: count(s.nDeadTup) },
          { label: t("object.modSinceAnalyze"), value: count(s.nModSinceAnalyze) },
          { label: t("object.insSinceVacuum"), value: count(s.nInsSinceVacuum) },
          { label: t("object.lastVacuum"), value: formatTimestamp(s.lastVacuumMs) },
          { label: t("object.lastAutovacuum"), value: formatTimestamp(s.lastAutovacuumMs) },
          { label: t("object.lastAnalyze"), value: formatTimestamp(s.lastAnalyzeMs) },
          { label: t("object.lastAutoanalyze"), value: formatTimestamp(s.lastAutoanalyzeMs) },
          { label: t("object.vacuumCount"), value: count(s.vacuumCount) },
          { label: t("object.autovacuumCount"), value: count(s.autovacuumCount) },
          { label: t("object.analyzeCount"), value: count(s.analyzeCount) },
          { label: t("object.autoanalyzeCount"), value: count(s.autoanalyzeCount) },
          {
            label: t("object.reloptions"),
            value:
              d.reloptions.length === 0 ? null : (
                <ul className="plain-list">
                  {d.reloptions.map((o) => (
                    <li key={o}>
                      <code>{o}</code>
                    </li>
                  ))}
                </ul>
              ),
          },
          { label: t("object.tablespace"), value: d.tablespace },
          { label: t("object.owner"), value: d.owner },
        ]}
      />
    </>
  );
}

/** Ссылка на таблицу в той же базе: имя `schema.name` как в каталоге (без кавычек), клик открывает её вкладку. */
function RelationLink({ r, connectionId, database }: { r: RelationRef; connectionId: string; database: string }) {
  const open = useWorkspace((s) => s.open);
  return (
    <button type="button" className="link" onClick={() => open({ kind: "table", connectionId, database, schema: r.schema, name: r.name })}>
      <code>{`${r.schema}.${r.name}`}</code>
    </button>
  );
}

function PartitioningPane({ d, connectionId, database }: { d: TableDetails; connectionId: string; database: string }) {
  const { t } = useTranslation();
  const p = d.partitioning;
  const code = (v: string | null) => (v === null ? null : <code>{v}</code>);
  return (
    <KeyValue
      items={[
        { label: t("object.partitionKey"), value: code(p.key) },
        { label: t("object.partitionBound"), value: code(p.bound) },
        { label: t("object.parent"), value: p.parent === null ? null : <RelationLink r={p.parent} connectionId={connectionId} database={database} /> },
        {
          label: t("object.partitions"),
          value:
            p.partitions.length === 0 ? null : (
              <ul className="plain-list">
                {p.partitions.map((r) => (
                  <li key={`${r.schema}.${r.name}`}>
                    <RelationLink r={r} connectionId={connectionId} database={database} />
                  </li>
                ))}
              </ul>
            ),
        },
      ]}
    />
  );
}

function TriggersPane({ d }: { d: TableDetails }) {
  const { t, i18n } = useTranslation();
  if (d.triggers.length === 0) return <p className="empty">{t("object.empty")}</p>;
  return (
    <ScrollTable label={t("object.triggers")}>
      <thead>
        <tr>
          <th scope="col">{t("object.name")}</th>
          <th scope="col">{t("object.enabled")}</th>
          <th scope="col">{t("object.definition")}</th>
        </tr>
      </thead>
      <tbody>
        {d.triggers.map((trg) => (
          <tr key={trg.name}>
            <td>{trg.name}</td>
            <td>{labelOr(i18n, t, `object.triggerState.${trg.enabled}`, trg.enabled)}</td>
            <td>
              <code>{trg.definition}</code>
            </td>
          </tr>
        ))}
      </tbody>
    </ScrollTable>
  );
}

function TableBody({ d, connectionId, database }: { d: TableDetails; connectionId: string; database: string }) {
  const { t } = useTranslation();
  const [section, setSection] = useState<Section>("columns");
  const tabs: { id: Section; label: string }[] = [
    { id: "columns", label: t("object.columns") },
    { id: "indexes", label: t("object.indexes") },
    { id: "constraints", label: t("object.constraints") },
    { id: "stats", label: t("object.stats") },
    // Только у партиционированных таблиц и у партиций.
    ...(d.relkind === "p" || d.partitioning.parent !== null ? [{ id: "partitioning" as const, label: t("object.partitioning") }] : []),
    { id: "triggers", label: t("object.triggers") },
    { id: "ddl", label: t("object.ddl") },
  ];
  return (
    <>
      {d.comment && <p>{d.comment}</p>}
      {d.unlogged && (
        <p>
          <span className="kind-badge">{t("object.unlogged")}</span> <span className="muted">{t("object.unloggedHint")}</span>
        </p>
      )}
      <SubTabs label={t("object.sections")} tabs={tabs} value={section} onChange={setSection}>
        {(active) => (
          <>
            {active === "columns" && <ColumnsTable columns={d.columns} />}
            {active === "indexes" && <IndexesTable indexes={d.indexes} />}
            {active === "constraints" && <ConstraintsPane d={d} />}
            {active === "stats" && <StatsPane d={d} />}
            {active === "partitioning" && <PartitioningPane d={d} connectionId={connectionId} database={database} />}
            {active === "triggers" && <TriggersPane d={d} />}
            {active === "ddl" && <DdlView ddl={d.ddl} />}
          </>
        )}
      </SubTabs>
    </>
  );
}

/** Вкладка таблицы (спец. P1 §9): колонки, индексы, ограничения, размер и статистика, партиционирование, триггеры, DDL. */
export function TableTab({ connectionId, database, schema, name }: { connectionId: string; database: string; schema: string; name: string }) {
  const { t } = useTranslation();
  // Ключ начинается с "catalog" и подключения: отключение и смена узла каталога сбрасывают его вместе с деревом (PG_QUERY_KEYS).
  const query = useQuery({
    queryKey: ["catalog", connectionId, "table", database, schema, name],
    queryFn: () => api.getTable(connectionId, database, schema, name),
  });
  return (
    <ObjectTab connectionId={connectionId} title={`${schema}.${name}`} kind={t(`object.kinds.${kindKey(query.data)}`)} query={query}>
      {(d) => <TableBody d={d} connectionId={connectionId} database={database} />}
    </ObjectTab>
  );
}
