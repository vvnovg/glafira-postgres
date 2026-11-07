import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../api/client";
import type { AppError, ExplainParams, ParsedPlan, PlanTree, RequestSource } from "../../api/types";
import { formatShortTimestamp, formatTimestamp } from "../../lib/format";
import { useConnectionProfile } from "../../lib/useConnectionProfile";
import { useTopology } from "../../lib/useConnectionEvents";
import { type ExplainPrefill, tabId, useWorkspace } from "../../state/workspace";
import { SubTabs } from "../catalog/SubTabs";
import { ErrorBanner } from "../common/ErrorBanner";
import { CopyLlmButton } from "./CopyLlmButton";
import { ProposalsPanel } from "./llm/ProposalsPanel";
import { AskLlmButton, type SentNotice } from "./llm/AskLlmButton";
import { PlanRunner, type Substitution } from "./plan/PlanRunner";
import { PlanView } from "./plan/PlanView";
import { Recommendations } from "./plan/Recommendations";

type Source = "sql" | "paste";

/**
 * План последнего запуска в консоли и то, для чего он построен: текст, узел и база берутся из отрисовки, которая
 * создала запуск (замыкание `onResult`), — к моменту ответа SQL уже могли править.
 */
interface LastRun {
  plan: PlanTree;
  sql: string;
  node: string;
  db: string;
  /** Параметры запуска: значения `$N` уходят в снимок запроса на оптимизацию. */
  params: ExplainParams;
}

/** Источник запросов на оптимизацию консоли — одна константа: кнопка сравнивает его с запросами подключения. */
const CONSOLE_SOURCE: RequestSource = { kind: "console" };
const NO_PARAMS: (string | null)[] = [];
/** Сколько заменённых текстов помнит консоль. */
const REPLACED_MAX = 10;
/** Подпись заменённого текста — не длиннее (полный текст — в подсказке кнопки). */
const HEAD_MAX = 80;

/** Заменённый текст и когда его заменили. */
interface Replaced {
  text: string;
  at: number;
}

/** Непустые строки текста без отступов. */
const lines = (sql: string) =>
  sql
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l !== "");

/**
 * Подпись заменённого текста (смоук P4b, R4): первая непустая строка; если она та же, что у текста редактора (предложение
 * модели обычно начинается так же), — первая строка, которой тексты расходятся, с «…» впереди.
 */
function replacedHead(text: string, current: string): string {
  const own = lines(text);
  const now = lines(current);
  if (own.length === 0) return "";
  if (own[0] !== now[0]) return own[0].slice(0, HEAD_MAX);
  const i = own.findIndex((l, k) => l !== now[k]);
  return i < 0 ? own[0].slice(0, HEAD_MAX) : `… ${own[i]}`.slice(0, HEAD_MAX);
}

/**
 * Номер версии для `PlanRunner` из `Member.serverVersion` ("16.4", "17beta1"): мажорная × 10 000 + минорная. Нужна одна
 * проверка «16+» (GENERIC_PLAN), точный `server_version_num` не требуется. Версия неизвестна — 0 (GENERIC_PLAN недоступен).
 */
function versionNum(text: string | null | undefined): number {
  const m = /^(\d+)(?:\.(\d+))?/.exec(text ?? "");
  return m ? Number(m[1]) * 10_000 + Number(m[2] ?? 0) : 0;
}

