import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toAppError } from "../../../api/client";
import type { AppError, Confirmation, ExplainMode, ExplainParams, ExplainRequest, ExplainResult, ExplainTier, Member, PlanNode, RelationRef } from "../../../api/types";
import { formatDuration, formatMs, formatTime } from "../../../lib/format";
import { canAct, useConnectionProfile } from "../../../lib/useConnectionProfile";
import { countParams, useExplain } from "../../../lib/useExplain";
import { ErrorBanner } from "../../common/ErrorBanner";
import { AnalyzeConfirm } from "./AnalyzeConfirm";
import { ParamsForm } from "./ParamsForm";
import { PlanNodeDetails } from "./PlanNodeDetails";
import { PlanSummary } from "./PlanSummary";
import { PlanTreeView } from "./PlanTree";

/** Сроки ANALYZE в выборе, секунды (Rust принимает 5…600); пробный план всегда идёт со своим сроком 15 с. */
const TIMEOUTS_S = [5, 15, 30, 60, 300, 600] as const;
const DEFAULT_TIMEOUT_S = 30;
/** Прошлых планов в истории вкладки. */
const HISTORY = 10;
const NO_CONFIRMATION: Confirmation = { confirm: null, typedName: null };

// Ключи — литералами (правило словарей).
const MODE_KEYS: Record<ExplainMode, string> = { plan: "diag.plan.history.modes.plan", analyze: "diag.plan.history.modes.analyze" };
const TIER_KEYS: Record<ExplainTier, string> = { read: "diag.plan.tiers.read", write: "diag.plan.tiers.write" };

/** Запуск в истории: результат хранится одним объектом — дерево плана сбрасывает развёртку при смене объекта. */
interface Run {
  id: number;
  at: number;
  result: ExplainResult;
}

/** Подсказка у кнопок: `force` — после `EXPLAIN_WRITES` можно выполнить как запись; остальные закрывают ярус записи. */
type WriteHint = "force" | "replica" | "readOnly";

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

function findNode(root: PlanNode, id: number | null): PlanNode | null {
  if (id === null) return null;
  if (root.id === id) return root;
  for (const c of root.children) {
    const f = findNode(c, id);
    if (f) return f;
  }
  return null;
}

/**
 * Запуск EXPLAIN (спец. P3 §8): параметры (`$N` → поля или GENERIC_PLAN на 16+), срок, `search_path`, «План» и «План с
 * выполнением». Ярус записи: пробный план → подтверждение со списком таблиц (всегда, и не на prod) → ANALYZE.
 * `EXPLAIN_WRITES` после ANALYZE чтения даёт кнопку «Выполнить как запись» (`forceWrite`, тот же путь подтверждения);
 * `READ_ONLY` её не даёт — сервер запись не примет. История запусков — в пределах компонента.
 * `members` — участники топологии (резерв: выбор реплики для ANALYZE чтения, спец. §8.3 п. 3, в P3a не используется).
 */
