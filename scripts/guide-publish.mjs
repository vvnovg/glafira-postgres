#!/usr/bin/env node
// Сборка руководства для GitHub: метки [[ui:ключ]] → **значение**, якоря {#id} → <a id>, README с языками и главами.
// Только модули node: — запускается в CI без установки зависимостей.
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { GUIDE_CHAPTERS, flatten } from "./i18n-check.mjs";

/** Метка и точка сразу после неё: точку убираем, если значение уже кончается на «.» или «:» (как в src/guide/parse.ts). */
const LABEL = /\[\[ui:([A-Za-z0-9_.-]+)\]\](\.)?/g;
const HEADING = /^(#{1,6})\s+(.*?)\s*$/;
const ANCHOR = /^(#{2,6}\s+.*?)\s*\{#([a-z0-9-]+)\}\s*$/;
const FENCE = /^\s*(```|~~~)/;
/** Те же символы, что экранирует src/guide/parse.ts. */
const SPECIAL = /[\\`*_[\]<>#|~!]/g;

const escapeMarkdown = (value) => value.replace(SPECIAL, (c) => `\\${c}`);

/** Глава → Markdown для GitHub. labels: ключ → значение языка. Неизвестная метка — ошибка. */
export function publishChapter(text, labels) {
  const raw = (key) => {
    if (!labels.has(key)) throw new Error(`метка [[ui:${key}]] — нет ключа ${key} в словаре`);
    return labels.get(key);
  };
  const tail = (key, dot) => (dot && !/[.:]$/.test(raw(key)) ? dot : "");
  const plainLabels = (s) => s.replace(LABEL, (_m, k, dot) => escapeMarkdown(raw(k)) + tail(k, dot));
  const boldLabels = (s) => s.replace(LABEL, (_m, k, dot) => `**${escapeMarkdown(raw(k))}**` + tail(k, dot));
  const out = [];
  let fence = false;
  for (const line of text.replace(/\r\n?/g, "\n").split("\n")) {
    if (FENCE.test(line)) {
      fence = !fence;
      out.push(line);
      continue;
    }
    if (fence) {
      out.push(line);
      continue;
    }
    const anchor = ANCHOR.exec(line);
    if (anchor) {
      out.push(`<a id="${anchor[2]}"></a>`, "", plainLabels(anchor[1]));
      continue;
    }
    out.push(HEADING.test(line) ? plainLabels(line) : boldLabels(line));
  }
  return out.join("\n");
}

/** Языки из src/i18n/languages.ts: [{ code, nativeName }] в порядке списка. */
export function languages(languagesTs) {
  return [...languagesTs.matchAll(/\bcode:\s*"([\w-]+)"\s*,\s*nativeName:\s*"([^"]+)"/g)].map((m) => ({ code: m[1], nativeName: m[2] }));
}

const chapterTitle = (text, fallback) => /^#\s+(.+?)\s*$/m.exec(text)?.[1] ?? fallback;

/** Пишет outDir/<код>/<глава>.md для каждого языка и outDir/README.md (языки и оглавления). */
export function publishGuide(root, outDir, chapters = GUIDE_CHAPTERS) {
  const langs = languages(readFileSync(join(root, "src/i18n/languages.ts"), "utf8"));
  if (!langs.length) throw new Error("в src/i18n/languages.ts не найдено ни одного языка");
  const toc = [];
  for (const { code, nativeName } of langs) {
    const labels = flatten(JSON.parse(readFileSync(join(root, "src/locales", `${code}.json`), "utf8")));
    const dir = join(outDir, code);
    rmSync(dir, { recursive: true, force: true });
    mkdirSync(dir, { recursive: true });
    const items = [];
    for (const ch of chapters) {
      const src = readFileSync(join(root, "docs/guide", code, `${ch}.md`), "utf8");
      let md;
      try {
        md = publishChapter(src, labels);
      } catch (e) {
        throw new Error(`${code}/${ch}.md: ${e.message}`);
      }
      writeFileSync(join(dir, `${ch}.md`), md);
      items.push({ ch, title: chapterTitle(md, ch) });
    }
    toc.push({ code, nativeName, items });
  }
  const readme = [
    "# Glafira for PostgreSQL — User guide",
    "",
    toc.map(({ code, nativeName }) => `[${nativeName}](${code}/getting-started.md)`).join(" · "),
    "",
  ];
  for (const { code, nativeName, items } of toc) {
    readme.push(`## ${nativeName}`, "", ...items.map(({ ch, title }) => `- [${title}](${code}/${ch}.md)`), "");
  }
  writeFileSync(join(outDir, "README.md"), readme.join("\n"));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const out = process.argv[2];
  if (!out) {
    console.error("использование: node scripts/guide-publish.mjs <каталог>");
    process.exit(2);
  }
  publishGuide(process.cwd(), resolve(out));
  console.log(`✓ руководство собрано в ${out}`);
}
