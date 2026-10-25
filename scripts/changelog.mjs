// Список изменений на языках интерфейса: CHANGELOG.md — английский (основной), docs/changelog/<код>.md — остальные языки.
// Раздел версии — от строки «## X.Y.Z» до следующей «## »: он идёт в тело GitHub-релиза и в заметки latest.json.
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

/** Путь списка изменений языка от корня репозитория. */
export function changelogPath(code) {
  return code === "en" ? "CHANGELOG.md" : `docs/changelog/${code}.md`;
}

/** Версии разделов «## X.Y.Z» в порядке файла. */
export function changelogVersions(text) {
  return [...text.matchAll(/^## (\S+)[ \t]*\r?$/gm)].map((m) => m[1]);
}

/** Текст раздела «## version» без заголовка, обрезанный по краям; null — раздела нет. */
export function changelogSection(text, version) {
  const lines = text.split(/\r?\n/);
  const start = lines.findIndex((l) => l.trimEnd() === `## ${version}`);
  if (start < 0) return null;
  const next = lines.findIndex((l, i) => i > start && l.startsWith("## "));
  return lines.slice(start + 1, next < 0 ? undefined : next).join("\n").trim();
}

/** Списки изменений языков из корня репозитория: { код: текст }, файла нет — null. */
export function readChangelogs(root, codes) {
  return Object.fromEntries(
    codes.map((code) => {
      const path = join(root, changelogPath(code));
      return [code, existsSync(path) ? readFileSync(path, "utf8") : null];
    }),
  );
}

/** Заметки к выпуску: { код: раздел } для языков, где раздел версии есть и не пуст. */
export function releaseNotes(changelogs, version) {
  const notes = {};
  for (const [code, text] of Object.entries(changelogs)) {
    const section = text === null ? null : changelogSection(text, version);
    if (section) notes[code] = section;
  }
  return notes;
}
