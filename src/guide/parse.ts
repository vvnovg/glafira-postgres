import type { ChapterId } from "./chapters";

/**
 * Раздел главы. heading false — вступление до первого `##` (его title — заголовок главы);
 * id null — вступление или `##` без `{#id}`.
 */
export interface GuideSection {
  id: string | null;
  title: string;
  markdown: string;
  heading: boolean;
}

export interface GuideChapter {
  id: ChapterId;
  title: string;
  sections: GuideSection[];
}

export interface GuideHit {
  chapter: ChapterId;
  section: string | null;
  title: string;
  snippet: string;
}

/** Метка и точка сразу после неё: точку убираем, если значение уже кончается на «.» или «:». */
const LABEL = /\[\[ui:([A-Za-z0-9_.-]+)\]\](\.)?/g;
const SECTION = /^##\s+(.*?)\s*(?:\{#([a-z0-9-]+)\})?\s*$/;
const FENCE = /^\s*(```|~~~)/;

/** Символы Markdown, которые в значении метки должны остаться текстом. */
const SPECIAL = /[\\`*_[\]<>#|~!]/g;
const ESCAPED = /\\([\\`*_[\]<>#|~!])/g;

const escapeMarkdown = (value: string) => value.replace(SPECIAL, (c) => `\\${c}`);

function withLabels(text: string, label: (key: string) => string, bold: boolean): string {
  return text.replace(LABEL, (_m, key: string, dot: string | undefined) => {
    const value = label(key);
    const tail = dot && !/[.:]$/.test(value) ? dot : "";
    return (bold ? `**${escapeMarkdown(value)}**` : value) + tail;
  });
}

/** Разбирает главу: `# Заголовок`, разделы `## Заголовок {#id}`, метки `[[ui:ключ]]` → `**значение**`. */
export function parseChapter(id: ChapterId, text: string, label: (key: string) => string): GuideChapter {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  let title: string = id;
  const sections: GuideSection[] = [];
  let current: { id: string | null; title: string | null; body: string[] } = { id: null, title: null, body: [] };
  // title null — вступление до первого `##`.
  let inFence = false;
  let titleSeen = false;

  const flush = () => {
    const markdown = current.body.join("\n").trim();
    if (current.title !== null || markdown)
      sections.push({ id: current.id, title: current.title ?? title, markdown, heading: current.title !== null });
  };

  for (const line of lines) {
    if (FENCE.test(line)) inFence = !inFence;
    if (!inFence && !titleSeen && /^#\s+/.test(line)) {
      title = withLabels(line.replace(/^#\s+/, "").trim(), label, false);
      titleSeen = true;
      continue;
    }
    const m = inFence ? null : SECTION.exec(line);
    if (m) {
      flush();
      current = { id: m[2] ?? null, title: withLabels(m[1], label, false), body: [] };
      continue;
    }
    current.body.push(inFence ? line : withLabels(line, label, true));
  }
  flush();
  return { id, title, sections };
}

/** Экранированный символ `\\X` на время разбора заменяется символом из области частного использования. */
const MARKS = "\\`*_[]<>#|~!";
const PLACEHOLDER = /[\uE000-\uE00B]/g;
const hide = (_m: string, c: string) => String.fromCharCode(0xe000 + MARKS.indexOf(c));
const restore = (c: string) => MARKS[c.charCodeAt(0) - 0xe000];

/** Текст раздела без разметки Markdown — для поиска и фрагментов. Без lookbehind: старый WebKit его не разбирает. */
function plain(markdown: string): string {
  return markdown
    .replace(ESCAPED, hide)
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^\s*(```|~~~).*$/gm, "")
    .replace(/^\s{0,3}(#{1,6}\s+|>\s?|[-*+]\s+|\d+\.\s+)/gm, "")
    .replace(/(\*\*|__|`|\*|_)/g, "")
    .replace(PLACEHOLDER, restore)
    .replace(/\s+/g, " ")
    .trim();
}

const CONTEXT = 40;

function snippetAt(text: string, index: number, length: number): string {
  const start = Math.max(0, index - CONTEXT);
  const end = Math.min(text.length, index + length + CONTEXT);
  return `${start > 0 ? "…" : ""}${text.slice(start, end).trim()}${end < text.length ? "…" : ""}`;
}

/** Поиск без учёта регистра по заголовкам и тексту разделов (после подстановки меток). */
export function searchGuide(chapters: GuideChapter[], query: string): GuideHit[] {
  const q = query.trim().toLocaleLowerCase();
  if (!q) return [];
  const hits: GuideHit[] = [];
  for (const chapter of chapters) {
    for (const section of chapter.sections) {
      const body = plain(section.markdown);
      const inBody = body.toLocaleLowerCase().indexOf(q);
      const inTitle = section.title.toLocaleLowerCase().includes(q);
      if (inBody < 0 && !inTitle) continue;
      hits.push({
        chapter: chapter.id,
        section: section.id,
        title: section.title,
        snippet: inBody >= 0 ? snippetAt(body, inBody, q.length) : snippetAt(body, 0, 0),
      });
    }
  }
  return hits;
}
