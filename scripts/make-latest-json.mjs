#!/usr/bin/env node
// latest.json для tauri-plugin-updater (артефакты v2) из собранных файлов и их .sig.
// macOS-архив Tauri называется «<productName>.app.tar.gz» без архитектуры —
// release.yml переименовывает его в «…_<версия>_<aarch64|x64>.app.tar.gz» до загрузки.
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

// tauri-plugin-updater 2.x ищет в latest.json сначала «{os}-{arch}-{установщик}» (тип установки
// зашит в бинарь бандлером: deb, rpm, appimage, msi, app), затем «{os}-{arch}». Установка из .deb/.rpm
// ставит обновление через dpkg/rpm и отвергает другой формат, поэтому для каждого Linux-пакета —
// своя запись; общий «linux-x86_64» оставлен за AppImage (сборки без типа установки).
const PLATFORMS_BY_SUFFIX = [
  [".msi", ["windows-x86_64"]],
  [".AppImage", ["linux-x86_64", "linux-x86_64-appimage"]],
  [".deb", ["linux-x86_64-deb"]],
  [".rpm", ["linux-x86_64-rpm"]],
];

/** Записи, которые обязаны быть в latest.json релиза (проверяется в release.yml через --require-all). */
export const REQUIRED_PLATFORMS = Object.freeze([
  "darwin-aarch64",
  "darwin-x86_64",
  ...PLATFORMS_BY_SUFFIX.flatMap(([, keys]) => keys),
]);

function platformsOf(name) {
  if (name.endsWith(".app.tar.gz")) {
    if (/aarch64|arm64/.test(name)) return ["darwin-aarch64"];
    if (/x64|x86_64/.test(name)) return ["darwin-x86_64"];
    throw new Error(`в имени ${name} нет архитектуры (aarch64/x64)`);
  }
  for (const [suffix, keys] of PLATFORMS_BY_SUFFIX) {
    if (name.endsWith(suffix)) return keys;
  }
  return [];
}

/** files: { имя артефакта: содержимое .sig } */
export function buildLatest({ version, notes, baseUrl, files, pubDate }) {
  const platforms = {};
  for (const [name, signature] of Object.entries(files)) {
    const keys = platformsOf(name);
    if (keys.length === 0) continue;
    if (!signature?.trim()) throw new Error(`нет подписи для ${name}`);
    for (const platform of keys) {
      if (platforms[platform]) throw new Error(`две сборки для ${platform}: ${name}`);
      platforms[platform] = { signature: signature.trim(), url: `${baseUrl}/${encodeURIComponent(name)}` };
    }
  }
  return { version, notes, pub_date: pubDate, platforms };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const { values } = parseArgs({
    options: {
      version: { type: "string" }, "notes-file": { type: "string" }, "base-url": { type: "string" }, dir: { type: "string" },
      "require-all": { type: "boolean", default: false },
    },
  });
  for (const k of ["version", "notes-file", "base-url", "dir"]) {
    if (!values[k]) {
      console.error("использование: make-latest-json.mjs --version <v> --notes-file <f> --base-url <url> --dir <артефакты> [--require-all]");
      process.exit(2);
    }
  }
  const dir = values.dir;
  const names = readdirSync(dir);
  const artifacts = names.filter((n) => platformsOf(n).length > 0);
  // Артефакт без .sig — ошибка: значит, сборка шла без ключа подписи updater.
  const files = Object.fromEntries(
    artifacts.map((n) => [n, names.includes(`${n}.sig`) ? readFileSync(join(dir, `${n}.sig`), "utf8") : ""]),
  );
  const latest = buildLatest({
    version: values.version,
    notes: readFileSync(values["notes-file"], "utf8").trim(),
    baseUrl: values["base-url"].replace(/\/+$/, ""),
    files,
    pubDate: new Date().toISOString(),
  });
  if (Object.keys(latest.platforms).length === 0) {
    console.error(`в ${dir} нет артефактов updater`);
    process.exit(1);
  }
  if (values["require-all"]) {
    const missing = REQUIRED_PLATFORMS.filter((p) => !latest.platforms[p]);
    if (missing.length > 0) {
      console.error(`в latest.json нет платформ: ${missing.join(", ")}`);
      process.exit(1);
    }
  }
  process.stdout.write(JSON.stringify(latest, null, 2) + "\n");
}
