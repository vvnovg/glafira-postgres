import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import type { DdlQueueItem, DiagReport, LockNode, LockSession, LocksData, WaitingLock } from "../../api/types";
import { i18n } from "../../i18n";
import { formatDuration } from "../../lib/format";
import { blockersText, oneLine, QUERY_PREVIEW_CHARS, QUERY_TITLE_CHARS } from "./sessionText";

/** Знаков запроса в карточке DDL (в дереве — `QUERY_PREVIEW_CHARS`); полный текст — в подсказке. */
const CARD_QUERY_CHARS = 200;

/** Отношение блокировки: имя (БД соединения), «oid N в БД X» (другая БД), «oid N» (БД неизвестна) или `null`. */
function relationText(l: { relation: string | null; relationOid: number | null; relationDatabase: string | null }): string | null {
  if (l.relation) return l.relation;
  if (l.relationOid === null) return null;
  return l.relationDatabase ? i18n.t("diag.locks.otherDb", { oid: l.relationOid, db: l.relationDatabase }) : i18n.t("diag.locks.oidOnly", { oid: l.relationOid });
}

/** «Режим на отношении»; блокировка не отношения (transactionid, tuple) — «режим (тип)». */
function requestText(l: { mode: string | null; locktype?: string | null; relation: string | null; relationOid: number | null; relationDatabase: string | null }): string {
  const relation = relationText(l);
  if (l.mode && relation) return i18n.t("diag.locks.requests", { mode: l.mode, relation });
  if (l.mode && l.locktype) return i18n.t("diag.locks.requestsOther", { mode: l.mode, locktype: l.locktype });
  return l.mode ?? relation ?? l.locktype ?? "—";
}

function SessionMeta({ s }: { s: LockSession }) {
  const meta = [s.user, s.application, s.database, s.state].filter(Boolean).join(" · ");
  return meta ? <span className="muted">{meta}</span> : null;
}

function QueryText({ s, max }: { s: LockSession; max: number }) {
  return s.query ? <code className="lock-query" title={s.query.slice(0, QUERY_TITLE_CHARS)}>{oneLine(s.query, max)}</code> : null;
}

function DdlCard({ item }: { item: DdlQueueItem }) {
  const { t } = useTranslation();
  // Фраза целиком — ключ словаря, с ожиданием и без него (скобки вокруг времени — в словаре): код ничего не склеивает.
  // waitsFor пуст, когда блокирующий не виден: подготовленная транзакция или сессия, завершившаяся между запросами.
  const waitS = item.waitS === null ? null : formatDuration(item.waitS);
  let waits: string;
  if (item.waitsFor === null) waits = waitS === null ? t("diag.locks.waitsForHidden") : t("diag.locks.waitsForHiddenTimed", { waitS });
  else waits = waitS === null ? t("diag.locks.waitsFor", { pid: item.waitsFor }) : t("diag.locks.waitsForTimed", { pid: item.waitsFor, waitS });
  return (
    <li className="lock-card">
      <div className="lock-line">
        <strong>{t("diag.locks.session", { pid: item.session.pid })}</strong>
        <SessionMeta s={item.session} />
      </div>
      <QueryText s={item.session} max={CARD_QUERY_CHARS} />
      <div className="lock-line">
        <span>{requestText({ ...item, locktype: "relation" })}</span>
        <span>{waits}</span>
        <span>{t("diag.locks.queued", { count: item.queued })}</span>
      </div>
    </li>
  );
}

function WaitText({ lock }: { lock: WaitingLock }) {
  const { t } = useTranslation();
  if (lock.waitS === null) return null;
  const value = formatDuration(lock.waitS);
  // PostgreSQL 13 не хранит начало ожидания: время оценено по смене состояния сессии.
  return lock.approx ? (
    <span className="lock-wait" title={t("diag.locks.approxTitle")}>
      {t("diag.locks.waitApprox", { value })}
    </span>
  ) : (
    <span className="lock-wait">{value}</span>
  );
}

