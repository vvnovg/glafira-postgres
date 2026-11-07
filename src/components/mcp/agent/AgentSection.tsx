import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/client";
import type { AgentDetected, AgentPreset, AgentSettings, McpStatus } from "../../../api/types";
import { formatCount } from "../../../lib/format";
import { isActive, PRESET_NAMES, useAgentRuns } from "../../../lib/useAgentRuns";
import { MCP_STATUS_KEY } from "../../../lib/useMcpStatus";
import { tabId, useWorkspace } from "../../../state/workspace";
import { ErrorBanner } from "../../common/ErrorBanner";
import { Hint } from "../../common/Hint";
import { AgentLogDialog } from "./AgentLogDialog";
import { AgentRuns } from "./AgentRuns";
import { type AgentDraft, type FieldError, fromDraft, hiddenErrorPreset, isLoopbackUrl, MIN_VERSION, needsModel, pathRules, toDraft, unloadOff, validateDraft } from "./agentForm";
import { checkResultText } from "./agentText";

/** Ключ настроек фонового ассистента в кэше запросов. */
export const AGENT_SETTINGS_KEY = ["agentSettings"] as const;
const PRESETS: AgentPreset[] = ["qwen", "claude"];

/** Почему выключатель неактивен (спец. P4c §8.3). Ключи — литералами. */
type Block = "policy" | "server" | "serverError" | "otherInstance";
const BLOCK_KEYS: Record<Block, string> = {
  policy: "settings.mcp.policy",
  server: "settings.mcp.agent.serverOff",
  serverError: "settings.mcp.agent.serverError",
  otherInstance: "settings.mcp.agent.otherInstance",
};

/**
 * Политика, затем MCP-сервер: фоновый ассистент работает только в окне, которое обслуживает MCP (спец. P4c §14). Сервер
 * включён, но не запустился (`error`), — своя пометка: «включите» тут не поможет.
 */
function toggleBlock(s: McpStatus): Block | null {
  if (!s.agent.policyEnabled) return "policy";
  switch (s.server) {
    case "running":
      return null;
    case "otherInstance":
      return "otherInstance";
    case "error":
      return "serverError";
    default:
      return "server";
  }
}

/** Пояснение, почему «Выгрузить модель» неактивна (кроме нелокального адреса — о нём пометка у поля). Ключи — литералами. */
const UNLOAD_OFF_KEYS = {
  noLocalModel: "settings.mcp.agent.unloadNoLocalModel",
  keepWarmOff: "settings.mcp.agent.unloadKeepWarmOff",
} as const;

/** Атрибуты поля с ошибкой: `aria-invalid` и связь с текстом ошибки (и с пояснениями поля `notes` — их id). */
function invalid(id: string, e: FieldError | undefined, notes: string[] = []) {
  const describedBy = [...(e ? [`${id}-error`] : []), ...notes].join(" ");
  return { ...(e ? { "aria-invalid": true as const } : {}), ...(describedBy ? { "aria-describedby": describedBy } : {}) };
}

/**
 * Строка формы: подпись с подсказкой; поле (с кнопкой рядом), под ним — текст ошибки и пояснения (`below`). Числа пределов в
 * ошибке — по правилам языка («0,01»).
 */
function Field({ id, label, hint, error, below, children }: { id: string; label: string; hint: string; error?: FieldError; below?: ReactNode; children: ReactNode }) {
  const { t } = useTranslation();
  const params = error?.params && Object.fromEntries(Object.entries(error.params).map(([k, v]) => [k, typeof v === "number" ? formatCount(v, Number.isInteger(v) ? 0 : 2) : v]));
  return (
    <>
      <span className="field-label">
        <label htmlFor={id}>{label}</label>
        <Hint k={hint} label={label} />
      </span>
      <div className="agent-control">
        <span className="agent-field">{children}</span>
        {error && (
          <p id={`${id}-error`} className="agent-error">
            {t(error.key, params)}
          </p>
        )}
        {below}
      </div>
    </>
  );
}

/**
 * «Настройки» → «LLM-ассистенты (MCP)» → «Фоновый ассистент» (спец. P4c §8.3). Постоянная пометка о маскировании и
 * расходах; форма — после загрузки настроек. F1 в подразделе — раздел `background` главы `llm-mcp` (метка
 * `data-help-anchor`, её читает `SettingsPanel`). Открытие «Настроек» индикатором во время запуска (`section: "agent"`)
 * прокручивает подраздел в видимую часть, когда форма нарисована.
 */