/**
 * Консоль EXPLAIN (спец. P3 §8.7): узел, база, SQL и запуск планов через `PlanRunner`; режим «Вставить план» разбирает
 * текстовый план или JSON (в том числе с обрамлением psql) на месте, без обращения к серверу БД, и показывает рекомендации по нему. Одна на подключение: «Открыть в консоли»
 * подставляет узел, базу и текст сюда же, а открытие из дерева (без подстановки) оставляет набранное.
 * active — вкладка сейчас видна: скрытая список баз не читает, а при возвращении читает заново. Список нужен только режиму
 * «Запрос» и только пока база не известна (спека или выбор): «Вставить план» от него не зависит.
 * prefill — подстановка «Открыть в консоли EXPLAIN» из журнала MCP: применяется один раз на nonce.
 * Порядок режима «Запрос» (смоук P4b, R1b): редактор → «Заменённые тексты» → форма запуска (`PlanRunner`: параметры, ряд
 * кнопок с «Скопировать для LLM» и «Передать LLM-ассистенту» и их строками статуса, результат плана, «История») → панель
 * «Предложения LLM» (P4b): `prefill.proposalId` выбирает в ней предложение, а предложение на запрос, переданный отсюда,
 * выбирается само (R2).
 * Подстановка текста (`prefill`, «Вернуть») сразу убирает план прежнего текста (R6); «Вернуть» и подстановка предложения
 * (`prefill.autoPlan`) сразу строят «План» по новому тексту (R11), подстановка из журнала MCP — нет.
 * «Скопировать для LLM» получает план последнего запуска, пока SQL, узел и база не менялись, иначе Rust строит пробный план сам;
 * «Передать LLM-ассистенту» — тот же план и значения параметров последнего запуска (спец. P4 §8.1).
 */
