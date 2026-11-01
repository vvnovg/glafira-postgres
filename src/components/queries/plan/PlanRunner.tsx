import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { toAppError } from "../../../api/client";
import type { AppError, Confirmation, ExplainMode, ExplainParams, ExplainRequest, ExplainResult, ExplainTier, Member, RelationRef } from "../../../api/types";
import { formatDuration, formatMs, formatTime } from "../../../lib/format";
import { isStandby } from "../../../lib/topology";
import { canAct, useConnectionProfile } from "../../../lib/useConnectionProfile";
import { countParams, useExplain } from "../../../lib/useExplain";
import { ErrorBanner } from "../../common/ErrorBanner";
import { AnalyzeConfirm } from "./AnalyzeConfirm";
import { PARAM_FIELDS_LIMIT, ParamsForm } from "./ParamsForm";
import { PlanView } from "./PlanView";

/** Сроки ANALYZE в выборе, секунды (Rust принимает 5…600); пробный план всегда идёт со своим сроком 15 с. */
const TIMEOUTS_S = [5, 15, 30, 60, 300, 600] as const;
const DEFAULT_TIMEOUT_S = 30;
/** Прошлых планов в истории вкладки. */
const HISTORY = 10;
const NO_CONFIRMATION: Confirmation = { confirm: null, typedName: null };

// Ключи — литералами (правило словарей).
const MODE_KEYS: Record<ExplainMode, string> = { plan: "diag.plan.history.modes.plan", analyze: "diag.plan.history.modes.analyze" };
const TIER_KEYS: Record<ExplainTier, string> = { read: "diag.plan.tiers.read", write: "diag.plan.tiers.write" };

/**
 * Запуск в истории: результат хранится одним объектом — дерево плана сбрасывает развёртку при смене объекта. `member` —
 * узел, на котором план получен (ANALYZE чтения с лидера уходит на реплику): история и сводка называют его, если он не узел
 * вкладки.
 */
interface Run {
  id: number;
  at: number;
  result: ExplainResult;
  member: string;
}

/** Подсказка у кнопок: `force` — после `EXPLAIN_WRITES` можно выполнить как запись; остальные закрывают ярус записи. */
type WriteHint = "force" | "replica" | "readOnly";

/**
 * То, для чего получена проба: текст, значения параметров и `search_path`. Подтверждение яруса записи и «Выполнить как
 * запись» относятся только к ним — запрос после подтверждения собирается отсюда, а не из текущих полей.
 */
interface Inputs {
  sql: string;
  values: (string | null)[];
  searchPath: string;
}

/**
 * Ключ входа для сравнения: значения — только у существующих `$N` (лишние в массиве ничего не меняют). Свыше
 * `PARAM_FIELDS_LIMIT` полей нет, и значения во вход не входят.
 */
function inputsKey(i: Inputs): string {
  const n = countParams(i.sql);
  return JSON.stringify([i.sql, Array.from({ length: n > PARAM_FIELDS_LIMIT ? 0 : n }, (_, k) => i.values[k] ?? null), i.searchPath.trim()]);
}

/** Место ошибки разбора в тексте: строка и колонка (с 1) и строка текста, разрезанная вокруг отмеченного символа. */
interface ErrorPlace {
  line: number;
  column: number;
  before: string;
  at: string;
  after: string;
}

/**
 * `INVALID_INPUT` с `details: "position N"` (символы с 1, относительно текста пользователя) → место в `sql`; иначе `null`.
 * Позиция сразу за последним символом (неожиданный конец текста) отмечается пробелом в конце строки.
 */
function errorPlace(sql: string, error: AppError | null): ErrorPlace | null {
  if (error?.code !== "INVALID_INPUT" || !error.details) return null;
  const m = /^position (\d+)$/.exec(error.details.trim());
  if (!m) return null;
  const chars = Array.from(sql);
  const idx = Number(m[1]) - 1;
  if (idx < 0 || idx > chars.length) return null;
  let start = idx;
  while (start > 0 && chars[start - 1] !== "\n") start -= 1;
  let end = idx;
  while (end < chars.length && chars[end] !== "\n") end += 1;
  const line = chars.slice(0, start).filter((c) => c === "\n").length + 1;
  return {
    line,
    column: idx - start + 1,
    before: chars.slice(start, idx).join(""),
    at: idx < end ? chars[idx] : " ",
    after: idx < end ? chars.slice(idx + 1, end).join("") : "",
  };
}

