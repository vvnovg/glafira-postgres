import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../api/client";
import type { AppError, PlanTree } from "../../api/types";
import { useTopology } from "../../lib/useConnectionEvents";
import { tabId, useWorkspace } from "../../state/workspace";
import { SubTabs } from "../catalog/SubTabs";
import { ErrorBanner } from "../common/ErrorBanner";
import { CopyLlmButton } from "./CopyLlmButton";
import { PlanRunner } from "./plan/PlanRunner";
import { PlanView } from "./plan/PlanView";

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
 * JSON (в том числе с обрамлением psql) на месте, без обращения к серверу БД. Одна на подключение: «Открыть в консоли»
 * подставляет узел, базу и текст сюда же, а открытие из дерева (без подстановки) оставляет набранное.
 * active — вкладка сейчас видна: скрытая список баз не читает, а при возвращении читает заново. Список нужен только режиму
 * «Запрос» и только пока база не известна (спека или выбор): «Вставить план» от него не зависит.
 * «Скопировать для LLM» получает план последнего запуска, пока SQL, узел и база не менялись, иначе Rust строит пробный план сам.
 */
export function ExplainTab({
  connectionId,
  member,
  database,
  sql: initialSql,
  active,
}: {
  connectionId: string;
  member: string | null | undefined;
  database: string | undefined;
  sql: string | undefined;
  active: boolean;
}) {
  const { t } = useTranslation();
  const view = useTopology(connectionId);
  const topo = view.data?.topology ?? null;
  const live = view.data?.state === "connected" || view.data?.state === "degraded";
  const dbs = useQuery({ queryKey: ["catalog", connectionId, "databases"], queryFn: () => api.listDatabases(connectionId), enabled: live && active });
  const [node, setNode] = useState<string | null>(member ?? null);
  const [db, setDb] = useState<string | null>(database ?? null);
  const [sql, setSql] = useState(initialSql ?? "");
  const [source, setSource] = useState<Source>("sql");
  const [pasted, setPasted] = useState("");
  // Разобранный план хранится одним объектом: дерево плана сбрасывает развёртку при смене объекта.
  const [parsed, setParsed] = useState<PlanTree | null>(null);
  const [last, setLast] = useState<LastRun | null>(null);
  const [parsing, setParsing] = useState(false);
  const [parseError, setParseError] = useState<AppError | null>(null);
  // Повторное «Открыть в консоли» меняет спеку вкладки (при том же тексте — только её объект): новые узел, база и текст подставляются.
  const spec = useWorkspace((s) => s.tabs.find((x) => x.id === tabId({ kind: "explain", connectionId })));
  useEffect(() => {
    if (member !== undefined) setNode(member);
    if (database !== undefined) setDb(database);
    if (initialSql !== undefined) setSql(initialSql);
  }, [spec, member, database, initialSql]);

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
  const dbName = db ?? dbNames[0] ?? null;
  // База из спеки может отсутствовать в списке (удалена, список ещё не прочитан): выбор показывает её, а не пустое место.
  if (dbName !== null && !dbNames.includes(dbName)) dbNames.unshift(dbName);
  const memberInfo = members.find((m) => m.name === nodeName);
  const inRecovery = memberInfo ? memberInfo.role !== "primary" : false;
  const serverVersionNum = versionNum(memberInfo?.serverVersion);
  // Для «Скопировать для LLM» план идёт только пока консоль ещё в том состоянии, для которого он построен.
  const lastPlan = last && last.sql === sql && last.node === nodeName && last.db === dbName ? last.plan : null;

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
              {nodeName && dbName ? (
                <>
                  <PlanRunner
                    key={`${nodeName}:${dbName}`}
                    connectionId={connectionId}
                    member={nodeName}
                    database={dbName}
                    sql={sql}
                    serverVersionNum={serverVersionNum}
                    inRecovery={inRecovery}
                    onResult={(r) => setLast({ plan: r.plan, sql, node: nodeName, db: dbName })}
                  />
                  {sql.trim() && (
                    <div className="toolbar">
                      <CopyLlmButton connectionId={connectionId} member={nodeName} database={dbName} source={{ kind: "sql", sql }} plan={lastPlan} />
                    </div>
                  )}
                </>
              ) : (
                // Базы ещё не знаем (список читается): запуску нужна база, режиму «Вставить план» — нет.
                dbs.isPending && <p className="empty">{t("common.loading")}</p>
              )}
            </div>
            <div className="explain-panel" hidden={s !== "paste"}>
              <textarea className="mono explain-sql" aria-label={t("diag.explain.pasteLabel")} rows={12} value={pasted} onChange={(e) => setPasted(e.target.value)} placeholder={t("diag.explain.pastePlaceholder")} />
              <div className="toolbar">
                <button type="button" className="primary" disabled={!pasted.trim() || parsing} onClick={() => void parse()}>
                  {t("diag.explain.parse")}
                </button>
              </div>
              {parseError && <ErrorBanner error={parseError} />}
              {parsed && <PlanView tree={parsed} />}
            </div>
          </>
        )}
      </SubTabs>
    </div>
  );
}
