import { test } from "node:test";
import assert from "node:assert/strict";
import { checkRelease } from "./check-release.mjs";

const files = (o = {}) => ({
  packageJson: JSON.stringify({ version: "0.2.0" }),
  cargoToml: '[package]\nname = "x"\nversion = "0.2.0"\n',
  tauriConf: JSON.stringify({ version: "0.2.0", plugins: { updater: { pubkey: "REALKEY" } } }),
  changelog: "# Changelog\n\n## 0.2.0\n- первое\n",
  ...o,
});

test("всё в порядке", () => assert.deepEqual(checkRelease({ tag: "v0.2.0", files: files() }), []));
test("разные версии", () => {
  const p = checkRelease({ tag: "v0.2.1", files: files() });
  assert.equal(p.length, 4); // три файла + CHANGELOG
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
    changelog: "## 0.2.0-beta.1\n",
  });
  assert.deepEqual(checkRelease({ tag: "v0.2.0-beta.1", files: f }), []);
});
test("раздел CHANGELOG ищется точно: 0.2.0 не засчитывается разделом 0.2.0-beta.1", () => {
  const p = checkRelease({ tag: "v0.2.0", files: files({ changelog: "## 0.2.0-beta.1\n" }) });
  assert.deepEqual(p.length, 1);
  assert.ok(p[0].includes("CHANGELOG"));
});

const withPlaceholderKey = () =>
  files({ tauriConf: JSON.stringify({ version: "0.2.0", plugins: { updater: { pubkey: "REPLACE_WITH_UPDATER_PUBKEY" } } }) });

test("по умолчанию (тег) строго: ключ — ошибка, даже если передан warnings", () => {
  const warnings = [];
  const p = checkRelease({ tag: "v0.2.0", files: withPlaceholderKey(), warnings });
  assert.equal(p.length, 1);
  assert.deepEqual(warnings, []);
});

test("пробный прогон: ключ — предупреждение, версии и CHANGELOG — по-прежнему ошибки", () => {
  const warnings = [];
  assert.deepEqual(checkRelease({ tag: "v0.2.0", files: withPlaceholderKey(), dryRun: true, warnings }), []);
  assert.equal(warnings.length, 1);
  const p = checkRelease({ tag: "v0.3.0", files: withPlaceholderKey(), dryRun: true, warnings: [] });
  assert.equal(p.length, 4);
});