export function AgentSection({ status }: { status: McpStatus }) {
  const { t } = useTranslation();
  const settings = useQuery({ queryKey: AGENT_SETTINGS_KEY, queryFn: () => api.getAgentSettings() });
  const rootRef = useRef<HTMLElement>(null);
  const spec = useWorkspace((st) => st.tabs.find((x) => x.id === tabId({ kind: "settings" })));
  const loaded = settings.data !== undefined;
  useEffect(() => {
    // Каждое открытие индикатором — новый объект спеки вкладки: прокрутка снова. jsdom не знает scrollIntoView.
    if (loaded && spec?.kind === "settings" && spec.section === "agent") rootRef.current?.scrollIntoView?.({ block: "start", behavior: "smooth" });
  }, [spec, loaded]);
  return (
    <section ref={rootRef} className="agent-section" aria-labelledby="settings-agent-title" data-help-anchor="background">
      <h4 id="settings-agent-title">{t("settings.mcp.agent.title")}</h4>
      <p className="muted">{t("settings.mcp.agent.intro")}</p>
      <p className="diag-note">{t("settings.mcp.agent.note")}</p>
      {settings.error ? (
        <ErrorBanner error={settings.error} onRetry={() => void settings.refetch()} />
      ) : !settings.data ? (
        <p className="muted">{t("common.loading")}</p>
      ) : (
        <AgentForm status={status} saved={settings.data} />
      )}
    </section>
  );
}

/**
 * Форма подраздела. Выключатель сохраняется сразу (`{ ...сохранённые, enabled }`); остальные поля — черновиком с
 * «Сохранить» (проверка — по пределам Rust, `agentForm.ts`); после сохранения статус MCP перечитывается: кнопка
 * «Передать» смотрит на `agent.enabled` и `agent.configured`. Поля не зависят от выключателя и политики: путь и модель можно
 * готовить заранее, а запрет политикой Rust применяет только к включению. «Найти» подставляет путь и показывает версию.
 * Путь, указанный руками, при уходе из поля проверяется так же (`detect_agent_cli` с `path` — версия этой программы).
 * Список моделей — по сохранённым «Через Ollama» и адресу; что значит пустое поле «Модель» (модель по умолчанию или
 * обязательная модель Ollama), написано текстом под полем. «Проверить» — по сохранённым настройкам: итог приходит событием
 * запуска. «Выгрузить модель» неактивна, пока идёт запуск, и когда Glafira моделью не управляет (`unloadOff`). Запись
 * настроек — одна за раз: пока пишется выключатель, «Сохранить» ждёт, и наоборот (иначе запись с прежним снимком могла бы
 * прийти в Rust второй и затереть первую). Кнопки, занятые своим действием, — `aria-disabled` (фокус остаётся на них),
 * неактивные по состоянию формы — `disabled`.
 */
