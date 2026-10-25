import { test } from "node:test";
import assert from "node:assert/strict";
import { changelogPath, changelogSection, changelogVersions, releaseNotes } from "./changelog.mjs";

const LOG = "# Changelog\n\n## 0.3.0\n- three\n  - nested\n\n### Notes\n- sub\n\n## 0.2.0-beta.1\n- beta\n\n## 0.2.0\n- two\n";

test("пути: en — CHANGELOG.md, остальные — docs/changelog/<код>.md", () => {
  assert.equal(changelogPath("en"), "CHANGELOG.md");
  assert.equal(changelogPath("pt-BR"), "docs/changelog/pt-BR.md");
});

test("раздел — до следующего «## », подзаголовки ### остаются внутри", () => {
  assert.equal(changelogSection(LOG, "0.3.0"), "- three\n  - nested\n\n### Notes\n- sub");
  assert.equal(changelogSection(LOG, "0.2.0"), "- two");
});

test("версия ищется точно: 0.2.0 не находит 0.2.0-beta.1, нет раздела — null", () => {
  assert.equal(changelogSection(LOG, "0.2.0-beta.1"), "- beta");
  assert.equal(changelogSection("## 0.2.0-beta.1\n- beta\n", "0.2.0"), null);
  assert.equal(changelogSection(LOG, "0.4.0"), null);
});

test("CRLF и пробелы после заголовка", () => {
  assert.equal(changelogSection("## 0.2.0  \r\n- two\r\n## 0.1.0\r\n- one\r\n", "0.2.0"), "- two");
  assert.deepEqual(changelogVersions("## 0.2.0  \r\n- two\r\n## 0.1.0\r\n"), ["0.2.0", "0.1.0"]);
});

test("версии разделов по порядку", () => {
  assert.deepEqual(changelogVersions(LOG), ["0.3.0", "0.2.0-beta.1", "0.2.0"]);
});

test("заметки: только языки с непустым разделом", () => {
  const notes = releaseNotes({ en: "## 1.0.0\n- one\n", ru: "## 1.0.0\n- один\n", de: "## 0.9.0\n- alt\n", fr: "## 1.0.0\n\n", ja: null }, "1.0.0");
  assert.deepEqual(notes, { en: "- one", ru: "- один" });
});
