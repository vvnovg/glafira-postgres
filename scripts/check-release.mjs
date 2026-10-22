#!/usr/bin/env node
// Проверки перед релизом: версии, ключ updater, раздел CHANGELOG.
// --dry-run (workflow_dispatch): проблема с ключом — предупреждение, а не ошибка.
// Сборка по тегу всегда строгая.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Все проблемы релиза: { kind: "version" | "key" | "changelog", message }. */
export function releaseIssues({ tag, files }) {
  const issues = [];
  const add = (kind, message) => issues.push({ kind, message });
  const version = tag.replace(/^v/, "");
  const pkg = JSON.parse(files.packageJson).version;
  const cargo = /^version\s*=\s*"([^"]+)"/m.exec(files.cargoToml)?.[1];
  const tauri = JSON.parse(files.tauriConf);
  for (const [name, v] of [["package.json", pkg], ["src-tauri/Cargo.toml", cargo], ["src-tauri/tauri.conf.json", tauri.version]]) {
    if (v !== version) add("version", `${name}: версия ${v ?? "не найдена"}, а тег ${tag}`);
  }
  const pubkey = tauri.plugins?.updater?.pubkey;
  if (!pubkey || pubkey === "REPLACE_WITH_UPDATER_PUBKEY") {
    add("key", "tauri.conf.json: не задан открытый ключ updater (plugins.updater.pubkey)");
  }
  if (!new RegExp(`^## ${escapeRe(version)}\\s*$`, "m").test(files.changelog)) {
    add("changelog", `CHANGELOG.md: нет раздела «## ${version}»`);
  }
  return issues;
}

/**
 * Ошибки, блокирующие релиз (string[]). По умолчанию строго: ключ — ошибка.
 * С dryRun ключ уходит в warnings (если передан массив).
 */
export function checkRelease({ tag, files, dryRun = false, warnings = [] }) {
  const errors = [];
  for (const { kind, message } of releaseIssues({ tag, files })) {
    if (dryRun && kind === "key") warnings.push(message);
    else errors.push(message);
  }
  return errors;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const dryRun = args.includes("--dry-run");
  const tag = args.find((a) => !a.startsWith("--"));
  if (!tag) {
    console.error("использование: check-release.mjs [--dry-run] <tag>");
    process.exit(2);
  }
  const read = (p) => readFileSync(p, "utf8");
  const warnings = [];
  const problems = checkRelease({
    tag,
    dryRun,
    warnings,
    files: {
      packageJson: read("package.json"),
      cargoToml: read("src-tauri/Cargo.toml"),
      tauriConf: read("src-tauri/tauri.conf.json"),
      changelog: read("CHANGELOG.md"),
    },
  });
  for (const w of warnings) console.log(`::warning::пробный прогон: ${w}`);
  if (problems.length) {
    console.error(problems.map((p) => `✗ ${p}`).join("\n"));
    process.exit(1);
  }
  console.log(dryRun ? `✓ ${tag}: пробный прогон разрешён` : `✓ ${tag}: можно выпускать`);
}
