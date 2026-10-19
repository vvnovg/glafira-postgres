import { keepPreviousData, useInfiniteQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { type HTMLAttributes, type Ref, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/client";
import type { ObjectItem, ObjectKind } from "../../../api/types";
import { formatBytes, formatCount } from "../../../lib/format";
import { useDebouncedValue } from "../../../lib/useDebouncedValue";
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

function ObjectRow({ item, connectionId, db, rowAttrs }: { item: ObjectItem; connectionId: string; db: string; rowAttrs?: RowAttrs }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const partitioned = item.relkind === "p" && (item.partitions ?? 0) > 0;
  const label = item.signature !== null ? `${item.name}(${item.signature})` : item.name;
  return (
    <li {...rowAttrs}>
      <div className="tree-conn tree-wrap">
        {partitioned ? (
          <button type="button" className="tree-folder-toggle" aria-expanded={open} title={label} onClick={() => setOpen(!open)}>
            <span className="tree-caret" aria-hidden="true">
              {open ? "▾" : "▸"}
            </span>
            <span className="tree-name">{label}</span>
          </button>
        ) : (
          <span className="tree-name" title={label}>
            {label}
          </span>
        )}
        {partitioned && <span className="muted">{t("sidebar.partitions", { count: item.partitions ?? 0 })}</span>}
        {item.totalBytes !== null && item.totalBytes > 0 && <span className="muted">{formatBytes(item.totalBytes)}</span>}
      </div>
      {open && <PartitionList connectionId={connectionId} db={db} schema={item.schema} table={item.name} />}
    </li>
  );
}

function MoreButton({ pending, onClick }: { pending: boolean; onClick: () => void }) {
  const { t } = useTranslation();
  return (
    <li>
      <button type="button" className="link" disabled={pending} onClick={onClick}>
        {t("sidebar.more")}
      </button>
    </li>
  );
}

function PartitionList({ connectionId, db, schema, table }: { connectionId: string; db: string; schema: string; table: string }) {
  const { t } = useTranslation();
  const q = useInfiniteQuery({
    queryKey: ["catalog", connectionId, "partitions", db, schema, table],
    queryFn: ({ pageParam }) => api.listPartitions(connectionId, db, schema, table, pageParam, PAGE),
    initialPageParam: 0,
    getNextPageParam: (last) => last.nextOffset ?? undefined,
  });
  if (q.isPending) return <p className="tree-note">{t("common.loading")}</p>;
  if (q.error && !q.data) return <ErrorBanner error={q.error} onRetry={() => void q.refetch()} />;
  return (
    <>
      {q.error && <ErrorBanner error={q.error} onRetry={() => void (q.isFetchNextPageError ? q.fetchNextPage() : q.refetch())} />}
      <ul>
        {q.data?.pages
          .flatMap((p) => p.items)
          .map((item) => (
            <ObjectRow key={item.oid} item={item} connectionId={connectionId} db={db} />
          ))}
        {q.hasNextPage && <MoreButton pending={q.isFetchingNextPage} onClick={() => void q.fetchNextPage()} />}
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
  const items = useMemo(() => q.data?.pages.flatMap((p) => p.items) ?? [], [q.data]);
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
  useEffect(() => {
    // После ошибки следующую страницу берёт кнопка «Повторить»: иначе запрос повторялся бы на каждой перерисовке.
    if (virtual && lastVisible >= items.length - PREFETCH_AHEAD && hasNextPage && !isFetchingNextPage && !isFetchNextPageError) {
      void fetchNextPage();
    }
  }, [virtual, lastVisible, items.length, hasNextPage, isFetchingNextPage, isFetchNextPageError, fetchNextPage]);

  return (
    <div>
      {count > FILTER_FROM && (
        <input
          className="tree-filter"
          aria-label={t("sidebar.objectFilter", { label })}
          placeholder={t("sidebar.objectFilterPlaceholder")}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
      )}
      {q.isPending && <p className="tree-note">{t("common.loading")}</p>}
      {q.error && <ErrorBanner error={q.error} onRetry={() => void (q.isFetchNextPageError ? q.fetchNextPage() : q.refetch())} />}
      {q.data && items.length === 0 && <p className="tree-note">{t("sidebar.noMatches")}</p>}
      {virtual ? (
        <div ref={parentRef} className="tree-virtual" style={{ height: 320, overflow: "auto", position: "relative" }}>
          <ul style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
            {visible.map((v) => (
              <ObjectRow
                key={items[v.index].oid}
                item={items[v.index]}
                connectionId={connectionId}
                db={db}
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
            <ObjectRow key={item.oid} item={item} connectionId={connectionId} db={db} />
          ))}
          {hasNextPage && <MoreButton pending={isFetchingNextPage} onClick={() => void fetchNextPage()} />}
        </ul>
      )}
      {q.data && <p className="tree-note">{t("sidebar.shown", { shown: formatCount(items.length), total: formatCount(total) })}</p>}
    </div>
  );
}
