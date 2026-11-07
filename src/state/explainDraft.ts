/**
 * Черновик консоли EXPLAIN подключения (смоук P4c, R18): текст редактора, выбранные база и узел переживают перезапуск.
 * Хранится в localStorage на подключение; пароли, значения параметров и результаты планов сюда не попадают. Запись — с
 * задержкой (`DRAFT_SAVE_DELAY_MS`): черновик пишется, когда перестали печатать; закрытие консоли и окна дописывает
 * ожидающую запись сразу. Испорченное значение или другая версия схемы читаются как «черновика нет».
 */
export interface ExplainDraft {
  sql: string;
  /** null — «БД по умолчанию» подключения. */
  database: string | null;
  /** null — «за лидером». */
  member: string | null;
}

const PREFIX = "glafira.explain.draft:";
const VERSION = 1;
/** Задержка записи черновика, мс. */
export const DRAFT_SAVE_DELAY_MS = 500;

const key = (connectionId: string) => `${PREFIX}${connectionId}`;

/** Ожидающие записи по подключению: последняя правка и её таймер. */
const pending = new Map<string, { draft: ExplainDraft; timer: ReturnType<typeof setTimeout> }>();
/**
 * Удалённые подключения (их id не переиспользуются): консоль удалённого подключения закрывается уже после стирания, и её
 * последняя запись черновик не возвращает.
 */
const forgotten = new Set<string>();

export function readExplainDraft(connectionId: string): ExplainDraft | null {
  try {
    const raw = localStorage.getItem(key(connectionId));
    if (raw === null) return null;
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return null;
    const d = parsed as Record<string, unknown>;
    const nullableStr = (v: unknown): v is string | null => v === null || typeof v === "string";
    if (d.v !== VERSION || typeof d.sql !== "string" || !nullableStr(d.database) || !nullableStr(d.member)) return null;
    return { sql: d.sql, database: d.database, member: d.member };
  } catch {
    return null;
  }
}

function write(connectionId: string, draft: ExplainDraft) {
  try {
    // Пустой черновик (ни текста, ни выбора) не хранится: консоль и так откроется пустой.
    if (draft.sql === "" && draft.database === null && draft.member === null) localStorage.removeItem(key(connectionId));
    else localStorage.setItem(key(connectionId), JSON.stringify({ v: VERSION, sql: draft.sql, database: draft.database, member: draft.member }));
  } catch {
    // Хранилище недоступно — черновик живёт, пока открыта консоль.
  }
}

let unloadHooked = false;
/** Закрытие окна дописывает ожидающие черновики (WebView может не дать времени таймеру). */
function hookUnload() {
  if (unloadHooked || typeof window === "undefined") return;
  unloadHooked = true;
  const flushAll = () => {
    for (const id of [...pending.keys()]) flushExplainDraft(id);
  };
  window.addEventListener("pagehide", flushAll);
  window.addEventListener("beforeunload", flushAll);
}

/** Запомнить черновик: запись через `DRAFT_SAVE_DELAY_MS`, новая правка откладывает её. */
export function saveExplainDraft(connectionId: string, draft: ExplainDraft): void {
  if (forgotten.has(connectionId)) return;
  const prev = pending.get(connectionId);
  if (prev) clearTimeout(prev.timer);
  const timer = setTimeout(() => {
    pending.delete(connectionId);
    write(connectionId, draft);
  }, DRAFT_SAVE_DELAY_MS);
  pending.set(connectionId, { draft, timer });
  hookUnload();
}

/** Дописать ожидающую запись сразу (консоль закрывается); без неё — ничего. */
export function flushExplainDraft(connectionId: string): void {
  const p = pending.get(connectionId);
  if (!p) return;
  clearTimeout(p.timer);
  pending.delete(connectionId);
  if (!forgotten.has(connectionId)) write(connectionId, p.draft);
}

/** Подключение удалено: черновик стирается, ожидающая и последующие записи отменяются. */
export function forgetExplainDraft(connectionId: string): void {
  forgotten.add(connectionId);
  const p = pending.get(connectionId);
  if (p) clearTimeout(p.timer);
  pending.delete(connectionId);
  try {
    localStorage.removeItem(key(connectionId));
  } catch {
    // Хранилище недоступно — стирать нечего.
  }
}
