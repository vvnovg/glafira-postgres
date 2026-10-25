#!/usr/bin/env node
// latest.json для tauri-plugin-updater (артефакты v2) из собранных файлов и их .sig.
// macOS-архив Tauri называется «<productName>.app.tar.gz» без архитектуры —
// release.yml переименовывает его в «…_<версия>_<aarch64|x64>.app.tar.gz» до загрузки.
// Заметки: `notes` — английский раздел CHANGELOG.md (его показывают версии до 0.2.0 включительно), `notes_i18n` —
// разделы на всех языках интерфейса; tauri-plugin-updater пропускает незнакомое поле, приложение читает его из raw_json.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { readChangelogs, releaseNotes } from "./changelog.mjs";
import { languageCodes } from "./i18n-check.mjs";

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

/** notes: { код языка: Markdown }, en обязателен; files: { имя артефакта: содержимое .sig } */
export function buildLatest({ version, notes, baseUrl, files, pubDate }) {
  if (!notes.en) throw new Error(`нет английских заметок к ${version} (раздел в CHANGELOG.md)`);
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
  return { version, notes: notes.en, notes_i18n: notes, pub_date: pubDate, platforms };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const { values } = parseArgs({
    options: {
      version: { type: "string" }, "base-url": { type: "string" }, dir: { type: "string" }, "notes-out": { type: "string" },
      "require-all": { type: "boolean", default: false },
    },
  });
  for (const k of ["version", "base-url", "dir"]) {
    if (!values[k]) {
      console.error(
        "использование: make-latest-json.mjs --version <v> --base-url <url> --dir <артефакты> [--notes-out <файл>] [--require-all]\n" +
          "запускать из корня репозитория: заметки — разделы «## <v>» из CHANGELOG.md и docs/changelog/<код>.md",
      );
      process.exit(2);
    }
  }
  const codes = languageCodes(readFileSync("src/i18n/languages.ts", "utf8"));
  const notes = releaseNotes(readChangelogs(process.cwd(), codes), values.version);
  if (!notes.en) {
    console.error(`CHANGELOG.md: нет раздела «## ${values.version}» или он пуст`);
    process.exit(1);
  }
  const untranslated = codes.filter((c) => !notes[c]);
  if (untranslated.length > 0) {
    console.error(`::warning::нет заметок к ${values.version} на языках ${untranslated.join(", ")} — приложение покажет английские`);
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
    notes,
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
  // Тело GitHub-релиза — тот же английский раздел, что и `notes`.
  if (values["notes-out"]) writeFileSync(values["notes-out"], `${notes.en}\n`);
  process.stdout.write(JSON.stringify(latest, null, 2) + "\n");
}
