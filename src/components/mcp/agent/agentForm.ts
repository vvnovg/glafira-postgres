import type { AgentPreset, AgentSettings } from "../../../api/types";

/**
 * Пределы формы — те же, что у `AgentSettings::validated` в Rust (`src-tauri/src/agent/settings.rs`, спец. P4c §8.3):
 * неверное значение форма не отправляет, а Rust всё равно проверяет сам.
 */
export const TIMEOUT_RANGE = { min: 2, max: 60 } as const;
export const KEEP_WARM_RANGE = { min: 0, max: 120 } as const;
export const BUDGET_RANGE = { min: 0.01, max: 100 } as const;
/** Имя модели (спец. P4c §4.1, `presets::valid_model`): одна строка без пробелов, до 128 символов. */
export const MODEL_RE = /^[A-Za-z0-9._:/@+-]{1,128}$/;
/** Путь программы — до 1024 символов, адрес Ollama — до 256 байт (`settings.rs`: `MAX_PATH_CHARS`, `MAX_URL_CHARS`). */
const MAX_PATH_CHARS = 1024;
const MAX_URL_BYTES = 256;
/** Минимальные версии пресетов (`presets::min_version`) — для текста предупреждения «ниже минимальной». */
export const MIN_VERSION: Record<AgentPreset, string> = { qwen: "0.26", claude: "2.1" };

/** Черновик формы: числа — строками, как их ввёл пользователь; выключатель в черновик не входит (сохраняется сразу). */
export interface AgentDraft {
  preset: AgentPreset;
  qwenPath: string;
  qwenModel: string;
  claudePath: string;
  claudeModel: string;
  viaOllama: boolean;
  budget: string;
  timeout: string;
  keepWarm: string;
  ollamaUrl: string;
}
export type DraftField = "qwenPath" | "qwenModel" | "claudePath" | "claudeModel" | "budget" | "timeout" | "keepWarm" | "ollamaUrl";
/** Ошибка поля: ключ словаря и параметры (числа форматирует форма). */
export interface FieldError {
  key: string;
  params?: Record<string, string | number>;
}
export type DraftErrors = Partial<Record<DraftField, FieldError>>;
/** Поля пресета: при другом выбранном агенте они скрыты, и форма называет агента с ошибкой. */
export const PRESET_FIELDS: Record<AgentPreset, DraftField[]> = { qwen: ["qwenPath", "qwenModel"], claude: ["claudePath", "claudeModel", "budget"] };

/** Число с точкой или запятой (в ru десятичный разделитель — запятая); не число (знак, экспонента, пусто) — NaN. */
export function parseDecimal(s: string): number {
  const v = s.trim().replace(",", ".");
  return /^\d+(\.\d+)?$/.test(v) ? Number(v) : NaN;
}

/** Путь абсолютный: Unix — с «/», Windows — с буквы диска или «\\» (спец. P4c §4.2); какой из них годится, решает Rust. */
export function isAbsolutePath(p: string): boolean {
  return p.startsWith("/") || /^[A-Za-z]:[\\/]/.test(p) || p.startsWith("\\\\");
}

/** Управляющие символы (Unicode Cc), как `char::is_control` в Rust. */
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/;

/** Адрес Ollama: http(s) с хостом, не длиннее 256 байт; иначе null. */
export function parseOllamaUrl(s: string): URL | null {
  const v = s.trim();
  if (new TextEncoder().encode(v).length > MAX_URL_BYTES) return null;
  try {
    const u = new URL(v);
    return (u.protocol === "http:" || u.protocol === "https:") && u.hostname !== "" ? u : null;
  } catch {
    return null;
  }
}

/** Локальный адрес (спец. P4c §6.1, `ollama::is_loopback`): моделью Glafira управляет только у Ollama на этом компьютере. */
export function isLoopbackUrl(s: string): boolean {
  const u = parseOllamaUrl(s);
  return u !== null && ["localhost", "127.0.0.1", "[::1]"].includes(u.hostname);
}