export function PlanRunner({
  connectionId,
  member,
  database,
  sql,
  serverVersionNum,
  inRecovery,
  onResult,
}: {
  connectionId: string;
  member: string;
  database: string;
  sql: string;
  serverVersionNum: number;
  inRecovery: boolean;
  members?: Member[] | null;
  onResult?: (r: ExplainResult) => void;
}) {
  const { t } = useTranslation();
  const profile = useConnectionProfile(connectionId);
  const count = countParams(sql);
  const canGeneric = serverVersionNum >= 160_000 && count > 0;
  const [generic, setGeneric] = useState(true);
  const [values, setValues] = useState<(string | null)[]>([]);
  const [timeoutS, setTimeoutS] = useState<number>(DEFAULT_TIMEOUT_S);
  const [searchPath, setSearchPath] = useState("");
  const [runs, setRuns] = useState<Run[]>([]);
  const [shown, setShown] = useState<number | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  /** Открытое подтверждение яруса записи: таблицы пробного плана и `forceWrite`. */
  const [confirm, setConfirm] = useState<{ relations: RelationRef[]; force: boolean } | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [writeHint, setWriteHint] = useState<WriteHint | null>(null);
  /** Таблицы пробного плана того ANALYZE, что получил `EXPLAIN_WRITES`: их покажет подтверждение «как запись». */
  const [forceRelations, setForceRelations] = useState<RelationRef[]>([]);
  // Запуск из двух вызовов (пробный план и ANALYZE) — одна операция: кнопки ждут её целиком.
  const [running, setRunning] = useState(false);
  const explain = useExplain(connectionId);
  const seq = useRef(0);

  const params = (mode: ExplainMode): ExplainParams => {
    if (count === 0) return { kind: "none" };
    if (mode === "plan" && generic && canGeneric) return { kind: "generic" };
    return { kind: "values", values: Array.from({ length: count }, (_, i) => values[i] ?? null) };
  };
  const request = (mode: ExplainMode, confirmation: Confirmation, forceWrite: boolean): ExplainRequest => ({
    member,
    database,
    sql,
    mode,
    params: params(mode),
    timeoutS,
    searchPath: searchPath.trim() || null,
    confirmation,
    forceWrite,
  });

  const remember = (result: ExplainResult) => {
    seq.current += 1;
    const run: Run = { id: seq.current, at: Date.now(), result };
    setRuns((r) => [run, ...r].slice(0, HISTORY));
    setShown(run.id);
    setSelected(null);
    onResult?.(result);
  };

  /** Запуск: прошлые ошибка и подсказка снимаются, новая ошибка — баннером. */
  const guarded = async (step: () => Promise<void>) => {
    setError(null);
    setWriteHint(null);
    setRunning(true);
    try {
      await step();
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setRunning(false);
    }
  };

  /** Ярус записи: read-only профиль и реплика закрыты подсказкой (Rust проверит и сам), иначе — подтверждение. */
  const openWrite = (relations: RelationRef[], force: boolean) => {
    if (!canAct(profile)) setWriteHint("readOnly");
    else if (inRecovery) setWriteHint("replica");
    else setConfirm({ relations, force });
  };

  const runPlan = () => guarded(async () => remember(await explain.mutateAsync(request("plan", NO_CONFIRMATION, false))));

  /** ANALYZE: ярус определяет пробный план; чтение выполняется сразу, запись — только после подтверждения. */
  const runAnalyze = () =>
    guarded(async () => {
      const probe = await explain.mutateAsync(request("plan", NO_CONFIRMATION, false));
      if (probe.tier === "write") {
        openWrite(probe.relations, false);
        return;
      }
      try {
        remember(await explain.mutateAsync(request("analyze", NO_CONFIRMATION, false)));
      } catch (e) {
        // Чтение записало (volatile-функция, триггер): повтор «как запись» пойдёт ярусом записи с подтверждением.
        // Отказ самого пробного плана повтором не лечится — Rust снова построит его в READ ONLY, — поэтому только здесь.
        if (toAppError(e).code === "EXPLAIN_WRITES") {
          setForceRelations(probe.relations);
          setWriteHint("force");
        }
        throw e;
      }
    });

  const runAsWrite = () => {
    setError(null);
    setWriteHint(null);
    openWrite(forceRelations, true);
  };

  const confirmed = (c: Confirmation) => {
    const force = confirm?.force ?? false;
    // Подтверждение остаётся открытым (кнопки ждут), пока ANALYZE идёт; закрывается итогом — результатом или ошибкой.
    return guarded(async () => {
      try {
        remember(await explain.mutateAsync(request("analyze", c, force)));
      } finally {
        setConfirm(null);
      }
    });
  };

  const current = runs.find((r) => r.id === shown) ?? null;
  const node = current ? findNode(current.result.plan.root, selected) : null;
  const place = errorPlace(sql, error);
  const busy = running || explain.isPending;
  const idle = !busy && confirm === null && sql.trim() !== "";
  return (
    <div className="plan-runner">
      <ParamsForm count={count} values={values} onChange={setValues} generic={generic} canGeneric={canGeneric} onGeneric={setGeneric} />
      <div className="toolbar">
        <button type="button" className="primary" disabled={!idle} onClick={() => void runPlan()}>
          {t("diag.plan.run.plan")}
        </button>
        <button type="button" disabled={!idle} onClick={() => void runAnalyze()}>
          {t("diag.plan.run.analyze")}
        </button>
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
      {writeHint === "force" && (
        <p className="diag-note" role="status">
          {t("diag.plan.run.writesHint")}{" "}
          <button type="button" disabled={!idle} onClick={runAsWrite}>
            {t("diag.plan.run.asWrite")}
          </button>
        </p>
      )}
      {writeHint === "replica" && (
        <p className="diag-note" role="status">
          {t("diag.plan.run.writeOnReplica")}
        </p>
      )}
      {writeHint === "readOnly" && (
        <p className="diag-note" role="status">
          {t("diag.plan.run.writeReadOnly")}
        </p>
      )}
      {confirm && profile && (
        <AnalyzeConfirm connection={profile} relations={confirm.relations} busy={busy} onConfirm={(c) => void confirmed(c)} onCancel={() => setConfirm(null)} />
      )}
      {current && (
        <>
          <PlanSummary tree={current.result.plan} tier={current.result.tier} warnings={current.result.warnings} raw={current.result.raw} elapsedMs={current.result.elapsedMs} />
          <PlanTreeView tree={current.result.plan} selectedId={selected} onSelect={setSelected} />
          {node && <PlanNodeDetails node={node} />}
        </>
      )}
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
                    setShown(r.id);
                    setSelected(null);
                  }}
                >
                  {t("diag.plan.history.item", { n: r.id, mode: t(MODE_KEYS[r.result.mode]), tier: t(TIER_KEYS[r.result.tier]), time: formatTime(r.at), ms: formatMs(r.result.elapsedMs) })}
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
