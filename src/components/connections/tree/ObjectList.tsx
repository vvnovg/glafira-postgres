import { keepPreviousData, useInfiniteQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { type HTMLAttributes, type Ref, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/client";
import type { ObjectItem, ObjectKind, ObjectPage } from "../../../api/types";
import { formatBytes, formatCount } from "../../../lib/format";
import { Glyph } from "../../common/Glyph";
import { useDebouncedValue } from "../../../lib/useDebouncedValue";
import { type TabSpec, useWorkspace } from "../../../state/workspace";
import { ErrorBanner } from "../../common/ErrorBanner";

/** Размер страницы списка объектов (в спецификации — 500; предел команды Rust — 1000). */
export const PAGE = 500;
/** С какого числа строк список виртуализируется. */
const VIRTUAL_FROM = 200;
const ROW_HEIGHT = 22;
/** За сколько строк до конца видимой области подгружается следующая страница. */
const PREFETCH_AHEAD = 50;
/** Поле фильтра — у групп больше этого числа объектов. */
const FILTER_FROM = 20;

/** Атрибуты строки виртуального списка: позиция и замер высоты (раскрытые партиции меняют её). */
type RowAttrs = HTMLAttributes<HTMLLIElement> & { ref?: Ref<HTMLLIElement>; "data-index"?: number };

/**
 * Объекты со всех загруженных страниц без повторов. Страницы берутся по смещению, а список между запросами
 * меняется (создали или удалили таблицу) — соседние страницы могут перекрыться, и один oid встретился бы дважды:
 * ключи React и ключи размеров виртуализатора должны быть уникальны. Остаётся первое вхождение.
 */
function uniqueByOid(pages: readonly ObjectPage[] | undefined): ObjectItem[] {
  const seen = new Set<number>();
  const out: ObjectItem[] = [];
  for (const page of pages ?? []) {
    for (const item of page.items) {
      if (seen.has(item.oid)) continue;
      seen.add(item.oid);
      out.push(item);
    }
  }
  return out;
}

/** Раскрытие строк: набор раскрытых oid и переключатель, общие для списка и вложенных списков партиций. */
interface OpenState {
  openOids: ReadonlySet<number>;
  onToggle: (oid: number) => void;
}

/**
 * Какие партиционированные таблицы раскрыты. Состояние живёт в списке, а не в строке: виртуализация
 * размонтирует строки, вышедшие из окна, и локальное состояние пропало бы вместе с ними.
 * oid уникален в пределах базы, поэтому один набор обслуживает и вложенные списки партиций.
 */
function useOpenOids(): OpenState {
  const [openOids, setOpenOids] = useState<ReadonlySet<number>>(() => new Set());
  const onToggle = useCallback((oid: number) => {
    setOpenOids((prev) => {
      const next = new Set(prev);
      if (!next.delete(oid)) next.add(oid);
      return next;
    });
  }, []);
  return useMemo(() => ({ openOids, onToggle }), [openOids, onToggle]);
}

/** Вкладка объекта из строки списка; схема — схема самого объекта (партиция может лежать в другой схеме, чем список). */
function objectTab(kind: ObjectKind, item: ObjectItem, connectionId: string, database: string): TabSpec {
  const { schema, name } = item;
  switch (kind) {
    case "tables":
      return { kind: "table", connectionId, database, schema, name };
    case "views":
      return { kind: "view", connectionId, database, schema, name, matview: false };
    case "matviews":
      return { kind: "view", connectionId, database, schema, name, matview: true };
    case "functions":
      // Имена функций перегружены: вкладка открывается по oid.
      return { kind: "function", connectionId, database, oid: item.oid, name };
    case "sequences":
      return { kind: "sequence", connectionId, database, schema, name };
  }
}

function ObjectRow({
  item,
  kind,
  connectionId,
  db,
  schema,
  openState,
  rowAttrs,
}: {
  item: ObjectItem;
  /** Вид объектов списка: по нему выбирается вкладка. */
  kind: ObjectKind;
  connectionId: string;
  db: string;
  /** Схема списка, в котором лежит строка: объект из другой схемы (партиция) подписывается `схема.имя`. */
  schema: string;
  openState: OpenState;
  rowAttrs?: RowAttrs;
}) {
  const { t } = useTranslation();
  const openTab = useWorkspace((s) => s.open);
  const partitioned = item.relkind === "p" && (item.partitions ?? 0) > 0;
  const open = partitioned && openState.openOids.has(item.oid);
  const name = item.schema === schema ? item.name : `${item.schema}.${item.name}`;
  const label = item.signature !== null ? `${name}(${item.signature})` : name;
  const toggleLabel = t("sidebar.partitionsToggle", { name: label });
  return (
    <li {...rowAttrs}>
      <div className="tree-conn tree-wrap">
        {/* Стрелка раскрывает партиции (состояние — aria-expanded), имя открывает вкладку: у кнопок разные имена. */}
        {partitioned && (
          <button
            type="button"
            className="tree-folder-toggle caret-only"
            aria-expanded={open}
            aria-label={toggleLabel}
            title={toggleLabel}
            onClick={() => openState.onToggle(item.oid)}
          >
            <span className="tree-caret" aria-hidden="true">
              {open ? "▾" : "▸"}
            </span>
          </button>
        )}
        <button
          type="button"
          className="link tree-name"
          title={label}
          aria-label={t("sidebar.openObject", { name: label })}
          onClick={() => openTab(objectTab(kind, item, connectionId, db))}
        >
          {label}
        </button>
        {partitioned && <span className="muted">{t("sidebar.partitions", { count: item.partitions ?? 0 })}</span>}
        {item.totalBytes !== null && item.totalBytes > 0 && <span className="muted">{formatBytes(item.totalBytes)}</span>}
        {/* Размер партиционированной таблицы в списке не считается: пришлось бы заблокировать каждую партицию. */}
        {item.relkind === "p" && item.totalBytes === null && <Glyph className="muted" symbol="—" label={t("sidebar.partitionedSize")} />}
      </div>
      {open && <PartitionList connectionId={connectionId} db={db} schema={item.schema} table={item.name} openState={openState} />}
    </li>
  );
}

/** Кнопка «Ещё»; имя называет список (вид, схема), иначе одинаковые кнопки разных списков не различить. */
function MoreButton({ pending, label, onClick }: { pending: boolean; label: string; onClick: () => void }) {
  const { t } = useTranslation();
  return (
    <li>
      <button type="button" className="link" disabled={pending} aria-label={label} onClick={onClick}>
        {t("sidebar.more")}
      </button>
    </li>
  );
}

function PartitionList({
  connectionId,
  db,
  schema,
  table,
  openState,
}: {
  connectionId: string;
  db: string;
  schema: string;
  table: string;
  openState: OpenState;
}) {
  const { t } = useTranslation();
  const q = useInfiniteQuery({
    queryKey: ["catalog", connectionId, "partitions", db, schema, table],
    queryFn: ({ pageParam }) => api.listPartitions(connectionId, db, schema, table, pageParam, PAGE),
    initialPageParam: 0,
    getNextPageParam: (last) => last.nextOffset ?? undefined,
  });
  const items = useMemo(() => uniqueByOid(q.data?.pages), [q.data]);
  if (q.isPending) {
    return (
      <p className="tree-note" role="status">
        {t("common.loading")}
      </p>
    );
  }
  if (q.error && !q.data) return <ErrorBanner error={q.error} onRetry={() => void q.refetch()} />;
  return (
    <>
      {q.error && <ErrorBanner error={q.error} onRetry={() => void (q.isFetchNextPageError ? q.fetchNextPage() : q.refetch())} />}
      <ul>
        {items.map((item) => (
          // Партиция — таблица: вкладка открывается как у таблицы, схема берётся у самой партиции.
          <ObjectRow key={item.oid} item={item} kind="tables" connectionId={connectionId} db={db} schema={schema} openState={openState} />
        ))}
        {q.hasNextPage && (
          <MoreButton
            pending={q.isFetchingNextPage}
            label={t("sidebar.morePartitions", { where: `${db}.${schema}.${table}` })}
            onClick={() => void q.fetchNextPage()}
          />
        )}
      </ul>
    </>
  );
}

/** Постраничный список объектов одного вида в схеме: фильтр в SQL, виртуализация от VIRTUAL_FROM строк. */
export function ObjectList({
  connectionId,
  db,
  schema,
  kind,
  count,
  label,
}: {
  connectionId: string;
  db: string;
  schema: string;
  kind: ObjectKind;
  count: number;
  label: string;
}) {
  const { t } = useTranslation();
  const [filter, setFilter] = useState("");
  const debounced = useDebouncedValue(filter.trim(), 300);
  const q = useInfiniteQuery({
    queryKey: ["catalog", connectionId, "objects", db, schema, kind, debounced],
    queryFn: ({ pageParam }) => api.listObjects(connectionId, db, schema, kind, debounced || null, pageParam, PAGE),
    initialPageParam: 0,
    getNextPageParam: (last) => last.nextOffset ?? undefined,
    // Пока набирается фильтр, прежний список остаётся на месте — без мигания «Загрузка…».
    placeholderData: keepPreviousData,
  });
  const items = useMemo(() => uniqueByOid(q.data?.pages), [q.data]);
  const openState = useOpenOids();
  const total = q.data?.pages[q.data.pages.length - 1]?.total ?? 0;
  const parentRef = useRef<HTMLDivElement>(null);
  const virtual = items.length > VIRTUAL_FROM;
  const virtualizer = useVirtualizer({
    count: virtual ? items.length : 0,
    enabled: virtual,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_HEIGHT,
    // Высота замеряется (раскрытая партиция увеличивает строку), поэтому размеры привязаны к объекту, а не к номеру.
    getItemKey: (index) => items[index]?.oid ?? index,
    overscan: 20,
  });
  const visible = virtual ? virtualizer.getVirtualItems() : [];
  const lastVisible = visible.length > 0 ? visible[visible.length - 1].index : -1;
  const { hasNextPage, isFetchingNextPage, isFetchNextPageError, fetchNextPage } = q;
  // Новый фильтр — новый список: прокрутка возвращается в начало, а не остаётся на прежнем смещении.
  useEffect(() => {
    if (parentRef.current) parentRef.current.scrollTop = 0;
  }, [debounced]);
  useEffect(() => {
    // После ошибки следующую страницу берёт кнопка «Повторить»: иначе запрос повторялся бы на каждой перерисовке.
    if (virtual && lastVisible >= items.length - PREFETCH_AHEAD && hasNextPage && !isFetchingNextPage && !isFetchNextPageError) {
      void fetchNextPage();
    }
  }, [virtual, lastVisible, items.length, hasNextPage, isFetchingNextPage, isFetchNextPageError, fetchNextPage]);

  // Вид и «база.схема» — по ним различаются одноимённые списки, поля фильтра и кнопки «Ещё» разных схем.
  const where = `${db}.${schema}`;

  return (
    <div>
      {count > FILTER_FROM && (
        <input
          className="tree-filter"
          aria-label={t("sidebar.objectFilter", { label, where })}
          placeholder={t("sidebar.objectFilterPlaceholder")}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
      )}
      {q.isPending && (
        <p className="tree-note" role="status">
          {t("common.loading")}
        </p>
      )}
      {q.error && <ErrorBanner error={q.error} onRetry={() => void (q.isFetchNextPageError ? q.fetchNextPage() : q.refetch())} />}
      {q.data && items.length === 0 && <p className="tree-note">{t("sidebar.noMatches")}</p>}
      {virtual ? (
        // Прокручиваемая область доступна с клавиатуры (Tab, затем стрелки и PageUp/PageDown), поэтому ей нужны роль и имя.
        <div
          ref={parentRef}
          className="tree-virtual"
          role="group"
          tabIndex={0}
          aria-label={t("sidebar.objectList", { label, where })}
          style={{ height: 320, overflow: "auto", position: "relative" }}
        >
          <ul style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
            {visible.map((v) => (
              <ObjectRow
                key={items[v.index].oid}
                item={items[v.index]}
                kind={kind}
                connectionId={connectionId}
                db={db}
                schema={schema}
                openState={openState}
                rowAttrs={{
                  ref: virtualizer.measureElement,
                  "data-index": v.index,
                  style: { position: "absolute", top: v.start, left: 0, right: 0 },
                }}
              />
            ))}
          </ul>
        </div>
      ) : (
        <ul>
          {items.map((item) => (
            <ObjectRow key={item.oid} item={item} kind={kind} connectionId={connectionId} db={db} schema={schema} openState={openState} />
          ))}
          {hasNextPage && (
            <MoreButton pending={isFetchingNextPage} label={t("sidebar.moreFor", { label, where })} onClick={() => void fetchNextPage()} />
          )}
        </ul>
      )}
      {q.data && <p className="tree-note">{t("sidebar.shown", { shown: formatCount(items.length), total: formatCount(total) })}</p>}
    </div>
  );
}