export function toDraft(s: AgentSettings): AgentDraft {
  return {
    preset: s.preset,
    qwenPath: s.qwen.path,
    qwenModel: s.qwen.model,
    claudePath: s.claude.path,
    claudeModel: s.claude.model,
    viaOllama: s.claude.viaOllama,
    budget: s.claude.budgetUsd === null ? "" : String(s.claude.budgetUsd),
    timeout: String(s.timeoutMin),
    keepWarm: String(s.keepWarmMin),
    ollamaUrl: s.ollamaUrl,
  };
}

const inBudget = (v: number) => v >= BUDGET_RANGE.min && v <= BUDGET_RANGE.max;
const intIn = (s: string, r: { min: number; max: number }) => /^\d+$/.test(s.trim()) && Number(s) >= r.min && Number(s) <= r.max;

/**
 * Настройки из проверенного черновика; `enabled` — из сохранённых. Через Ollama поле предела скрыто и не проверяется:
 * неверный ввод в нём не затирает сохранённый предел.
 */
export function fromDraft(d: AgentDraft, saved: AgentSettings): AgentSettings {
  const b = d.budget.trim();
  const budget = parseDecimal(b);
  return {
    enabled: saved.enabled,
    preset: d.preset,
    qwen: { path: d.qwenPath.trim(), model: d.qwenModel.trim() },
    claude: {
      path: d.claudePath.trim(),
      model: d.claudeModel.trim(),
      viaOllama: d.viaOllama,
      budgetUsd: b === "" ? null : inBudget(budget) ? budget : saved.claude.budgetUsd,
    },
    timeoutMin: Number(d.timeout.trim()),
    keepWarmMin: Number(d.keepWarm.trim()),
    ollamaUrl: d.ollamaUrl.trim(),
  };
}

/** Проверка черновика по правилам Rust; поля обоих агентов (скрытые тоже — `hiddenErrorPreset`). */
export function validateDraft(d: AgentDraft): DraftErrors {
  const e: DraftErrors = {};
  const path = (field: "qwenPath" | "claudePath") => {
    const v = d[field].trim();
    if (v !== "" && (!isAbsolutePath(v) || CONTROL.test(v) || [...v].length > MAX_PATH_CHARS)) e[field] = { key: "settings.mcp.agent.errors.path" };
  };
  const model = (field: "qwenModel" | "claudeModel") => {
    const v = d[field].trim();
    if (v !== "" && !MODEL_RE.test(v)) e[field] = { key: "settings.mcp.agent.errors.model" };
  };
  path("qwenPath");
  path("claudePath");
  model("qwenModel");
  model("claudeModel");
  if (!d.viaOllama && d.budget.trim() !== "" && !inBudget(parseDecimal(d.budget))) e.budget = { key: "settings.mcp.agent.errors.budget", params: { ...BUDGET_RANGE } };
  if (!intIn(d.timeout, TIMEOUT_RANGE)) e.timeout = { key: "settings.mcp.agent.errors.range", params: { ...TIMEOUT_RANGE } };
  if (!intIn(d.keepWarm, KEEP_WARM_RANGE)) e.keepWarm = { key: "settings.mcp.agent.errors.range", params: { ...KEEP_WARM_RANGE } };
  if (parseOllamaUrl(d.ollamaUrl) === null) e.ollamaUrl = { key: "settings.mcp.agent.errors.url" };
  return e;
}

/** Агент, в скрытых полях которого есть ошибка (выбран другой агент), или null. */
export function hiddenErrorPreset(d: AgentDraft, e: DraftErrors): AgentPreset | null {
  const other: AgentPreset = d.preset === "qwen" ? "claude" : "qwen";
  return PRESET_FIELDS[other].some((f) => e[f]) ? other : null;
}

/**
 * Сохранённым настройкам для запуска не хватает модели: Claude Code через Ollama с путём, но без модели — у Ollama нет
 * «модели по умолчанию» (`AgentSettings::configured`). Подраздел называет это вместо «укажите программу».
 */
export function needsModel(s: AgentSettings): boolean {
  return s.preset === "claude" && s.claude.viaOllama && s.claude.path.trim() !== "" && s.claude.model.trim() === "";
}
