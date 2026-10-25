import { test } from "node:test";
import assert from "node:assert/strict";
import { buildLatest, REQUIRED_PLATFORMS } from "./make-latest-json.mjs";

const N = { en: "n" };

test("платформы по именам файлов", () => {
  const files = {
    "App_aarch64.app.tar.gz": "SIG-A",
    "App_x64.app.tar.gz": "SIG-X",
    "App_0.2.0_x64_en-US.msi": "SIG-W",
    "app_0.2.0_amd64.AppImage": "SIG-L",
    "app_0.2.0_amd64.deb": "SIG-D",
    "app-0.2.0-1.x86_64.rpm": "SIG-R",
  };
  const latest = buildLatest({ version: "0.2.0", notes: N, baseUrl: "https://g/r/releases/download/v0.2.0", files, pubDate: "2026-11-01T00:00:00Z" });
  assert.equal(latest.version, "0.2.0");
  assert.deepEqual(Object.keys(latest.platforms).sort(), [
    "darwin-aarch64",
    "darwin-x86_64",
    "linux-x86_64",
    "linux-x86_64-appimage",
    "linux-x86_64-deb",
    "linux-x86_64-rpm",
    "windows-x86_64",
  ]);
  assert.equal(latest.platforms["windows-x86_64"].url, "https://g/r/releases/download/v0.2.0/App_0.2.0_x64_en-US.msi");
  assert.equal(latest.platforms["darwin-aarch64"].signature, "SIG-A");
  assert.equal(latest.platforms["darwin-x86_64"].signature, "SIG-X");
});

test("нет подписи — ошибка", () => {
  assert.throws(() => buildLatest({ version: "1", notes: N, baseUrl: "u", files: { "a.msi": "" }, pubDate: "d" }), /подпис/);
});

test("имя с пробелами кодируется в URL", () => {
  const latest = buildLatest({ version: "1", notes: N, baseUrl: "https://x", files: { "My App_1_x64_en-US.msi": "S" }, pubDate: "d" });
  assert.equal(latest.platforms["windows-x86_64"].url, "https://x/My%20App_1_x64_en-US.msi");
});

test("macOS-архив без архитектуры в имени — ошибка", () => {
  assert.throws(() => buildLatest({ version: "1", notes: N, baseUrl: "u", files: { "App.app.tar.gz": "S" }, pubDate: "d" }), /архитектур/);
});

test("две сборки на одну платформу — ошибка", () => {
  assert.throws(
    () => buildLatest({ version: "1", notes: N, baseUrl: "u", files: { "a_x64.msi": "S", "b_x64.msi": "S" }, pubDate: "d" }),
    /windows-x86_64/,
  );
});

test("Linux: отдельная запись на каждый тип установки, linux-x86_64 — AppImage", () => {
  // tauri-plugin-updater ищет «{os}-{arch}-{установщик}», затем «{os}-{arch}»: установка из .deb,
  // получив AppImage по общему ключу, падала бы с InvalidUpdaterFormat.
  const files = { "a_1_amd64.AppImage": "L", "a_1_amd64.deb": "D", "a-1-1.x86_64.rpm": "R" };
  const { platforms } = buildLatest({ version: "1", notes: N, baseUrl: "https://x", files, pubDate: "d" });
  assert.equal(platforms["linux-x86_64"].url, "https://x/a_1_amd64.AppImage");
  assert.equal(platforms["linux-x86_64-appimage"].url, "https://x/a_1_amd64.AppImage");
  assert.equal(platforms["linux-x86_64-deb"].url, "https://x/a_1_amd64.deb");
  assert.equal(platforms["linux-x86_64-deb"].signature, "D");
  assert.equal(platforms["linux-x86_64-rpm"].url, "https://x/a-1-1.x86_64.rpm");
  assert.equal(platforms["linux-x86_64-rpm"].signature, "R");
});

test("REQUIRED_PLATFORMS — полный набор записей релиза", () => {
  assert.deepEqual([...REQUIRED_PLATFORMS].sort(), [
    "darwin-aarch64",
    "darwin-x86_64",
    "linux-x86_64",
    "linux-x86_64-appimage",
    "linux-x86_64-deb",
    "linux-x86_64-rpm",
    "windows-x86_64",
  ]);
});

test(".deb без подписи — ошибка", () => {
  assert.throws(() => buildLatest({ version: "1", notes: N, baseUrl: "u", files: { "a.deb": "" }, pubDate: "d" }), /подпис/);
});

test("заметки: notes — английский раздел, notes_i18n — все языки", () => {
  const notes = { en: "- fixes", ru: "- исправления", de: "- Korrekturen" };
  const latest = buildLatest({ version: "1", notes, baseUrl: "https://x", files: { "a.msi": "S" }, pubDate: "d" });
  assert.equal(latest.notes, "- fixes");
  assert.deepEqual(latest.notes_i18n, notes);
  // Порядок полей как у Tauri: version, notes, pub_date, platforms; notes_i18n — рядом с notes.
  assert.deepEqual(Object.keys(latest), ["version", "notes", "notes_i18n", "pub_date", "platforms"]);
});

test("нет английских заметок — ошибка", () => {
  assert.throws(() => buildLatest({ version: "1", notes: { ru: "- x" }, baseUrl: "u", files: { "a.msi": "S" }, pubDate: "d" }), /CHANGELOG\.md/);
});