/**
 * Запуск EXPLAIN (спец. P3 §8): параметры (`$N` → поля или GENERIC_PLAN на 16+), срок, `search_path`, «План» и «План с
 * выполнением». Ярус записи: пробный план → подтверждение со списком таблиц (всегда, и не на prod) → ANALYZE.
 * `EXPLAIN_WRITES` — от ANALYZE чтения или уже от пробного плана — даёт кнопку «Выполнить как запись» (`forceWrite`, тот же
 * путь подтверждения; Rust тогда проверяет ограду до пробы и строит её вне READ ONLY); `READ_ONLY` её не даёт — сервер
 * запись не примет. Подтверждение и подсказки относятся к тексту, параметрам и `search_path` пробы: их правка закрывает
 * открытый диалог и снимает подсказку. История запусков — в пределах компонента; ответ, пришедший после размонтирования
 * (смена источника текста, узла или базы), отбрасывается.
 * `members` — участники топологии: если узел — лидер, а проба показала ярус чтения, форма предлагает выполнить ANALYZE
 * на реплике (спец. §8.3 п. 3; по умолчанию — первая доступная реплика, можно вернуть лидера). `autoRun` — один пробный
 * план, как только запуск доступен
 * («Получить план» карточки); `onAutoRun` сообщает, что запрос принят (если «План» без значений невозможен — принят без
 * запуска). `onResult` получает результат и узел, где он получен.
 * Значения параметров: свыше `PARAM_FIELDS_LIMIT` полей нет, и запуски, которым нужны значения, закрыты; GENERIC_PLAN
 * влияет только на «План», поля значений для «Плана с выполнением» видны и при нём.
 * Ярус записи пишет журнал действий — после него список журнала перечитывается, как после диалогов P2.
 */