/** Узел дерева: сессия, её запрос, что она ждёт, сколько сессий за ней; потомки вложены и сворачиваются. */
function TreeNode({ node, base, collapsed, toggle }: { node: LockNode; base: string; collapsed: ReadonlySet<number>; toggle: (pid: number) => void }) {
  const { t } = useTranslation();
  const { session: s, lock } = node;
  const hasChildren = node.children.length > 0;
  const expanded = !collapsed.has(s.pid);
  const labelId = `${base}-${s.pid}`;
  return (
    // Имя узла — только его заголовок: иначе имя treeitem вобрало бы тексты всех вложенных узлов.
    <li role="treeitem" aria-labelledby={labelId} aria-expanded={hasChildren ? expanded : undefined} className="lock-node">
      <div className="lock-line">
        {hasChildren ? (
          <button
            type="button"
            className="icon lock-toggle"
            aria-label={t(expanded ? "diag.locks.collapse" : "diag.locks.expand", { pid: s.pid })}
            onClick={() => toggle(s.pid)}
          >
            <span aria-hidden="true">{expanded ? "▾" : "▸"}</span>
          </button>
        ) : (
          <span className="lock-toggle" aria-hidden="true" />
        )}
        <strong id={labelId}>{t("diag.locks.session", { pid: s.pid })}</strong>
        <SessionMeta s={s} />
        {s.xactAgeS !== null && <span className="muted">{t("diag.locks.xactAge", { duration: formatDuration(s.xactAgeS) })}</span>}
        {node.victims > 0 && <span className="lock-victims">{t("diag.locks.victims", { count: node.victims })}</span>}
      </div>
      <div className="lock-detail">
        <QueryText s={s} max={QUERY_PREVIEW_CHARS} />
        {lock && (
          <div className="lock-line">
            <span>{requestText(lock)}</span>
            <WaitText lock={lock} />
          </div>
        )}
        {node.alsoBlockedBy.length > 0 && <div className="muted">{t("diag.locks.alsoBlockedBy", { pids: blockersText(node.alsoBlockedBy) })}</div>}
      </div>
      {hasChildren && expanded && (
        <ul role="group">
          {node.children.map((c) => (
            <TreeNode key={c.session.pid} node={c} base={base} collapsed={collapsed} toggle={toggle} />
          ))}
        </ul>
      )}
    </li>
  );
}

function LocksBody({ data }: { data: LocksData }) {
  const { t } = useTranslation();
  const base = useId();
  const ddlId = `${base}-ddl`;
  const treeId = `${base}-tree`;
  // Свёрнутые ветки (pid корня ветки): всё раскрыто, пока пользователь не свернёт; выбор переживает обновления отчёта.
  const [collapsed, setCollapsed] = useState<ReadonlySet<number>>(() => new Set());
  const toggle = (pid: number) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (!next.delete(pid)) next.add(pid);
      return next;
    });
  const empty = data.roots.length === 0 && data.cycles.length === 0;
  return (
    <div className="locks-view">
      {data.ddlQueue.length > 0 && (
        <section role="region" aria-labelledby={ddlId} className="panel">
          <h3 id={ddlId}>{t("diag.locks.ddlQueue")}</h3>
          <ul className="plain-list lock-cards">
            {data.ddlQueue.map((item) => (
              <DdlCard key={`${item.session.pid}:${item.session.backendStartUs ?? ""}`} item={item} />
            ))}
          </ul>
        </section>
      )}
      {data.roots.length > 0 && (
        <section>
          <h3 id={treeId}>{t("diag.locks.tree")}</h3>
          <ul role="tree" aria-labelledby={treeId} className="plain-list lock-tree">
            {data.roots.map((r) => (
              <TreeNode key={r.session.pid} node={r} base={base} collapsed={collapsed} toggle={toggle} />
            ))}
          </ul>
        </section>
      )}
      {data.cycles.length > 0 && (
        <section>
          <h3>{t("diag.locks.cycles")}</h3>
          <ul className="plain-list">
            {data.cycles.map((c) => (
              <li key={c.join(",")}>{t("diag.locks.cycle", { pids: c.join(", ") })}</li>
            ))}
          </ul>
        </section>
      )}
      {empty && <p className="muted">{t("diag.locks.none")}</p>}
      {/* Подпись о границе инструмента — всегда: взаимные блокировки, уже разрешённые сервером, видны только в его логе. */}
      <p className="muted">{t("diag.locks.logNote")}</p>
    </div>
  );
}

/**
 * Блокировки (спец. P2 §7.4): DDL в очереди карточками сверху (самая частая причина «всё встало»), под ними — дерево
 * блокировок от корней-блокирующих с числом жертв, затем циклы ожидания.
 */
export function LocksView({ report }: { report: DiagReport }) {
  if (report.data.kind !== "locks") return null;
  return <LocksBody data={report.data} />;
}
