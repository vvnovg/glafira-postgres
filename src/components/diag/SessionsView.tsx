import { useVirtualizer } from "@tanstack/react-virtual";
import { type CSSProperties, Fragment, type KeyboardEvent, type Ref, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { DiagReport, SessionRow, SessionsData } from "../../api/types";
import { formatCount, formatDuration, formatTimestamp } from "../../lib/format";
import type { SessionActionChoice } from "../actions/SessionActionDialog";
import { SubTabs } from "../catalog/SubTabs";
import { CopyButton } from "../common/CopyButton";
import { Hint } from "../common/Hint";
import { blockersText, oneLine, QUERY_PREVIEW_CHARS, QUERY_TITLE_CHARS } from "./sessionText";

/** Набор сессий: что показывает список (ждущие и idle in transaction — готовые отборы «кто мешает»). */
export type SessionsPreset = "all" | "active" | "idleTx" | "waiting";
type GroupBy = "none" | "application" | "user" | "client" | "database";

/** Высота строки списка, px: строки одной высоты (текст запроса обрезается), поэтому виртуализатору не нужен замер. */
const ROW_HEIGHT = 26;
/** Со скольких строк список виртуализируется (меньше — рисуется целиком, как список объектов дерева). */
const VIRTUAL_FROM = 100;

const isIdleTx = (r: SessionRow) => r.state?.startsWith("idle in transaction") ?? false;
// blockedBy заполняется только у ждущих блокировку, но сессия могла перестать ждать между запросами — берём оба признака.
const isWaiting = (r: SessionRow) => r.waitEventType === "Lock" || r.blockedBy.length > 0;

const PRESETS: { id: SessionsPreset; labelKey: string; match: (r: SessionRow) => boolean; sortBy?: (r: SessionRow) => number | null }[] = [
  { id: "all", labelKey: "diag.sessions.presets.all", match: () => true },
  { id: "active", labelKey: "diag.sessions.presets.active", match: (r) => r.state === "active", sortBy: (r) => r.queryAgeS },
  { id: "idleTx", labelKey: "diag.sessions.presets.idleTx", match: isIdleTx, sortBy: (r) => r.stateAgeS },
  { id: "waiting", labelKey: "diag.sessions.presets.waiting", match: isWaiting, sortBy: (r) => r.queryAgeS },
];

const GROUPS: { id: GroupBy; labelKey: string; of?: (r: SessionRow) => string | null }[] = [
  { id: "none", labelKey: "diag.sessions.groups.none" },
  { id: "application", labelKey: "diag.sessions.groups.application", of: (r) => r.application },
  { id: "user", labelKey: "diag.sessions.groups.user", of: (r) => r.user },
  { id: "client", labelKey: "diag.sessions.groups.client", of: (r) => r.clientAddr },
  { id: "database", labelKey: "diag.sessions.groups.database", of: (r) => r.database },
];

/** Ключ сессии: pid переиспользуется, поэтому вместе со временем старта backend. */
const rowKey = (r: SessionRow) => `${r.pid}:${r.backendStartUs ?? ""}`;
const waitText = (r: SessionRow) => (r.waitEventType === null ? "" : r.waitEvent === null ? r.waitEventType : `${r.waitEventType}:${r.waitEvent}`);
const clientText = (r: SessionRow) => (r.clientAddr === null ? "" : r.clientPort === null || r.clientPort < 0 ? r.clientAddr : `${r.clientAddr}:${r.clientPort}`);

interface Col {
  id: string;
  labelKey: string;
  /** Ширина колонки, px; у `flex` — минимальная (колонка занимает остаток). */
  width: number;
  flex?: boolean;
  cell(r: SessionRow): string;
  /** Подсказка ячейки; по умолчанию — её текст. */
  title?(r: SessionRow): string | undefined;
}

const COLS = {
  pid: { id: "pid", labelKey: "diag.sessions.columns.pid", width: 64, cell: (r) => String(r.pid) },
  state: { id: "state", labelKey: "diag.sessions.columns.state", width: 150, cell: (r) => r.state ?? "" },
  idleFor: { id: "idleFor", labelKey: "diag.sessions.columns.idleFor", width: 96, cell: (r) => formatDuration(r.stateAgeS) },
  locksHeld: { id: "locksHeld", labelKey: "diag.sessions.columns.locksHeld", width: 150, cell: (r) => formatCount(r.locksHeld) },
  xminAge: { id: "xminAge", labelKey: "diag.sessions.columns.xminAge", width: 110, cell: (r) => (r.xminAge === null ? "—" : formatCount(r.xminAge)) },
  wait: { id: "wait", labelKey: "diag.sessions.columns.wait", width: 160, cell: waitText },
  queryAge: { id: "queryAge", labelKey: "diag.sessions.columns.queryAge", width: 96, cell: (r) => formatDuration(r.queryAgeS) },
  xactAge: { id: "xactAge", labelKey: "diag.sessions.columns.xactAge", width: 100, cell: (r) => formatDuration(r.xactAgeS) },
  user: { id: "user", labelKey: "diag.sessions.columns.user", width: 90, cell: (r) => r.user ?? "" },
  application: { id: "application", labelKey: "diag.sessions.columns.application", width: 130, cell: (r) => r.application ?? "" },
  client: { id: "client", labelKey: "diag.sessions.columns.client", width: 110, cell: (r) => r.clientAddr ?? "" },
  database: { id: "database", labelKey: "diag.sessions.columns.database", width: 100, cell: (r) => r.database ?? "" },
  blockedBy: { id: "blockedBy", labelKey: "diag.sessions.columns.blockedBy", width: 80, cell: (r) => blockersText(r.blockedBy) },
  text: {
    id: "text",
    labelKey: "diag.sessions.columns.text",
    width: 240,
    flex: true,
    cell: (r) => oneLine(r.query, QUERY_PREVIEW_CHARS),
    title: (r) => r.query?.slice(0, QUERY_TITLE_CHARS),
  },
} satisfies Record<string, Col>;

const BASE_COLS: Col[] = [COLS.pid, COLS.state, COLS.wait, COLS.queryAge, COLS.xactAge, COLS.user, COLS.application, COLS.client, COLS.database, COLS.blockedBy, COLS.text];
// У idle in transaction важнее простой, число блокировок и возраст xmin. Состояние (всегда idle in transaction), ожидание
// (всегда Client:ClientRead), «Ждёт» (простаивающая сессия ничего не ждёт) и время запроса (≈ простой) в этом наборе
// ничего не говорят — без них таблица помещается в окно.
const IDLE_TX_COLS: Col[] = [COLS.pid, COLS.idleFor, COLS.locksHeld, COLS.xminAge, COLS.xactAge, COLS.user, COLS.application, COLS.client, COLS.database, COLS.text];

/** Строка подходит под фильтр: подстрока (без учёта регистра) в pid, роли, приложении, клиенте, БД или запросе. */
function haystack(r: SessionRow): string {
  return [String(r.pid), r.user, r.application, r.clientAddr, r.database, r.query].filter(Boolean).join("\n").toLowerCase();
}

function sortRows(rows: SessionRow[], by: ((r: SessionRow) => number | null) | undefined): SessionRow[] {
  if (!by) return rows;
  // Без значения — в конец; сортировка устойчива, равные остаются в порядке pid.
  return [...rows].sort((a, b) => (by(b) ?? -Infinity) - (by(a) ?? -Infinity) || 0);
}

interface Group {
  name: string;
  total: number;
  active: number;
  idle: number;
  idleTx: number;
  other: number;
}

function groupRows(rows: SessionRow[], of: (r: SessionRow) => string | null): Group[] {
  const groups = new Map<string, Group>();
  for (const r of rows) {
    const name = of(r) ?? "";
    let g = groups.get(name);
    if (!g) groups.set(name, (g = { name, total: 0, active: 0, idle: 0, idleTx: 0, other: 0 }));
    g.total++;
    if (r.state === "active") g.active++;
    else if (r.state === "idle") g.idle++;
    else if (isIdleTx(r)) g.idleTx++;
    else g.other++;
  }
  return [...groups.values()].sort((a, b) => b.total - a.total || a.name.localeCompare(b.name));
}

function GroupTable({ rows, by }: { rows: SessionRow[]; by: Exclude<GroupBy, "none"> }) {
  const { t } = useTranslation();
  const group = GROUPS.find((g) => g.id === by)!;
  const groups = useMemo(() => groupRows(rows, group.of!), [rows, group]);
  return (
    <>
      <div className="table-scroll">
        <table aria-label={t("diag.sessions.groupTable")}>
          <thead>
            <tr>
              <th scope="col">{t(group.labelKey)}</th>
              <th scope="col">{t("diag.sessions.groupTotal")}</th>
              <th scope="col">{t("diag.sessions.groupActive")}</th>
              <th scope="col">{t("diag.sessions.groupIdle")}</th>
              <th scope="col">{t("diag.sessions.groupIdleTx")}</th>
              <th scope="col">{t("diag.sessions.groupOther")}</th>
            </tr>
          </thead>
          <tbody>
            {groups.map((g) => (
              <tr key={g.name}>
                <th scope="row">{g.name === "" ? <span className="muted">{t("diag.sessions.groupUnset")}</span> : g.name}</th>
                <td>{formatCount(g.total)}</td>
                <td>{formatCount(g.active)}</td>
                <td>{formatCount(g.idle)}</td>
                <td>{formatCount(g.idleTx)}</td>
                <td>{formatCount(g.other)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="muted">{t("diag.sessions.groupNote")}</p>
    </>
  );
}

function SessionRowView({
  row,
  cols,
  index,
  selected,
  tabbable,
  onSelect,
  onFocus,
  style,
}: {
  row: SessionRow;
  cols: Col[];
  /** Номер строки данных (с нуля). */
  index: number;
  selected: boolean;
  tabbable: boolean;
  onSelect: () => void;
  onFocus: () => void;
  style?: CSSProperties;
}) {
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.target !== e.currentTarget) return;
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onSelect();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      // Фокус переходит к соседней отрисованной строке; браузер прокручивает к ней, и виртуализатор дорисовывает следующие.
      const next = e.key === "ArrowDown" ? e.currentTarget.nextElementSibling : e.currentTarget.previousElementSibling;
      if (next instanceof HTMLElement) {
        e.preventDefault();
        next.focus();
      }
    }
  };
  return (
    <div
      role="row"
      aria-rowindex={index + 2}
      aria-current={selected ? "true" : undefined}
      tabIndex={tabbable ? 0 : -1}
      className={`sess-row${selected ? " selected" : ""}${row.glafira ? " muted" : ""}`}
      style={style}
      onClick={onSelect}
      onFocus={onFocus}
      onKeyDown={onKeyDown}
    >
      {cols.map((c) => {
        const text = c.cell(row);
        return (
          <div key={c.id} role="cell" title={c.title ? c.title(row) : text || undefined}>
            {text}
          </div>
        );
      })}
    </div>
  );
}

function SessionList({ rows, cols, selectedKey, onSelect }: { rows: SessionRow[]; cols: Col[]; selectedKey: string | null; onSelect: (r: SessionRow) => void }) {
  const { t } = useTranslation();
  const parentRef = useRef<HTMLDivElement>(null);
  // Строка, на которой остановился фокус: она — единственная в порядке Tab (остальные — стрелками), иначе сотня строк = сотня остановок.
  const [focusKey, setFocusKey] = useState<string | null>(null);
  const virtual = rows.length > VIRTUAL_FROM;
  const virtualizer = useVirtualizer({
    count: virtual ? rows.length : 0,
    enabled: virtual,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_HEIGHT,
    // Заголовок лежит в той же прокручиваемой области выше первой строки.
    scrollMargin: ROW_HEIGHT,
    getItemKey: (i) => (rows[i] ? rowKey(rows[i]) : i),
    overscan: 20,
  });
  const visible = virtual ? virtualizer.getVirtualItems().map((v) => ({ index: v.index, top: v.start - ROW_HEIGHT })) : rows.map((_, index) => ({ index, top: 0 }));
  const keys = visible.map((v) => rowKey(rows[v.index]));
  const tabKey = focusKey !== null && keys.includes(focusKey) ? focusKey : (keys[0] ?? null);
  const minWidth = cols.reduce((sum, c) => sum + c.width, 16);
  const style = {
    "--sess-cols": cols.map((c) => (c.flex ? `minmax(${c.width}px, 1fr)` : `${c.width}px`)).join(" "),
    minWidth,
  } as CSSProperties;
  return (
    <div className="sess-scroll" ref={parentRef}>
      <div role="table" aria-label={t("diag.sessions.table")} aria-rowcount={rows.length + 1} className="sess-table" style={style}>
        <div role="rowgroup" className="sess-head-group">
          <div role="row" aria-rowindex={1} className="sess-row sess-head">
            {cols.map((c) => (
              <div key={c.id} role="columnheader">
                {t(c.labelKey)}
              </div>
            ))}
          </div>
        </div>
        <div role="rowgroup" style={virtual ? { height: rows.length * ROW_HEIGHT, position: "relative" } : undefined}>
          {visible.map((v, i) => {
            const row = rows[v.index];
            const key = keys[i];
            return (
              <SessionRowView
                key={key}
                row={row}
                cols={cols}
                index={v.index}
                selected={key === selectedKey}
                tabbable={key === tabKey}
                onSelect={() => onSelect(row)}
                onFocus={() => setFocusKey(key)}
                style={virtual ? { position: "absolute", top: v.top, left: 0, right: 0 } : undefined}
              />
            );
          })}
        </div>
      </div>
    </div>
  );
}

/**
 * Панель сессии: поля, полный текст запроса и копирование. `ended` — сессии уже нет в списке, показаны последние данные.
 * Кнопки отмены и завершения (`onSessionAction`) — у клиентской сессии с известным временем старта: пара (pid, время старта)
 * однозначна, а служебные процессы сервера сигналом не трогаем.
 */
function SessionPanel({
  row,
  ended,
  onClose,
  onSessionAction,
  ref,
}: {
  row: SessionRow;
  ended: boolean;
  onClose: () => void;
  onSessionAction?: (choice: SessionActionChoice) => void;
  ref: Ref<HTMLElement>;
}) {
  const { t } = useTranslation();
  const startUs = row.backendStartUs;
  const signal = (kind: SessionActionChoice["kind"]) => {
    if (startUs !== null) onSessionAction?.({ kind, target: { kind: "selected", sessions: [{ pid: row.pid, backendStartUs: startUs }] } });
  };
  const fields: { labelKey: string; value: string }[] = [
    { labelKey: "diag.sessions.columns.state", value: row.state ?? "" },
    { labelKey: "diag.sessions.columns.wait", value: waitText(row) },
    { labelKey: "diag.sessions.columns.blockedBy", value: blockersText(row.blockedBy) },
    { labelKey: "diag.sessions.columns.user", value: row.user ?? "" },
    { labelKey: "diag.sessions.columns.application", value: row.application ?? "" },
    { labelKey: "diag.sessions.columns.client", value: clientText(row) },
    { labelKey: "diag.sessions.columns.database", value: row.database ?? "" },
    { labelKey: "diag.sessions.fields.backendType", value: row.backendType ?? "" },
    { labelKey: "diag.sessions.fields.leaderPid", value: row.leaderPid === null ? "" : String(row.leaderPid) },
    { labelKey: "diag.sessions.fields.started", value: row.backendStartUs === null ? "" : formatTimestamp(Math.floor(row.backendStartUs / 1000)) },
    { labelKey: "diag.sessions.columns.xactAge", value: row.xactAgeS === null ? "" : formatDuration(row.xactAgeS) },
    { labelKey: "diag.sessions.columns.queryAge", value: row.queryAgeS === null ? "" : formatDuration(row.queryAgeS) },
    { labelKey: "diag.sessions.fields.stateAge", value: row.stateAgeS === null ? "" : formatDuration(row.stateAgeS) },
    { labelKey: "diag.sessions.fields.xid", value: row.backendXid ?? "" },
    { labelKey: "diag.sessions.columns.xminAge", value: row.xminAge === null ? "" : formatCount(row.xminAge) },
    { labelKey: "diag.sessions.fields.queryId", value: row.queryId ?? "" },
    { labelKey: "diag.sessions.columns.locksHeld", value: formatCount(row.locksHeld) },
  ].filter((f) => f.value !== "");
  return (
    <section ref={ref} role="region" aria-label={t("diag.sessions.panel", { pid: row.pid })} className="panel sess-panel">
      <div className="toolbar">
        {/* tabIndex -1: заголовок принимает фокус программно, когда пользователь открыл панель, но не попадает в порядок Tab. */}
        <h3 tabIndex={-1}>{t("diag.sessions.panel", { pid: row.pid })}</h3>
        {onSessionAction && startUs !== null && row.backendType === "client backend" && (
          <>
            <button type="button" disabled={ended} onClick={() => signal("cancel")}>
              {t("actions.buttons.cancel")}
            </button>
            <button type="button" disabled={ended} onClick={() => signal("terminate")}>
              {t("actions.buttons.terminate")}
            </button>
          </>
        )}
        <button type="button" onClick={onClose}>
          {t("common.close")}
        </button>
      </div>
      {ended && <p className="diag-note">{t("diag.sessions.ended")}</p>}
      <dl className="kv">
        {fields.map((f) => (
          <Fragment key={f.labelKey}>
            <dt>{t(f.labelKey)}</dt>
            <dd>{f.value}</dd>
          </Fragment>
        ))}
      </dl>
      {row.queryTruncated && <p className="muted">{t("diag.sessions.truncated")}</p>}
      {row.query && (
        <>
          <pre className="sess-query">{row.query}</pre>
          <div>
            <CopyButton text={row.query} label={t("diag.sessions.copyQuery")} />
          </div>
        </>
      )}
    </section>
  );
}

/** Значение `idle_in_transaction_session_timeout` (мс): 0 — выключено, целые секунды — длительностью. */
function timeoutText(ms: number | null): string {
  if (ms === null) return "—";
  if (ms === 0) return "0";
  return ms % 1000 === 0 ? formatDuration(ms / 1000) : `${formatCount(ms)} ms`;
}

/** Порог в минутах: целое не меньше 1 (меньше минуты — шум обновления отчёта, а не простой); иначе `null`. */
function parseMinutes(text: string): number | null {
  const n = Number(text.trim());
  // Порог уходит в Rust секундами: число должно остаться точным целым и после умножения на 60.
  return Number.isInteger(n) && n >= 1 && Number.isSafeInteger(n * 60) ? n : null;
}

/** «Завершить idle in transaction старше N мин…»: пакет по критерию, диалог предпросмотра покажет, кого это затронет. */
function IdleTerminate({ onSessionAction }: { onSessionAction: (choice: SessionActionChoice) => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState("5");
  const min = parseMinutes(text);
  return (
    <div className="toolbar">
      <label className="inline">
        <span>{t("actions.buttons.idleOlderThan")}</span>
        <input type="number" min={1} step={1} value={text} aria-invalid={min === null} onChange={(e) => setText(e.target.value)} />
      </label>
      <button
        type="button"
        disabled={min === null}
        onClick={() => min !== null && onSessionAction({ kind: "terminate", target: { kind: "idleInTransaction", olderThanS: min * 60 } })}
      >
        {t("actions.buttons.terminateIdle", { min: min ?? "N" })}
      </button>
    </div>
  );
}

function SessionsBody({ data, preset: initial, onSessionAction }: { data: SessionsData; preset: SessionsPreset; onSessionAction?: (choice: SessionActionChoice) => void }) {
  const { t } = useTranslation();
  const [preset, setPreset] = useState<SessionsPreset>(initial);
  const [filter, setFilter] = useState("");
  const [groupBy, setGroupBy] = useState<GroupBy>("none");
  const [hideGlafira, setHideGlafira] = useState(true);
  // Сессия, открытая в панели: последний снимок её строки (обновляется с каждым отчётом, пока сессия есть в списке).
  const [opened, setOpened] = useState<SessionRow | null>(null);
  // Счётчик открытий панели пользователем (клик, Enter): по нему фокус уходит в панель. Обновление данных и возврат панели после
  // переключения группировки счётчик не меняют, поэтому фокус не отнимают.
  const [openCount, setOpenCount] = useState(0);
  const panelRef = useRef<HTMLElement>(null);
  const select = (r: SessionRow) => {
    setOpened(r);
    setOpenCount((n) => n + 1);
  };
  useEffect(() => {
    if (openCount === 0) return;
    const panel = panelRef.current;
    // Панель открывается под списком: заголовок получает фокус (читалка объявит сессию), а панель показывается целиком.
    panel?.querySelector<HTMLElement>("h3")?.focus({ preventScroll: true });
    panel?.scrollIntoView?.({ block: "nearest" });
  }, [openCount]);

  const needle = filter.trim().toLowerCase();
  const filtering = needle !== "";
  // Строки поиска строятся, только пока фильтр задан (тысяча сессий × текст запроса до 16 КБ), и переживают набор текста фильтра.
  const haystacks = useMemo(() => (filtering ? new Map(data.rows.map((r) => [rowKey(r), haystack(r)])) : null), [data.rows, filtering]);
  // Скрытие сессий Glafira и фильтр действуют и на список, и на группы; набор — только на список (счётчики групп — по состояниям).
  const base = useMemo(
    () => data.rows.filter((r) => !(hideGlafira && r.glafira) && (haystacks === null || (haystacks.get(rowKey(r)) ?? "").includes(needle))),
    [data.rows, hideGlafira, needle, haystacks],
  );
  const current = PRESETS.find((p) => p.id === preset) ?? PRESETS[0];
  const rows = useMemo(() => sortRows(base.filter(current.match), current.sortBy), [base, current]);
  const cols = preset === "idleTx" ? IDLE_TX_COLS : BASE_COLS;
  const grouped = groupBy !== "none";

  const openedKey = opened ? rowKey(opened) : null;
  const fresh = openedKey === null ? undefined : data.rows.find((r) => rowKey(r) === openedKey);
  // Снимок обновляется вместе со списком: когда сессия исчезнет, панель останется с её последними данными, а не с данными клика.
  if (fresh !== undefined && fresh !== opened) setOpened(fresh);

  return (
    <SubTabs label={t("diag.sessions.presetLabel")} tabs={PRESETS.map((p) => ({ id: p.id, label: t(p.labelKey) }))} value={preset} onChange={setPreset}>
      {() => (
        <div className="sess-view">
          <div className="toolbar">
            <input type="text" aria-label={t("diag.sessions.filter")} placeholder={t("diag.sessions.filterPlaceholder")} value={filter} onChange={(e) => setFilter(e.target.value)} />
            <label className="inline">
              <span>{t("diag.sessions.groupBy")}</span>
              <select aria-label={t("diag.sessions.groupBy")} value={groupBy} onChange={(e) => setGroupBy(e.target.value as GroupBy)}>
                {GROUPS.map((g) => (
                  <option key={g.id} value={g.id}>
                    {t(g.labelKey)}
                  </option>
                ))}
              </select>
            </label>
            <label className="inline">
              <input type="checkbox" checked={hideGlafira} onChange={(e) => setHideGlafira(e.target.checked)} />
              <span>{t("diag.sessions.hideGlafira")}</span>
            </label>
          </div>
          {preset === "idleTx" && (
            <p className="muted sess-setting">
              <code>{t("diag.sessions.timeoutSetting", { value: timeoutText(data.settings.idleInTransactionSessionTimeoutMs) })}</code>
              <Hint k="hints.diag.idleTimeout" label={t("diag.sessions.timeoutName")} />
            </p>
          )}
          {preset === "idleTx" && onSessionAction && <IdleTerminate onSessionAction={onSessionAction} />}
          {grouped ? (
            <GroupTable rows={base} by={groupBy} />
          ) : (
            <>
              <SessionList rows={rows} cols={cols} selectedKey={openedKey} onSelect={select} />
              {rows.length === 0 && <p className="muted">{t("diag.sessions.none")}</p>}
              <p className="muted">{t("sidebar.shown", { shown: formatCount(rows.length), total: formatCount(data.rows.length) })}</p>
              {opened && <SessionPanel ref={panelRef} row={opened} ended={fresh === undefined} onClose={() => setOpened(null)} onSessionAction={onSessionAction} />}
            </>
          )}
        </div>
      )}
    </SubTabs>
  );
}

/**
 * Сессии и idle in transaction (спец. P2 §7.4): наборы (все, активные, idle in transaction, ждущие), фильтр, группировка,
 * скрытие сессий самой Glafira, виртуализированный список и панель выбранной сессии. `preset` — набор при открытии:
 * у вкладки «Idle in transaction» это одноимённый набор. `onSessionAction` — отмена и завершение сессий (панель сессии,
 * порог в наборе idle in transaction); без него кнопок нет (подключение только для чтения).
 */
export function SessionsView({ report, preset, onSessionAction }: { report: DiagReport; preset: SessionsPreset; onSessionAction?: (choice: SessionActionChoice) => void }) {
  if (report.data.kind !== "sessions") return null;
  return <SessionsBody data={report.data} preset={preset} onSessionAction={onSessionAction} />;
}