function AgentForm({ status, saved }: { status: McpStatus; saved: AgentSettings }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [draft, setDraft] = useState<AgentDraft>(() => toDraft(saved));
  const [detected, setDetected] = useState<{ preset: AgentPreset; result: AgentDetected } | null>(null);
  const [checkId, setCheckId] = useState<string | null>(null);
  const [logRun, setLogRun] = useState<string | null>(null);
  const [savedNote, setSavedNote] = useState(false);
  const modelsId = useId();
  const saveRef = useRef<HTMLButtonElement>(null);
  // Путь правили руками: при уходе из поля — версия этой программы (`pathNow` — путь в поле сейчас).
  const pathEdited = useRef(false);
  const pathNow = useRef("");
  const linesRef = useRef<HTMLDivElement>(null);
  const { all: runs, state: runsState, error: runsError, refresh: refreshRuns } = useAgentRuns(null);
  // ОС Glafira — для проверки абсолютного пути, как у Rust; пока не прочитана — годится любой вид пути (кэш общий с «О программе»).
  const appInfo = useQuery({ queryKey: ["app-info"], queryFn: () => api.getAppInfo(), staleTime: Infinity });
  const errors = validateDraft(draft, pathRules(appInfo.data?.os));
  const hasErrors = Object.keys(errors).length > 0;
  const hidden = hiddenErrorPreset(draft, errors);
  const dirty = JSON.stringify(draft) !== JSON.stringify(toDraft(saved));
  const block = toggleBlock(status);
  const qwen = draft.preset === "qwen";
  const path = qwen ? draft.qwenPath : draft.claudePath;
  pathNow.current = path;
  const model = qwen ? draft.qwenModel : draft.claudeModel;
  const pathError = qwen ? errors.qwenPath : errors.claudePath;
  const modelError = qwen ? errors.qwenModel : errors.claudeModel;
  const update = (patch: Partial<AgentDraft>) => {
    setDraft((d) => ({ ...d, ...patch }));
    setSavedNote(false);
  };

  const onSaved = (next: AgentSettings) => {
    qc.setQueryData(AGENT_SETTINGS_KEY, next);
    void qc.invalidateQueries({ queryKey: MCP_STATUS_KEY, exact: true });
  };
  const toggle = useMutation({ mutationFn: (on: boolean) => api.setAgentSettings({ ...saved, enabled: on }), onSuccess: onSaved });
  const save = useMutation({
    mutationFn: () => api.setAgentSettings(fromDraft(draft, saved)),
    onSuccess: (next) => {
      onSaved(next);
      setDraft(toDraft(next));
      setSavedNote(true);
    },
  });
  // «Найти»: поиск программы, найденный путь подставляется в поле.
  const detect = useMutation({
    mutationFn: (preset: AgentPreset) => api.detectAgentCli(preset),
    onSuccess: (result, preset) => {
      setDetected({ preset, result });
      if (result.path) update(preset === "qwen" ? { qwenPath: result.path } : { claudePath: result.path });
    },
  });
  // Путь, указанный руками: версия этой программы. Ответ, пришедший после новой правки или «Найти», уже не о поле — не нужен.
  const version = useMutation({
    mutationFn: ({ preset, path }: { preset: AgentPreset; path: string }) => api.detectAgentCli(preset, path),
    onSuccess: (result, { preset, path }) => {
      if (pathNow.current.trim() === path) setDetected({ preset, result });
    },
  });
  const models = useQuery({
    queryKey: ["agentModels", draft.preset, saved.claude.viaOllama, saved.ollamaUrl],
    queryFn: () => api.listAgentModels(draft.preset),
    retry: false,
  });
  const unload = useMutation({ mutationFn: () => api.unloadAgentModel() });
  const check = useMutation({
    mutationFn: () => api.checkAgent(),
    onSuccess: (run) => {
      setCheckId(run.id);
      refreshRuns();
    },
  });

  // После «Сохранить» кнопка неактивна (изменений нет): фокус с неё переходит на строку итога, а не теряется.
  useEffect(() => {
    const active = document.activeElement;
    if (savedNote && (active === document.body || active === saveRef.current)) linesRef.current?.focus();
  }, [savedNote]);

  const det = detected?.preset === draft.preset ? detected.result : null;
  // Что значит пустое поле «Модель» — видимым текстом под полем (у плейсхолдера контраст ниже AA).
  const modelEmptyNote = models.data?.default
    ? t("settings.mcp.agent.modelDefault", { model: models.data.default })
    : !qwen && draft.viaOllama
      ? t("settings.mcp.agent.modelOllama")
      : t("settings.mcp.agent.modelClientDefault");
  const writing = toggle.isPending || save.isPending;
  const runActive = status.agent.running != null;
  const checkRun = checkId === null ? null : (runs.find((r) => r.id === checkId) ?? (check.data?.id === checkId ? check.data : null));
  const checkBusy = check.isPending || (checkRun !== null && isActive(checkRun));
  const serverReady = status.agent.policyEnabled && status.server === "running";
  const needs = !status.agent.configured ? (needsModel(saved) ? "model" : "path") : null;
  const checkBlocked = !serverReady
    ? ""
    : needs === "model"
      ? t("settings.mcp.agent.checkNeedsModel")
      : needs === "path"
        ? t("settings.mcp.agent.checkNeedsPath")
        : dirty
          ? t("settings.mcp.agent.checkDirty")
          : "";
  const canCheck = serverReady && needs === null && !dirty;
  const toggleNote = block
    ? t(BLOCK_KEYS[block])
    : saved.enabled && needs === "model"
      ? t("settings.mcp.agent.notConfiguredModel")
      : saved.enabled && needs === "path"
        ? t("settings.mcp.agent.notConfigured")
        : "";
  const unloadReason = unloadOff(saved);
  const checkPath = () => {
    if (!pathEdited.current) return;
    pathEdited.current = false;
    const p = path.trim();
    if (p !== "" && !pathError) version.mutate({ preset: draft.preset, path: p });
  };

  return (
    <>
      <div className="form">
        <span className="field-label">
          <label htmlFor="agent-enabled">{t("settings.mcp.agent.enable")}</label>
          <Hint k="hints.settings.mcp.agent.enable" label={t("settings.mcp.agent.enable")} />
        </span>
        <div className="agent-control">
          <span className="agent-check">
            <input
              id="agent-enabled"
              type="checkbox"
              checked={status.agent.policyEnabled && saved.enabled}
              disabled={block !== null}
              aria-disabled={writing || undefined}
              aria-describedby={toggleNote ? "agent-enabled-note" : undefined}
              onChange={(e) => {
                if (!writing) toggle.mutate(e.target.checked);
              }}
            />
          </span>
          {toggleNote && (
            <p id="agent-enabled-note" className="muted agent-note">
              {toggleNote}
            </p>
          )}
        </div>

        <span className="field-label" id="agent-preset-label">
          {t("settings.mcp.agent.preset")}
        </span>
        <span role="radiogroup" aria-labelledby="agent-preset-label" className="agent-presets">
          {PRESETS.map((p) => (
            <label key={p} className="inline">
              <input type="radio" name="agent-preset" value={p} checked={draft.preset === p} onChange={() => update({ preset: p })} />
              {PRESET_NAMES[p]}
            </label>
          ))}
        </span>

        <Field
          id="agent-path"
          label={t("settings.mcp.agent.path")}
          hint="hints.settings.mcp.agent.path"
          error={pathError}
          below={
            <div role="status" className="agent-lines">
              {det?.path && det.version && !det.belowMinimum && <p className="muted">{t("settings.mcp.agent.version", { version: det.version })}</p>}
              {det?.path && det.version && det.belowMinimum && (
                <p className="diag-warn">{t("settings.mcp.agent.belowMinimum", { version: det.version, min: MIN_VERSION[draft.preset] })}</p>
              )}
              {det && !det.path && <p className="agent-error">{t("settings.mcp.agent.notFound")}</p>}
              {/* error бывает только у найденной программы: --version не удался — программа, скорее всего, не запустится. */}
              {det?.path && det.error && <p className="diag-warn">{t("settings.mcp.agent.versionError", { error: det.error })}</p>}
            </div>
          }
        >
          <input
            id="agent-path"
            type="text"
            className="mono"
            spellCheck={false}
            autoComplete="off"
            value={path}
            {...invalid("agent-path", pathError)}
            onChange={(e) => {
              // Итог «Найти» относится к найденному пути: правка руками его снимает, а уход из поля проверит новый путь.
              setDetected(null);
              pathEdited.current = true;
              update(qwen ? { qwenPath: e.target.value } : { claudePath: e.target.value });
            }}
            onBlur={checkPath}
          />
          <button type="button" aria-disabled={detect.isPending || undefined} onClick={() => !detect.isPending && detect.mutate(draft.preset)}>
            {detect.isPending ? t("settings.mcp.agent.finding") : t("settings.mcp.agent.find")}
          </button>
        </Field>

        <Field
          id="agent-model"
          label={t("settings.mcp.agent.model")}
          hint="hints.settings.mcp.agent.model"
          error={modelError}
          below={
            <>
              {model.trim() === "" && (
                <p id="agent-model-note" className="muted agent-note">
                  {modelEmptyNote}
                </p>
              )}
              {models.error && (
                <p id="agent-models-failed" className="muted agent-note">
                  {t("settings.mcp.agent.modelsFailed")}
                </p>
              )}
            </>
          }
        >
          <input
            id="agent-model"
            type="text"
            className="mono"
            spellCheck={false}
            autoComplete="off"
            list={modelsId}
            value={model}
            {...invalid("agent-model", modelError, [...(model.trim() === "" ? ["agent-model-note"] : []), ...(models.error ? ["agent-models-failed"] : [])])}
            onChange={(e) => update(qwen ? { qwenModel: e.target.value } : { claudeModel: e.target.value })}
          />
          <datalist id={modelsId}>
            {(models.data?.ids ?? []).map((m) => (
              <option key={m} value={m} />
            ))}
          </datalist>
        </Field>

        {!qwen && (
          <>
            <span className="field-label">
              <label htmlFor="agent-via-ollama">{t("settings.mcp.agent.viaOllama")}</label>
              <Hint k="hints.settings.mcp.agent.viaOllama" label={t("settings.mcp.agent.viaOllama")} />
            </span>
            <span className="agent-check">
              <input id="agent-via-ollama" type="checkbox" checked={draft.viaOllama} onChange={(e) => update({ viaOllama: e.target.checked })} />
            </span>
            {!draft.viaOllama && (
              <Field id="agent-budget" label={t("settings.mcp.agent.budget")} hint="hints.settings.mcp.agent.budget" error={errors.budget}>
                <input
                  id="agent-budget"
                  type="text"
                  className="agent-num"
                  inputMode="decimal"
                  autoComplete="off"
                  value={draft.budget}
                  placeholder={t("settings.mcp.agent.budgetEmpty")}
                  {...invalid("agent-budget", errors.budget)}
                  onChange={(e) => update({ budget: e.target.value })}
                />
              </Field>
            )}
          </>
        )}

        <Field id="agent-timeout" label={t("settings.mcp.agent.timeout")} hint="hints.settings.mcp.agent.timeout" error={errors.timeout}>
          <input
            id="agent-timeout"
            type="text"
            className="agent-num"
            inputMode="numeric"
            autoComplete="off"
            value={draft.timeout}
            {...invalid("agent-timeout", errors.timeout)}
            onChange={(e) => update({ timeout: e.target.value })}
          />
        </Field>
        <Field id="agent-keep-warm" label={t("settings.mcp.agent.keepWarm")} hint="hints.settings.mcp.agent.keepWarm" error={errors.keepWarm}>
          <input
            id="agent-keep-warm"
            type="text"
            className="agent-num"
            inputMode="numeric"
            autoComplete="off"
            value={draft.keepWarm}
            {...invalid("agent-keep-warm", errors.keepWarm)}
            onChange={(e) => update({ keepWarm: e.target.value })}
          />
        </Field>
        <Field
          id="agent-ollama"
          label={t("settings.mcp.agent.ollamaUrl")}
          hint="hints.settings.mcp.agent.ollamaUrl"
          error={errors.ollamaUrl}
          below={
            <>
              {!errors.ollamaUrl && !isLoopbackUrl(draft.ollamaUrl) && <p className="muted agent-note">{t("settings.mcp.agent.remoteOllama")}</p>}
              {unloadReason !== null && unloadReason !== "remote" && <p className="muted agent-note">{t(UNLOAD_OFF_KEYS[unloadReason])}</p>}
              <div role="status" className="agent-lines">
                {unload.isSuccess && <p className="muted">{t("settings.mcp.agent.unloaded")}</p>}
              </div>
              {unload.error && <ErrorBanner error={unload.error} />}
            </>
          }
        >
          <input
            id="agent-ollama"
            type="text"
            className="mono"
            spellCheck={false}
            autoComplete="off"
            value={draft.ollamaUrl}
            {...invalid("agent-ollama", errors.ollamaUrl)}
            onChange={(e) => update({ ollamaUrl: e.target.value })}
          />
          <button type="button" disabled={runActive || unloadReason !== null} aria-disabled={unload.isPending || undefined} onClick={() => !unload.isPending && unload.mutate()}>
            {t("settings.mcp.agent.unload")}
          </button>
        </Field>
      </div>

      <div className="toolbar agent-actions">
        <button type="button" ref={saveRef} className="primary" disabled={!dirty || hasErrors} aria-disabled={writing || undefined} onClick={() => !writing && save.mutate()}>
          {t("common.save")}
        </button>
        <button type="button" disabled={!canCheck} aria-disabled={checkBusy || undefined} onClick={() => !checkBusy && check.mutate()}>
          {t("settings.mcp.agent.check")}
        </button>
        <Hint k="hints.settings.mcp.agent.check" label={t("settings.mcp.agent.check")} />
      </div>
      {checkBlocked && <p className="muted agent-note">{checkBlocked}</p>}
      {/* tabIndex — сюда переходит фокус после «Сохранить» (кнопка становится неактивной). */}
      <div role="status" className="agent-lines" ref={linesRef} tabIndex={-1}>
        {hidden && <p className="agent-error">{t("settings.mcp.agent.errors.hidden", { agent: PRESET_NAMES[hidden] })}</p>}
        {savedNote && !dirty && <p className="muted">{t("settings.mcp.agent.saved")}</p>}
        {(check.isPending || checkRun !== null) && <p>{checkRun !== null ? checkResultText(checkRun, t) : t("settings.mcp.agent.checking")}</p>}
      </div>
      {checkRun && !isActive(checkRun) && checkRun.status !== "done" && checkRun.startedAtMs != null && (
        <button type="button" className="link" aria-label={t("settings.mcp.agent.checkLog")} onClick={() => setLogRun(checkRun.id)}>
          {t("llmAssist.agent.showLog")}
        </button>
      )}
      {toggle.error && <ErrorBanner error={toggle.error} />}
      {save.error && <ErrorBanner error={save.error} />}
      {detect.error && <ErrorBanner error={detect.error} />}
      {version.error && <ErrorBanner error={version.error} />}
      {check.error && <ErrorBanner error={check.error} />}
      <AgentRuns runs={runs} state={runsState} error={runsError} onRetry={refreshRuns} onShowLog={setLogRun} />
      {logRun && <AgentLogDialog runId={logRun} onClose={() => setLogRun(null)} />}
    </>
  );
}