export function ExplainTab({
  connectionId,
  member,
  database,
  sql: initialSql,
  prefill,
  active,
}: {
  connectionId: string;
  member: string | null | undefined;
  database: string | undefined;
  sql: string | undefined;
  prefill?: ExplainPrefill;
  active: boolean;
}) {
  const { t } = useTranslation();
  const view = useTopology(connectionId);
  const topo = view.data?.topology ?? null;
  const live = view.data?.state === "connected" || view.data?.state === "degraded";
  const dbs = useQuery({ queryKey: ["catalog", connectionId, "databases"], queryFn: () => api.listDatabases(connectionId), enabled: live && active });
  const [node, setNode] = useState<string | null>(member ?? null);
  const [db, setDb] = useState<string | null>(database ?? null);
  const profileDb = useConnectionProfile(connectionId)?.pg.database;
  const [sql, setSql] = useState(initialSql ?? "");
  const [source, setSource] = useState<Source>("sql");
  const [pasted, setPasted] = useState("");
  // Разобранный план хранится одним объектом: дерево плана сбрасывает развёртку при смене объекта.
  const [parsed, setParsed] = useState<ParsedPlan | null>(null);
  const [last, setLast] = useState<LastRun | null>(null);
  // Уведомление «строка скопирована» живёт здесь: кнопка перемонтируется вместе с запуском при смене узла или базы. Его
  // запрос — «свой» для панели предложений: первое предложение на него она выберет сама.
  const [sentNotice, setSentNotice] = useState<SentNotice | null>(null);
  // Тексты, которые подстановка («Открыть в консоли») заменила: запуски в истории хранят планы, а не текст редактора, и
  // при смене узла или базы история уходит вместе с запуском, поэтому прежний текст остаётся здесь и возвращается кнопкой.
  const [replaced, setReplaced] = useState<Replaced[]>([]);
  // Подстановка текста, которую запуск ещё не принял: он убирает прежний результат и, если нужно, строит «План».
  const [substitution, setSubstitution] = useState<Substitution | null>(null);
  const substitutions = useRef(0);
  const substitute = (plan: boolean) => {
    substitutions.current += 1;
    setSubstitution({ nonce: substitutions.current, plan });
  };
  const substituted = useCallback(() => setSubstitution(null), []);
  const [parsing, setParsing] = useState(false);
  const [parseError, setParseError] = useState<AppError | null>(null);
  // Повторное «Открыть в консоли» меняет спеку вкладки (при том же тексте — только её объект): новые узел, база и текст подставляются.
  const spec = useWorkspace((s) => s.tabs.find((x) => x.id === tabId({ kind: "explain", connectionId })));
  useEffect(() => {
    if (member !== undefined) setNode(member);
    if (database !== undefined) setDb(database);
    if (initialSql !== undefined) setSql(initialSql);
  }, [spec, member, database, initialSql]);
  // Подстановка из журнала MCP (и панели предложений P4b): SQL, база и узел ставятся один раз на nonce — перерисовка с той
  // же подстановкой правку пользователя не затирает; консоль переходит на «Запрос», план прежнего текста убирается, а
  // «План» строится только для предложения (`autoPlan`). Узла нет — лидер.
  useEffect(() => {
    if (!prefill) return;
    if (sql.trim() !== "" && sql !== prefill.sql) setReplaced((r) => [{ text: sql, at: Date.now() }, ...r.filter((x) => x.text !== sql)].slice(0, REPLACED_MAX));
    setSql(prefill.sql);
    setDb(prefill.database ?? null);
    setNode(prefill.member ?? null);
    setSource("sql");
    substitute(prefill.autoPlan === true);
  }, [prefill?.nonce]);

  if (view.data === undefined) {
    return <div className="page">{view.error ? <ErrorBanner error={view.error} onRetry={() => void view.refetch()} /> : <p className="empty">{t("common.loading")}</p>}</div>;
  }
  if (!live) {
    return (
      <div className="page">
        <p className="muted">{t("diag.closed")}</p>
      </div>
    );
  }
  const members = topo?.members ?? [];
  // Узла нет в спеке или он исчез из топологии — лидер; у списка без primary — первый участник.
  const leader = members.find((m) => m.role === "primary")?.name ?? members[0]?.name ?? null;
  const nodeName = node !== null && members.some((m) => m.name === node) ? node : leader;
  const dbNames = (dbs.data ?? []).map((d) => d.name);
  // Без выбранной базы — «БД по умолчанию» подключения, а не первая в списке сервера (обычно postgres).
  const dbName = db ?? (profileDb || null) ?? dbNames[0] ?? null;
  // База из спеки может отсутствовать в списке (удалена, список ещё не прочитан): выбор показывает её, а не пустое место.
  if (dbName !== null && !dbNames.includes(dbName)) dbNames.unshift(dbName);
  const memberInfo = members.find((m) => m.name === nodeName);
  const inRecovery = memberInfo ? memberInfo.role !== "primary" : false;
  const serverVersionNum = versionNum(memberInfo?.serverVersion);
  // Смена узла или базы забывает прошлый запуск совсем (его история ушла вместе с запуском прежнего узла и базы), и
  // возврат к ним план не воскрешает: сброс при отрисовке, без эффекта.
  if (last && (last.node !== nodeName || last.db !== dbName)) setLast(null);
  // «Скопировать для LLM» и запрос на оптимизацию получают план и значения параметров только пока консоль ещё в том
  // состоянии, для которого они получены.
  const lastFresh = last && last.sql === sql && last.node === nodeName && last.db === dbName ? last : null;
  const lastPlan = lastFresh?.plan ?? null;
  const lastParams = lastFresh && lastFresh.params.kind === "values" ? lastFresh.params.values : NO_PARAMS;

  const parse = async () => {
    setParseError(null);
    setParsing(true);
    try {
      setParsed(await api.parsePlan(pasted));
    } catch (e) {
      setParseError(toAppError(e));
    } finally {
      setParsing(false);
    }
  };

  return (
    <div className="page explain-tab">
      <div className="toolbar">
        <h2>{t("diag.explain.title")}</h2>
        {topo && topo.kind !== "server" && (
          <label className="inline">
            <span>{t("diag.header.node")}</span>
            <select aria-label={t("diag.header.node")} value={nodeName ?? ""} onChange={(e) => setNode(e.target.value)}>
              {members.map((m) => (
                <option key={m.name} value={m.name}>
                  {m.name}
                </option>
              ))}
            </select>
          </label>
        )}
        <label className="inline">
          <span>{t("diag.explain.database")}</span>
          <select aria-label={t("diag.explain.database")} value={dbName ?? ""} onChange={(e) => setDb(e.target.value)}>
            {dbNames.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
        </label>
      </div>
      {dbs.error && <ErrorBanner error={dbs.error} onRetry={() => void dbs.refetch()} />}
      <SubTabs<Source>
        label={t("diag.explain.source")}
        tabs={[
          { id: "sql", label: t("diag.explain.sourceSql") },
          { id: "paste", label: t("diag.explain.sourcePaste") },
        ]}
        value={source}
        onChange={setSource}
      >
        {(s) => (
          <>
            {/* Обе панели остаются смонтированными: переключение на «Вставить план» не стирает историю и значения параметров запуска. */}
            <div className="explain-panel" hidden={s !== "sql"}>
              <textarea className="mono explain-sql" aria-label="SQL" rows={8} value={sql} onChange={(e) => setSql(e.target.value)} placeholder={t("diag.explain.placeholder")} />
              {/* «Вернуть» — сразу под редактором (смоук P4b, R1a); подпись различает тексты с одинаковым началом (R4). */}
              {replaced.length > 0 && (
                <section className="llm-replaced" aria-label={t("diag.explain.replaced.title")}>
                  <h4>{t("diag.explain.replaced.title")}</h4>
                  <ul className="plain-list">
                    {replaced.map((r, i) => (
                      <li key={`${i}:${r.text}`}>
                        <span className="muted llm-replaced-time" title={formatTimestamp(r.at).slice(0, 19)}>
                          {formatShortTimestamp(r.at)}
                        </span>{" "}
                        <button
                          type="button"
                          className="link"
                          title={r.text}
                          onClick={() => {
                            setSql(r.text);
                            substitute(true);
                          }}
                        >
                          {t("diag.explain.replaced.restore", { head: replacedHead(r.text, sql) })}
                        </button>
                      </li>
                    ))}
                  </ul>
                </section>
              )}
              {nodeName && dbName ? (
                <PlanRunner
                  key={`${nodeName}:${dbName}`}
                  connectionId={connectionId}
                  member={nodeName}
                  database={dbName}
                  sql={sql}
                  serverVersionNum={serverVersionNum}
                  inRecovery={inRecovery}
                  onResult={(r, on, req) => setLast({ plan: r.plan, sql, node: on, db: dbName, params: req.params })}
                  substitution={substitution}
                  onSubstitution={substituted}
                  extraActions={
                    <>
                      {sql.trim() !== "" && <CopyLlmButton connectionId={connectionId} member={nodeName} database={dbName} source={{ kind: "sql", sql }} plan={lastPlan} />}
                      <AskLlmButton connectionId={connectionId} member={nodeName} database={dbName} source={CONSOLE_SOURCE} sql={sql} params={lastParams} plan={lastPlan} sent={sentNotice} onSent={setSentNotice} />
                    </>
                  }
                />
              ) : (
                // Базы ещё не знаем (список читается): запуску нужна база, режиму «Вставить план» — нет.
                dbs.isPending && <p className="empty">{t("common.loading")}</p>
              )}
              {/* Предложения LLM — после плана и истории (смоук P4b, R1b). */}
              {/* Ссылка карточки выбирает своё предложение по nonce подстановки; ответ на запрос отсюда выбирается сам (R2). */}
              <ProposalsPanel
                connectionId={connectionId}
                database={dbName}
                active={active}
                tabId={tabId({ kind: "explain", connectionId })}
                focus={prefill?.proposalId ? { id: prefill.proposalId, nonce: prefill.nonce } : null}
                follow={sentNotice?.id ?? null}
              />
            </div>
            <div className="explain-panel" hidden={s !== "paste"}>
              <p className="diag-note">{t("diag.explain.pasteHint")}</p>
              <textarea className="mono explain-sql" aria-label={t("diag.explain.pasteLabel")} rows={12} value={pasted} onChange={(e) => setPasted(e.target.value)} placeholder={t("diag.explain.pastePlaceholder")} />
              <div className="toolbar">
                <button type="button" className="primary" disabled={!pasted.trim() || parsing} onClick={() => void parse()}>
                  {t("diag.explain.parse")}
                </button>
              </div>
              {parseError && <ErrorBanner error={parseError} />}
              {/* Советы по вставленному плану — те же чистые правила; «Проверить гипотезу» нужен сервер — её нет. */}
              {parsed && <PlanView tree={parsed.plan} afterSummary={(showNode) => <Recommendations target={null} result={parsed} onShowNode={showNode} />} />}
            </div>
          </>
        )}
      </SubTabs>
    </div>
  );
}
