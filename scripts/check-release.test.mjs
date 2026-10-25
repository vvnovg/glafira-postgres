import { test } from "node:test";
import assert from "node:assert/strict";
import { checkRelease } from "./check-release.mjs";

const files = (o = {}) => ({
  packageJson: JSON.stringify({ version: "0.2.0" }),
  cargoToml: '[package]\nname = "x"\nversion = "0.2.0"\n',
  tauriConf: JSON.stringify({ version: "0.2.0", plugins: { updater: { pubkey: "REALKEY" } } }),
  changelogs: { en: "# Changelog\n\n## 0.2.0\n- first\n", ru: "# Список изменений\n\n## 0.2.0\n- первое\n" },
  ...o,
});

test("всё в порядке", () => assert.deepEqual(checkRelease({ tag: "v0.2.0", files: files() }), []));
test("разные версии", () => {
  const p = checkRelease({ tag: "v0.2.1", files: files() });
  assert.equal(p.length, 5); // три файла + CHANGELOG.md + docs/changelog/ru.md
});
test("заглушка updater", () => {
  const p = checkRelease({
    tag: "v0.2.0",
    files: files({ tauriConf: JSON.stringify({ version: "0.2.0", plugins: { updater: { pubkey: "REPLACE_WITH_UPDATER_PUBKEY" } } }) }),
  });
  assert.equal(p.length, 1);
  assert.ok(p[0].includes("updater"));
});
test("предрелиз", () => {
  const f = files({
    packageJson: JSON.stringify({ version: "0.2.0-beta.1" }),
    cargoToml: 'version = "0.2.0-beta.1"\n',
    tauriConf: JSON.stringify({ version: "0.2.0-beta.1", plugins: { updater: { pubkey: "K" } } }),
    changelogs: { en: "## 0.2.0-beta.1\n- beta\n", ru: "## 0.2.0-beta.1\n- бета\n" },
  });
  assert.deepEqual(checkRelease({ tag: "v0.2.0-beta.1", files: f }), []);
});
test("раздел CHANGELOG ищется точно: 0.2.0 не засчитывается разделом 0.2.0-beta.1", () => {
  const p = checkRelease({ tag: "v0.2.0", files: files({ changelogs: { en: "## 0.2.0-beta.1\n- beta\n", ru: "## 0.2.0\n- первое\n" } }) });
  assert.deepEqual(p, ["CHANGELOG.md: нет раздела «## 0.2.0»"]);
});

const withPlaceholderKey = () =>
  files({ tauriConf: JSON.stringify({ version: "0.2.0", plugins: { updater: { pubkey: "REPLACE_WITH_UPDATER_PUBKEY" } } }) });

test("по умолчанию (тег) строго: ключ — ошибка, даже если передан warnings", () => {
  const warnings = [];
  const p = checkRelease({ tag: "v0.2.0", files: withPlaceholderKey(), warnings });
  assert.equal(p.length, 1);
  assert.deepEqual(warnings, []);
});

test("пробный прогон: ключ — предупреждение, версии и CHANGELOG.md — по-прежнему ошибки", () => {
  const warnings = [];
  assert.deepEqual(checkRelease({ tag: "v0.2.0", files: withPlaceholderKey(), dryRun: true, warnings }), []);
  assert.equal(warnings.length, 1);
  const p = checkRelease({ tag: "v0.3.0", files: withPlaceholderKey(), dryRun: true, warnings: [] });
  assert.equal(p.length, 4);
});

test("переводы заметок: нет файла, нет раздела, пустой раздел — ошибки по тегу", () => {
  const changelogs = { en: "## 0.2.0\n- first\n", ru: null, de: "## 0.1.0\n- alt\n", fr: "## 0.2.0\n\n## 0.1.0\n- a\n" };
  assert.deepEqual(checkRelease({ tag: "v0.2.0", files: files({ changelogs }) }), [
    "docs/changelog/ru.md: нет файла",
    "docs/changelog/de.md: нет раздела «## 0.2.0»",
    "docs/changelog/fr.md: пустой раздел «## 0.2.0»",
  ]);
});

test("пробный прогон: непереведённый раздел — предупреждение, пустой английский — ошибка", () => {
  const warnings = [];
  const ok = checkRelease({ tag: "v0.2.0", files: files({ changelogs: { en: "## 0.2.0\n- first\n", ru: "## 0.1.0\n" } }), dryRun: true, warnings });
  assert.deepEqual(ok, []);
  assert.deepEqual(warnings, ["docs/changelog/ru.md: нет раздела «## 0.2.0»"]);
  const p = checkRelease({ tag: "v0.2.0", files: files({ changelogs: { en: "## 0.2.0\n" } }), dryRun: true });
  assert.deepEqual(p, ["CHANGELOG.md: пустой раздел «## 0.2.0»"]);
});