export function PlanRunner({
  connectionId,
  member,
  database,
  sql,
  serverVersionNum,
  inRecovery,
  members,
  onResult,
  autoRun,
  onAutoRun,
  extraActions,
}: {
  connectionId: string;
  member: string;
  database: string;
  sql: string;
  serverVersionNum: number;
  inRecovery: boolean;
  members?: Member[] | null;
  onResult?: (r: ExplainResult, member: string) => void;
  autoRun?: boolean;
  onAutoRun?: () => void;
  /** Кнопки в ряду копирования результата (рядом с «JSON» и «Как текст»); без результата — отдельным рядом. */
  extraActions?: ReactNode;
}) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const profile = useConnectionProfile(connectionId);
  const count = countParams(sql);
  const canGeneric = serverVersionNum >= 160_000 && count > 0;
  const [generic, setGeneric] = useState(true);
  // Параметров больше предела — полей значений нет, и запуск, которому они нужны, закрыт; «План» с GENERIC_PLAN (16+)
  // значений не требует.
  const genericOn = canGeneric && generic;
  const tooMany = count > PARAM_FIELDS_LIMIT;
  const needsValues = (mode: ExplainMode) => count > 0 && !(mode === "plan" && genericOn);
  const planBlocked = tooMany && needsValues("plan");
  const analyzeBlocked = tooMany && needsValues("analyze");
  const [values, setValues] = useState<(string | null)[]>([]);
  const [timeoutS, setTimeoutS] = useState<number>(DEFAULT_TIMEOUT_S);
  const [searchPath, setSearchPath] = useState("");
  const [runs, setRuns] = useState<Run[]>([]);
  const [shown, setShown] = useState<number | null>(null);
  /** Открытое подтверждение яруса записи: таблицы пробного плана, `forceWrite` и вход пробы. */
  const [confirm, setConfirm] = useState<{ relations: RelationRef[]; force: boolean; inputs: Inputs; admin?: boolean } | null>(null);
  /** Ошибка запуска и текст, для которого он шёл: место ошибки разбора отмечается в нём, а не в текущем поле. */
  const [failed, setFailed] = useState<{ error: AppError; sql: string } | null>(null);
  const [writeHint, setWriteHint] = useState<WriteHint | null>(null);
  /** Таблицы пробного плана и вход того запуска, что получил `EXPLAIN_WRITES`: их покажет подтверждение «как запись». */
  const [forceFrom, setForceFrom] = useState<{ relations: RelationRef[]; inputs: Inputs } | null>(null);
  /** Ключ входа, чья проба показала ярус чтения: для него предлагается узел ANALYZE. */
  const [readProbe, setReadProbe] = useState<string | null>(null);
  /** Узел ANALYZE чтения, выбранный пользователем; `null` — по умолчанию (первая доступная реплика). */
  const [analyzeOn, setAnalyzeOn] = useState<string | null>(null);
  // Запуск из двух вызовов (пробный план и ANALYZE) — одна операция: кнопки ждут её целиком.
  const [running, setRunning] = useState(false);
  const explain = useExplain(connectionId);
  const seq = useRef(0);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const inputs: Inputs = { sql, values, searchPath };
  const key = inputsKey(inputs);
  // Правка текста, значений или search_path: подтверждение и подсказки прошлой пробы к новому входу не относятся.
  // `liveKey` читают продолжения запусков: проба, вернувшаяся после правки, диалог уже не открывает.
  const liveKey = useRef(key);
  useEffect(() => {
    if (liveKey.current === key) return;
    liveKey.current = key;
    setConfirm(null);
    setWriteHint(null);
    setForceFrom(null);
    setFailed(null);
  }, [key]);
  const stale = (from: Inputs) => !alive.current || inputsKey(from) !== liveKey.current;

  const leader = !inRecovery && members?.find((m) => m.name === member)?.role === "primary";
  const replicas = leader ? (members ?? []).filter((m) => m.name !== member && m.reachable && isStandby(m.role)) : [];
  const offerReplica = replicas.length > 0 && readProbe === key;
  // Узел ANALYZE чтения: выбор пользователя, пока он доступен (лидер или реплика из списка), иначе первая доступная
  // реплика; без реплик (или узел — не лидер) — узел вкладки.
  const analyzeTarget =
    replicas.length === 0 ? member : analyzeOn !== null && (analyzeOn === member || replicas.some((m) => m.name === analyzeOn)) ? analyzeOn : replicas[0].name;

  const params = (mode: ExplainMode, from: Inputs): ExplainParams => {
    const n = countParams(from.sql);
    if (n === 0) return { kind: "none" };
    if (mode === "plan" && generic && serverVersionNum >= 160_000) return { kind: "generic" };
    return { kind: "values", values: Array.from({ length: n }, (_, i) => from.values[i] ?? null) };
  };
  const request = (mode: ExplainMode, confirmation: Confirmation, forceWrite: boolean, from: Inputs, on = member): ExplainRequest => ({
    member: on,
    database,
    sql: from.sql,
    mode,
    params: params(mode, from),
    timeoutS,
    searchPath: from.searchPath.trim() || null,
    confirmation,
    forceWrite,
  });

  const remember = (result: ExplainResult, on: string) => {
    if (!alive.current) return;
    seq.current += 1;
    const run: Run = { id: seq.current, at: Date.now(), result, member: on };
    setRuns((r) => [run, ...r].slice(0, HISTORY));
    setShown(run.id);
    onResult?.(result, on);
  };

  /** Запуск входа `from`: прошлые ошибка и подсказка снимаются, новая ошибка — баннером (после размонтирования — никуда). */
  const guarded = async (from: Inputs, step: () => Promise<void>) => {
    setFailed(null);
    setWriteHint(null);
    setRunning(true);
    try {
      await step();
    } catch (e) {
      if (alive.current) setFailed({ error: toAppError(e), sql: from.sql });
    } finally {
      if (alive.current) setRunning(false);
    }
  };

  /** Ярус записи: read-only профиль и реплика закрыты подсказкой (Rust проверит и сам), иначе — подтверждение. */
  const openWrite = (relations: RelationRef[], force: boolean, from: Inputs, admin = false) => {
    if (!canAct(profile)) setWriteHint("readOnly");
    else if (inRecovery) setWriteHint("replica");
    else setConfirm({ relations, force, inputs: from, admin });
  };

  /** `EXPLAIN_WRITES`: повтор «как запись» пойдёт ярусом записи с подтверждением (таблицы — какие известны). */
  const offerForce = (e: unknown, relations: RelationRef[], from: Inputs) => {
    if (toAppError(e).code !== "EXPLAIN_WRITES" || stale(from)) return;
    setForceFrom({ relations, inputs: from });
    setWriteHint("force");
  };

  const runPlan = () => {
    const from = inputs;
    return guarded(from, async () => {
      const r = await explain.mutateAsync(request("plan", NO_CONFIRMATION, false, from));
      if (alive.current) setReadProbe(r.tier === "read" ? inputsKey(from) : null);
      remember(r, member);
    });
  };

  /**
   * ANALYZE: ярус определяет пробный план; чтение выполняется сразу — на выбранном узле, по умолчанию на реплике (выбор
   * появляется после пробы чтения), запись — после подтверждения и всегда на узле вкладки.
   */
  const runAnalyze = () => {
    const from = inputs;
    const on = analyzeTarget;
    return guarded(from, async () => {
      let probe: ExplainResult;
      try {
        probe = await explain.mutateAsync(request("plan", NO_CONFIRMATION, false, from));
      } catch (e) {
        // Побочный эффект уже при планировании: проба отказала в READ ONLY, таблиц не знаем.
        offerForce(e, [], from);
        throw e;
      }
      // Вход изменился, пока шла проба (или запуск размонтирован): ни подтверждения, ни ANALYZE прежнего текста.
      if (stale(from)) return;
      if (probe.tier === "write") {
        setReadProbe(null);
        openWrite(probe.relations, false, from, probe.warnings.includes("adminFunction"));
        return;
      }
      setReadProbe(inputsKey(from));
      try {
        remember(await explain.mutateAsync(request("analyze", NO_CONFIRMATION, false, from, on)), on);
      } catch (e) {
        // Чтение записало (volatile-функция, триггер).
        offerForce(e, probe.relations, from);
        throw e;
      }
    });
  };

  const runAsWrite = () => {
    if (!forceFrom) return;
    setFailed(null);
    setWriteHint(null);
    openWrite(forceFrom.relations, true, forceFrom.inputs);
  };

  const confirmed = (c: Confirmation) => {
    if (!confirm) return;
    const { force, inputs: from } = confirm;
    // Подтверждение остаётся открытым (кнопки ждут), пока ANALYZE идёт; закрывается итогом — результатом или ошибкой.
    // Запрос — из входа пробы: подтверждали именно его. Ярус записи — на узле вкладки; журнал действий перечитывается.
    return guarded(from, async () => {
      try {
        remember(await explain.mutateAsync(request("analyze", c, force, from)), member);
      } finally {
        void qc.invalidateQueries({ queryKey: ["actions", connectionId] });
        if (alive.current) setConfirm(null);
      }
    });
  };

  const current = runs.find((r) => r.id === shown) ?? null;
  const error = failed?.error ?? null;
  const place = failed ? errorPlace(failed.sql, failed.error) : null;
  // Отказ или ошибка последнего запуска: на экране не должно остаться прежнего результата, похожего на текущий.
  const refused = failed !== null || writeHint === "replica" || writeHint === "readOnly";
  const busy = running || explain.isPending;
  // Профиль ещё читается: ни подсказки read-only, ни запуска — кнопки ждут.
  const loading = profile === undefined;
  const confirmOpen = confirm !== null && inputsKey(confirm.inputs) === key;
  const idle = !busy && !loading && !confirmOpen && sql.trim() !== "";

  // «Получить план» карточки: одна проба, как только запуск доступен. Флаг «принято» держит одну пробу на запрос и при
  // повторном вызове эффекта (StrictMode, новая `onAutoRun`), пока родитель не снял `autoRun`. «План» без значений
  // невозможен (параметров больше предела, GENERIC_PLAN нет) — запрос принимается без запуска.
  const runPlanRef = useRef(runPlan);
  runPlanRef.current = runPlan;
  const autoTaken = useRef(false);
  useEffect(() => {
    if (!autoRun) {
      autoTaken.current = false;
      return;
    }
    if (!idle || autoTaken.current) return;
    autoTaken.current = true;
    onAutoRun?.();
    if (!planBlocked) void runPlanRef.current();
  }, [autoRun, idle, onAutoRun, planBlocked]);

  return (
    <div className="plan-runner">
      <ParamsForm count={count} values={values} onChange={setValues} generic={generic} canGeneric={canGeneric} onGeneric={setGeneric} />
      <div className="toolbar">
        <button type="button" disabled={!idle || planBlocked} onClick={() => void runPlan()}>
          {genericOn ? t("diag.plan.run.planGeneric") : t("diag.plan.run.plan")}
        </button>
        <button type="button" disabled={!idle || analyzeBlocked} onClick={() => void runAnalyze()}>
          {t("diag.plan.run.analyze")}
        </button>
        {loading && <span className="muted">{t("common.loading")}</span>}
        <label className="inline">
          <span>{t("diag.plan.run.timeout")}</span>
          <select aria-label={t("diag.plan.run.timeout")} value={timeoutS} onChange={(e) => setTimeoutS(Number(e.target.value))}>
            {TIMEOUTS_S.map((s) => (
              <option key={s} value={s}>
                {formatDuration(s)}
              </option>
            ))}
          </select>
        </label>
        {offerReplica && (
          <label className="inline">
            <span>{t("diag.plan.run.analyzeOn")}</span>
            <select aria-label={t("diag.plan.run.analyzeOn")} value={analyzeTarget} disabled={busy} onChange={(e) => setAnalyzeOn(e.target.value)}>
              {[member, ...replicas.map((m) => m.name)].map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </select>
            {analyzeTarget !== member && <span className="muted">{t("diag.plan.run.replicaPlanDiffers")}</span>}
          </label>
        )}
        <details className="search-path">
          <summary>search_path</summary>
          <input type="text" aria-label="search_path" value={searchPath} placeholder={t("diag.plan.run.searchPathDefault")} onChange={(e) => setSearchPath(e.target.value)} />
        </details>
      </div>
      {error && <ErrorBanner error={error} />}
      {place && (
        <div className="sql-error">
          <p className="muted">{t("diag.plan.run.errorAt", { line: place.line, column: place.column })}</p>
          <pre className="query-text">
            <code>
              {place.before}
              <mark>{place.at}</mark>
              {place.after}
            </code>
          </pre>
        </div>
      )}
      {writeHint === "force" && forceFrom && (
        <p className="diag-note" role="status">
          {t("diag.plan.run.writesHint")}{" "}
          <button type="button" disabled={!idle} onClick={runAsWrite}>
            {t("diag.plan.run.asWrite")}
          </button>
        </p>
      )}
      {writeHint === "replica" && (
        <p className="error-banner" role="status">
          {t("diag.plan.run.writeOnReplica")}
        </p>
      )}
      {writeHint === "readOnly" && (
        <p className="error-banner" role="status">
          {t("diag.plan.run.writeReadOnly")}
        </p>
      )}
      {confirmOpen && profile && (
        <AnalyzeConfirm connection={profile} relations={confirm.relations} admin={confirm.admin} busy={busy} onConfirm={(c) => void confirmed(c)} onCancel={() => setConfirm(null)} />
      )}
      {/* После отказа или ошибки запуска результат прошлого запуска как текущий не показывается; история остаётся. */}
      {current && !refused && (
        <PlanView
          tree={current.result.plan}
          tier={current.result.tier}
          mode={current.result.mode}
          warnings={current.result.warnings}
          raw={current.result.raw}
          elapsedMs={current.result.elapsedMs}
          member={current.member !== member ? current.member : undefined}
          extraActions={extraActions}
        />
      )}
      {(!current || refused) && extraActions && <div className="toolbar">{extraActions}</div>}
      {runs.length > 1 && (
        <section className="plan-history">
          <h3>{t("diag.plan.history.title")}</h3>
          <ul className="plain-list">
            {runs.map((r) => (
              <li key={r.id}>
                <button
                  type="button"
                  className="link"
                  aria-current={r.id === shown}
                  onClick={() => {
                    // Выбор запуска из истории снимает прежний отказ: показывается выбранный результат.
                    setFailed(null);
                    setWriteHint(null);
                    setShown(r.id);
                  }}
                >
                  {r.member === member
                    ? t("diag.plan.history.item", { n: r.id, mode: t(MODE_KEYS[r.result.mode]), tier: t(TIER_KEYS[r.result.tier]), time: formatTime(r.at), ms: formatMs(r.result.elapsedMs) })
                    : t("diag.plan.history.itemOn", { n: r.id, mode: t(MODE_KEYS[r.result.mode]), tier: t(TIER_KEYS[r.result.tier]), member: r.member, time: formatTime(r.at), ms: formatMs(r.result.elapsedMs) })}
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
