#!/usr/bin/env node
// Проверка собранных чанков (dist/assets/*.js) на синтаксис и API, которых нет в WebKit на macOS 11–12 (Safari 14–15.3).
// Синтаксис Vite понижает до build.target, но lookbehind не понижается, а API не полифилятся — их ловит эта проверка.
// Тест noLookbehind.test.ts видит только src; здесь — и зависимости. Только модули node:.
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

/** Синтаксис, который старый WebKit не разбирает: чанк не загружается целиком. */
const SYNTAX = [
  { name: "lookbehind (?<= / (?<! в регулярном выражении (Safari 16.4)", re: /\(\?<[=!]/ },
  { name: "приватное поле или метод класса #x (Safari 14.1/15)", re: /[{};]#[A-Za-z_$][\w$]*\s*[=;(]|\bthis\.#[A-Za-z_$]/ },
  { name: "статический блок класса static{} (Safari 16.4)", re: /\bstatic\s*\{/ },
];

/**
 * API новее Safari 14. ok — чем вызов допустим: polyfill ищется во всех чанках (src/polyfills.ts),
 * guard — в том же чанке (проверка typeof перед вызовом).
 */
const APIS = [
  { name: "Object.hasOwn (Safari 15.4)", re: /\bObject\.hasOwn\(/, polyfill: /defineProperty\([\w$]+,\s*[`"']hasOwn[`"']/ },
  { name: "structuredClone (Safari 15.4)", re: /\bstructuredClone\(/, guard: /typeof structuredClone\b/ },
  { name: "Array/String.prototype.at (Safari 15.4)", re: /\.at\(/ },
  { name: "findLast / findLastIndex (Safari 15.4)", re: /\.findLast(?:Index)?\(/ },
  { name: "toSorted / toReversed / toSpliced (Safari 16)", re: /\.to(?:Sorted|Reversed|Spliced)\(/ },
  { name: "Object.groupBy / Map.groupBy (Safari 17.4)", re: /\b(?:Object|Map)\.groupBy\(/ },
  { name: "Promise.withResolvers (Safari 17.4)", re: /\bPromise\.withResolvers\(/ },
  { name: "Array.fromAsync (Safari 16.4)", re: /\bArray\.fromAsync\(/ },
  { name: "isWellFormed / toWellFormed (Safari 16.4)", re: /\.(?:is|to)WellFormed\(/ },
  { name: "AbortSignal.timeout / AbortSignal.any (Safari 16 / 17.4)", re: /\bAbortSignal\.(?:timeout|any)\(/ },
  { name: "crypto.randomUUID (Safari 15.4)", re: /\bcrypto\.randomUUID\(/ },
  { name: "методы Set: union, intersection… (Safari 17)", re: /\.(?:union|intersection|difference|symmetricDifference|isSubsetOf|isSupersetOf|isDisjointFrom)\(/ },
];

/** files: { имя: содержимое чанка }. Возвращает список проблем «имя: что найдено — фрагмент». */
export function checkDist(files) {
  const problems = [];
  const all = Object.values(files).join("\n");
  const at = (name, text, m) => `${name}: ${m.what} — …${text.slice(Math.max(0, m.index - 30), m.index + 30)}…`;
  for (const [name, text] of Object.entries(files)) {
    for (const { name: what, re } of SYNTAX) {
      const m = re.exec(text);
      if (m) problems.push(at(name, text, { what, index: m.index }));
    }
    for (const { name: what, re, polyfill, guard } of APIS) {
      const m = re.exec(text);
      if (!m) continue;
      if (polyfill && polyfill.test(all)) continue;
      if (guard && guard.test(text)) continue;
      problems.push(at(name, text, { what: `${what} без полифила`, index: m.index }));
    }
  }
  return problems;
}

/** Все .js в каталоге (без подкаталогов). Пустой каталог — ошибка: проверка без сборки ничего не доказывает. */
export function checkDistDir(dir) {
  const names = readdirSync(dir).filter((n) => n.endsWith(".js") && statSync(join(dir, n)).isFile());
  if (!names.length) throw new Error(`в ${dir} нет .js — сначала соберите фронтенд`);
  return checkDist(Object.fromEntries(names.map((n) => [n, readFileSync(join(dir, n), "utf8")])));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const dir = process.argv[2] ?? "dist/assets";
  const problems = checkDistDir(dir);
  if (problems.length) {
    console.error(problems.map((p) => `✗ ${p}`).join("\n"));
    console.error("Старый WebKit (macOS 11–12) этого не выполнит: замените, понизьте или добавьте полифил в src/polyfills.ts.");
    process.exit(1);
  }
  console.log(`✓ ${dir}: чанки совместимы с WebKit macOS 11+`);
}
