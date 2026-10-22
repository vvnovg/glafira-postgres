#!/usr/bin/env node
// Проверки локализации: одинаковые ключи и параметры во всех словарях, ключи из Rust, забытая кириллица в коде.
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const PLURAL = /_(zero|one|two|few|many|other)$/;

export function flatten(obj, prefix = "", out = new Map()) {
  for (const [k, v] of Object.entries(obj)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === "object") flatten(v, key, out);
    else out.set(key, String(v));
  }
  return out;
}

export function placeholders(text) {
  return [...new Set([...text.matchAll(/\{\{\s*([\w.]+)\s*\}\}/g)].map((m) => m[1]))];
}

const TAG = /<\/?[A-Za-z][\w-]*\s*\/?>/g;

/** Теги в строке (`<code>`, `</code>`, `<offset/>`, буквальные `<file>`) — отсортированный список с повторами. */
export function tags(text) {
  return [...text.matchAll(TAG)].map((m) => m[0].replace(/\s+/g, "")).sort();
}

/**
 * Пары открывающий/закрывающий тег (`<code>…</code>`) должны быть парными и вложенными правильно.
 * Порядок остальных тегов (`<offset/>`, `<total/>`) не сравниваем: в разных языках слова стоят по-разному.
 * Тег без закрывающего в этой же строке (буквальный `<file>`) не парный. Возвращает текст проблемы или null.
 */
export function pairedTagsProblem(text) {
  const found = [...text.matchAll(TAG)].map((m) => m[0].replace(/\s+/g, ""));
  const nameOf = (t) => t.replace(/^<\/?|\/?>$/g, "");
  const closing = new Set(found.filter((t) => t.startsWith("</")).map(nameOf));
  const stack = [];
  for (const t of found) {
    if (t.endsWith("/>")) continue;
    const name = nameOf(t);
    if (!t.startsWith("</")) {
      if (closing.has(name)) stack.push(name);
    } else if (stack.pop() !== name) return `парные теги не сходятся (${found.join(" ")})`;
  }
  return stack.length ? `парные теги не сходятся (${found.join(" ")})` : null;
}

function pluralForms(code) {
  const rules = new Intl.PluralRules(code);
  return rules.resolvedOptions().pluralCategories;
}

/** Форма `one` языка покрывает ровно число 1 (в fr и pt-BR — ещё 0, в ru — 21, 31…) — тогда {{count}} в `_one` можно не писать. */
export function oneMeansExactlyOne(code) {
  const rules = new Intl.PluralRules(code);
  for (let n = 0; n <= 1000; n++) if ((rules.select(n) === "one") !== (n === 1)) return false;
  return true;
}

/** Ключи словаря (в точечной записи) со значением, которое не непустая строка. */
export function badValues(obj, prefix = "", out = []) {
  for (const [k, v] of Object.entries(obj)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === "object" && !Array.isArray(v)) badValues(v, key, out);
    else if (typeof v !== "string" || v.trim() === "") out.push(key);
  }
  return out;
}

/** dicts: { en: {...}, ru: {...} } — первый (en) эталон. Возвращает список проблем. */
export function checkDictionaries(dicts) {
  const problems = [];
  for (const [code, d] of Object.entries(dicts))
    for (const k of badValues(d)) problems.push(`${code}: ${k} — пустое или не строковое значение`);
  const flat = Object.fromEntries(Object.entries(dicts).map(([c, d]) => [c, flatten(d)]));
  const base = flat.en;
  // Базовые ключи без плюрального суффикса и их параметры (по форме _other или самому ключу).
  const baseKeys = new Map();
  for (const [k, v] of base) {
    const stem = k.replace(PLURAL, "");
    const params = new Set(placeholders(v));
    const prev = baseKeys.get(stem);
    baseKeys.set(stem, {
      plural: PLURAL.test(k),
      params: prev ? new Set([...prev.params, ...params]) : params,
      tags: prev ? prev.tags : tags(v),
    });
  }
  for (const [code, map] of Object.entries(flat)) {
    if (code === "en") continue;
    const forms = pluralForms(code);
    const oneExact = oneMeansExactlyOne(code);
    for (const [stem, info] of baseKeys) {
      const keys = info.plural ? forms.map((f) => `${stem}_${f}`) : [stem];
      for (const key of keys) {
        if (!map.has(key)) {
          problems.push(`${code}: нет ключа ${key}`);
          continue;
        }
        const got = new Set(placeholders(map.get(key)));
        // «count» можно опустить только в _one и только там, где one — ровно число 1 (не fr, pt-BR, ru…).
        const countOptional = oneExact && key === `${stem}_one`;
        for (const p of info.params) {
          if (!got.has(p) && !(info.plural && p === "count" && countOptional)) problems.push(`${code}: ${key} без параметра {{${p}}}`);
        }
        for (const p of got) if (!info.params.has(p)) problems.push(`${code}: ${key} с лишним параметром {{${p}}}`);
        const gotTags = tags(map.get(key));
        if (gotTags.join(" ") !== info.tags.join(" "))
          problems.push(`${code}: ${key} — теги ${gotTags.join(" ") || "(нет)"}, в en ${info.tags.join(" ") || "(нет)"}`);
        else {
          const paired = pairedTagsProblem(map.get(key));
          if (paired) problems.push(`${code}: ${key} — ${paired}`);
        }
      }
    }
    for (const k of map.keys()) {
      const stem = k.replace(PLURAL, "");
      const info = baseKeys.get(stem);
      if (!info) problems.push(`${code}: лишний ключ ${k} (нет в en)`);
      else if (PLURAL.test(k) && !info.plural) problems.push(`${code}: лишний ключ ${k} (в en нет плюральных форм)`);
      else if (!PLURAL.test(k) && info.plural) problems.push(`${code}: лишний ключ ${k} (в en только плюральные формы)`);
      else if (info.plural && !forms.includes(k.slice(stem.length + 1)))
        problems.push(`${code}: ${k} — лишняя форма (язык не использует ${k.slice(stem.length + 1)})`);
    }
  }
  return problems;
}

const hasLetters = (v) => /[A-Za-zÀ-ž]/.test(v.replace(/\{\{[^}]*\}\}|<[^>]*>/g, ""));

/** Строки, совпадающие с en буква в букву (вероятно, забыли перевести). allow: { code: [ключ] } — намеренные совпадения. */
export function checkSameAsEn(dicts, allow = {}) {
  const problems = [];
  const en = flatten(dicts.en);
  const enOf = (k) => en.get(k) ?? en.get(`${k.replace(PLURAL, "")}_other`);
  for (const code of Object.keys(allow))
    if (!(code in dicts)) problems.push(`${code}: в docs/i18n/same-as-en.json есть язык, для которого нет словаря`);
  for (const [code, dict] of Object.entries(dicts)) {
    if (code === "en") continue;
    const map = flatten(dict);
    const ok = new Set(allow[code] ?? []);
    const forms = pluralForms(code);
    for (const k of ok) {
      const stem = k.replace(PLURAL, "");
      const form = PLURAL.test(k) ? k.slice(stem.length + 1) : null;
      const inEn = form ? [...en.keys()].some((e) => e.replace(PLURAL, "") === stem && PLURAL.test(e)) && forms.includes(form) : en.has(k);
      if (!inEn) problems.push(`${code}: ${k} в docs/i18n/same-as-en.json, но такого ключа (или формы для языка) нет в en — уберите из списка`);
    }
    for (const [k, v] of map) {
      if (v === enOf(k) && hasLetters(v) && !ok.has(k))
        problems.push(`${code}: ${k} совпадает с en «${v}» — переведите или добавьте ключ в docs/i18n/same-as-en.json`);
    }
    for (const k of ok) {
      if (map.has(k) && map.get(k) !== enOf(k))
        problems.push(`${code}: ${k} в docs/i18n/same-as-en.json, но перевод отличается от en — уберите из списка`);
    }
  }
  return problems;
}

/** Типографика: «...» вместо «…» во всех языках; во французском — U+00A0 перед : ; ! ? и внутри « ». */
export function checkTypography(dicts) {
  const problems = [];
  for (const [code, dict] of Object.entries(dicts)) {
    for (const [k, v] of flatten(dict)) {
      if (v.includes("...")) problems.push(`${code}: ${k} — «...» вместо «…»`);
      for (const pr of cjkProblems(v, code)) problems.push(`${code}: ${k} — ${pr}`);
      if (code !== "fr") continue;
      for (const ch of new Set([...v.matchAll(/ ([:;!?])/g)].map((m) => m[1])))
        problems.push(`fr: ${k} — обычный пробел вместо U+00A0 перед «${ch}»`);
      if (/« | »/.test(v)) problems.push(`fr: ${k} — обычный пробел вместо U+00A0 у кавычек « »`);
    }
  }
  return problems;
}

/** Языки с полноширинной пунктуацией (проверка «полуширинная после иероглифа») и языки без пробелов между иероглифами. */
export const CJK_TYPO_LANGS = ["zh-CN", "zh-TW", "ja"];
export const HAN_SPACE_LANGS = ["zh-CN", "zh-TW"];

// ー (U+30FC) и ・ (U+30FB) — Script=Common, но пишутся внутри слов катаканой.
const CJK_CHAR = "\\p{Script=Han}\\p{Script=Hiragana}\\p{Script=Katakana}\\u30FB\\u30FC";
const HALF_PUNCT_AFTER_CJK = new RegExp(`[${CJK_CHAR}][,.:;?!]`, "gu");
const SPACE_BETWEEN_HAN = /\p{Script=Han}[ \t\u00A0\u3000]+\p{Script=Han}/gu;
// Код, параметры, теги и метки заменяются нейтральным символом: он не иероглиф и не пробел, поэтому не склеивает соседей.
const CJK_SKIP = /`[^`]*`|\{\{[^}]*\}\}|<\/?[A-Za-z0-9][^>]*>|\[\[ui:[^\]]*\]\]/g;

/** Типографика zh/ja: полуширинная `, . : ; ? !` сразу после иероглифа или каны; zh — пробел между иероглифами. */
export function cjkProblems(text, code) {
  if (!CJK_TYPO_LANGS.includes(code)) return [];
  const t = text.replace(CJK_SKIP, "\u0001");
  const problems = [];
  for (const m of new Set([...t.matchAll(HALF_PUNCT_AFTER_CJK)].map((x) => x[0])))
    problems.push(`полуширинная пунктуация после иероглифа «${m}» — нужна полноширинная`);
  if (HAN_SPACE_LANGS.includes(code))
    for (const m of new Set([...t.matchAll(SPACE_BETWEEN_HAN)].map((x) => x[0]))) problems.push(`пробел между иероглифами «${m}»`);
  return problems;
}

// Полноширинные: иероглифы, кана (без полуширинной ｱ-ﾝ), хангыль (без полуширинного ﾡ-ￜ), символы CJK U+3000–U+303F,
// ー и ・, полноширинные формы ！-｠ и ￠-￦.
const WIDE = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}\u3000-\u303F\u30FB\u30FC\uFF01-\uFF60\uFFE0-\uFFE6]/u;
const HALFWIDTH = /[\uFF61-\uFFDC\uFFE8-\uFFEE]/u;

/** Видимая ширина строки: иероглиф, кана, хангыль и полноширинные формы — 2, остальное (и полуширинные формы) — 1. */
export function visualWidth(s) {
  let w = 0;
  for (const ch of s) w += WIDE.test(ch) && !HALFWIDTH.test(ch) ? 2 : 1;
  return w;
}

/** Коды языков из src/i18n/languages.ts (поля `code: "…"`). */
export function languageCodes(languagesTs) {
  return [...languagesTs.matchAll(/\bcode:\s*"([\w-]+)"/g)].map((m) => m[1]);
}

/** Каждый словарь — язык из списка; с requireAll — у каждого языка из списка есть словарь. */
export function checkLanguageFiles(codes, dictCodes, { requireAll }) {
  const problems = [];
  for (const d of dictCodes) if (!codes.includes(d)) problems.push(`словарь src/locales/${d}.json — языка ${d} нет в src/i18n/languages.ts`);
  if (requireAll) for (const c of codes) if (!dictCodes.includes(c)) problems.push(`нет словаря src/locales/${c}.json`);
  return problems;
}

/** Строки, заметно длиннее английских, — кандидаты на обрезание в вёрстке (для ручной проверки, не ошибка). */
export function lengthReport(dicts, { ratio = 1.6, minLength = 6, top = 15 } = {}) {
  const en = flatten(dicts.en);
  const out = [];
  for (const [code, dict] of Object.entries(dicts)) {
    if (code === "en") continue;
    const rows = [];
    for (const [k, v] of flatten(dict)) {
      const base = en.get(k) ?? en.get(`${k.replace(PLURAL, "")}_other`);
      if (!base || base.length < minLength) continue;
      const len = visualWidth(base);
      const r = visualWidth(v) / len;
      if (r < ratio) continue;
      rows.push({ k, v, r, len });
    }
    rows.sort((a, b) => b.r - a.r);
    for (const { k, v, r, len } of rows.slice(0, top))
      out.push(`${code}: ${k} — ${visualWidth(v)} / ${len} (×${r.toFixed(1)}) «${v}»`);
  }
  return out;
}

/** files: { path: content }. Кириллица в строках кода фронтенда вне словарей, тестов и комментариев. */
export function findCyrillic(files) {
  const hits = [];
  for (const [path, content] of Object.entries(files)) {
    if (/\.test\.[tj]sx?$/.test(path) || path.includes("/locales/") || path.includes("/test/")) continue;
    if (path.endsWith("src/i18n/languages.ts")) continue; // самоназвания языков — данные
    content.split("\n").forEach((line, i) => {
      const code = line.replace(/\/\/.*$/, "").replace(/\/\*.*?\*\//g, "");
      if (/^\s*\*/.test(line)) return; // строки JSDoc-блоков
      if (/[А-Яа-яЁё]/.test(code)) hits.push(`${path}:${i + 1}`);
    });
  }
  return hits;
}

/**
 * files: { path: content }. Статические ключи фронтенда: t("…"), tt("…"), i18n.t("…"), i18nKey="…", labelKey: "…".
 * Динамические (шаблонные строки, конкатенация) пропускаются. Возвращает [{ key, where }].
 */
export function frontendKeys(files) {
  const out = [];
  const re = /(?:\b(?:t|tt|i18n\.t)\(\s*|\bi18nKey=\{?\s*|\blabelKey:\s*)(["'])([\w.]+)\1(?=\s*[,)}]|\s*\/?>)/g;
  for (const [path, content] of Object.entries(files)) {
    if (/\.test\.[tj]sx?$/.test(path) || path.includes("/test/")) continue;
    for (const m of content.matchAll(re)) {
      out.push({ key: m[2], where: `${path}:${content.slice(0, m.index).split("\n").length}` });
    }
  }
  return out;
}

/** Каждый статический ключ фронтенда есть в en.json (точно или как основа плюральных форм). */
export function checkFrontendKeys(keys, enFlat) {
  const stems = new Set([...enFlat.keys()].map((k) => k.replace(PLURAL, "")));
  return keys
    .filter(({ key }) => !enFlat.has(key) && !stems.has(key))
    .map(({ key, where }) => `en: нет ключа ${key} (используется во фронтенде, ${where})`);
}

const HINT_LITERAL = /["'](hints\.[\w.]+)["']/g;

/** Ключи подсказок во фронтенде: любой строковый литерал "hints.…" вне тестов. Возвращает [{ key, where }]. */
export function hintKeys(files) {
  const out = [];
  for (const [path, content] of Object.entries(files)) {
    if (/\.test\.[tj]sx?$/.test(path) || path.includes("/test/")) continue;
    for (const m of content.matchAll(HINT_LITERAL)) {
      out.push({ key: m[1], where: `${path}:${content.slice(0, m.index).split("\n").length}` });
    }
  }
  return out;
}

/** Каждая подсказка из кода есть в en; каждая подсказка en (кроме hints.label) используется; en не длиннее maxLength без тегов. */
export function checkHints(uses, enFlat, { maxLength = 200 } = {}) {
  const problems = [];
  const used = new Set(uses.map((u) => u.key));
  for (const { key, where } of uses) if (!enFlat.has(key)) problems.push(`en: нет ключа ${key} (подсказка, ${where})`);
  for (const [k, v] of enFlat) {
    if (!k.startsWith("hints.") || k === "hints.label") continue;
    if (!used.has(k)) problems.push(`en: ${k} не используется ни в одном <Hint> — удалите или подключите`);
    const plain = v.replace(/<[^>]+>/g, "");
    if (plain.length > maxLength) problems.push(`en: ${k} — ${plain.length} символов, больше ${maxLength}`);
  }
  return problems;
}

/** Код Rust без строк-комментариев и без модуля тестов в конце файла (там бывают тестовые ключи).
 * Строки-комментарии заменяются пустыми, чтобы номера строк в сообщениях совпадали с файлом. */
function rustCode(content) {
  const tests = content.search(/^#\[cfg\(test\)\]\s*mod tests\b/m);
  const code = tests >= 0 ? content.slice(0, tests) : content;
  return code
    .split("\n")
    .map((line) => (/^\s*\/\//.test(line) ? "" : line))
    .join("\n");
}

/** Конец обычного строкового литерала Rust, начинающегося с `"` в позиции i (индекс после закрывающей кавычки) или -1. */
function stringEnd(src, i) {
  for (let j = i + 1; j < src.length; j++) {
    if (src[j] === "\\") j++;
    else if (src[j] === '"') return j + 1;
  }
  return -1;
}

/** Аргументы макроса верхнего уровня, начиная сразу после `(`; null — если скобки не сошлись. */
function splitArgs(src, pos) {
  const args = [];
  let depth = 0;
  let start = pos;
  for (let i = pos; i < src.length; ) {
    const c = src[i];
    const raw = /^r(#*)"/.exec(src.slice(i, i + 64));
    if (raw && !/\w/.test(src[i - 1] ?? "")) {
      const close = src.indexOf(`"${raw[1]}`, i + raw[0].length);
      if (close < 0) return null;
      i = close + 1 + raw[1].length;
    } else if (c === '"') {
      i = stringEnd(src, i);
      if (i < 0) return null;
    } else if (c === "'") {
      const ch = /^'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^\\'\n])'/.exec(src.slice(i, i + 16));
      i += ch ? ch[0].length : 1;
    } else if (src.startsWith("//", i)) {
      const nl = src.indexOf("\n", i);
      i = nl < 0 ? src.length : nl;
    } else if (src.startsWith("/*", i)) {
      const e = src.indexOf("*/", i + 2);
      if (e < 0) return null;
      i = e + 2;
    } else {
      if ("([{".includes(c)) depth++;
      else if (")]}".includes(c)) {
        if (depth === 0) {
          if (c !== ")") return null;
          args.push(src.slice(start, i));
          if (args.length && args[args.length - 1].trim() === "") args.pop(); // запятая в конце
          return { args };
        }
        depth--;
      } else if (c === "," && depth === 0) {
        args.push(src.slice(start, i));
        start = i + 1;
      }
      i++;
    }
  }
  return null;
}

/** Значение обычного строкового литерала Rust (весь аргумент — ровно один литерал) или { error }. */
export function rustString(arg) {
  const a = arg.trim();
  if (/^(?:b|c)?r#*"/.test(a) || /^[bc]"/.test(a)) return { error: "нужен обычный строковый литерал (не r\"…\", b\"…\", c\"…\")" };
  if (!a.startsWith('"') || stringEnd(a, 0) !== a.length) return { error: "нужен один строковый литерал" };
  const body = a.slice(1, -1);
  let out = "";
  for (let i = 0; i < body.length; i++) {
    if (body[i] !== "\\") {
      out += body[i];
      continue;
    }
    const n = body[++i];
    const simple = { n: "\n", t: "\t", r: "\r", 0: "\0", "\\": "\\", '"': '"', "'": "'" };
    if (n in simple) out += simple[n];
    else if (n === "\n" || n === "\r") {
      // Перенос строки после `\`: Rust пропускает его и пробелы в начале следующей строки.
      while (/\s/.test(body[i + 1] ?? "")) i++;
    } else if (n === "x" && /^[0-7][0-9a-fA-F]$/.test(body.slice(i + 1, i + 3))) {
      out += String.fromCharCode(parseInt(body.slice(i + 1, i + 3), 16));
      i += 2;
    } else {
      const u = n === "u" ? /^\{([0-9a-fA-F_]{1,8})\}/.exec(body.slice(i + 1)) : null;
      if (!u) return { error: `неизвестная escape-последовательность \\${n}` };
      out += String.fromCodePoint(parseInt(u[1].replace(/_/g, ""), 16));
      i += u[0].length;
    }
  }
  return { value: out };
}

/**
 * Вызовы app_err!(код, "key", "English {{p}}", p = …) и ltext!("key", "English", p = …) в коде Rust.
 * Каждое вхождение `app_err!` / `ltext!` либо разбирается, либо попадает в problems — макрос не пропадает молча.
 * Возвращает { calls: [{ key, template, params, where }], problems }.
 */
export function rustKeys(files /* {path: content} */) {
  const calls = [];
  const problems = [];
  for (const [path, content] of Object.entries(files)) {
    const src = rustCode(content);
    for (const m of src.matchAll(/\b(app_err|ltext)!\s*([([{])/g)) {
      const where = `${path}:${src.slice(0, m.index).split("\n").length}`;
      const fail = (why) => problems.push(`${where}: макрос ${m[1]}!, который проверка не может разобрать: ${why}`);
      if (m[2] !== "(") {
        fail("вызывайте с круглыми скобками");
        continue;
      }
      const parsed = splitArgs(src, m.index + m[0].length);
      if (!parsed) {
        fail("не найдена закрывающая скобка");
        continue;
      }
      const args = parsed.args.map((a) => a.trim());
      const rest = m[1] === "app_err" ? args.slice(1) : args;
      if (rest[0]?.startsWith("$")) continue; // тело другого macro_rules (метапеременные), не вызов
      if (rest.length < 2) {
        fail("ожидаются ключ и английский шаблон");
        continue;
      }
      const key = rustString(rest[0]);
      const template = rustString(rest[1]);
      if (key.error || !/^[\w.]+$/.test(key.value)) {
        fail(`ключ: ${key.error ?? `недопустимый ключ «${key.value}»`}`);
        continue;
      }
      if (template.error) {
        fail(`шаблон ${key.value}: ${template.error}`);
        continue;
      }
      const params = [];
      let bad = null;
      for (const p of rest.slice(2)) {
        const pm = /^([A-Za-z_]\w*)\s*=(?!=)\s*\S/.exec(p);
        if (pm) params.push(pm[1]);
        else bad = p;
      }
      if (bad !== null) {
        fail(`${key.value}: параметр «${bad}» не в виде name = значение`);
        continue;
      }
      calls.push({ key: key.value, template: template.value, params, where });
    }
  }
  return { calls, problems };
}

/** Каждый ключ Rust есть в en.json, текст en совпадает с шаблоном Rust, параметры вызова = {{…}} шаблона.
 * Плюральный ключ передаётся без суффикса (`warnings.x`): шаблон Rust — форма `_other` из en, параметр `count` обязателен. */
export function checkRustKeys(calls, enFlat) {
  const problems = [];
  for (const { key, template, params, where } of calls) {
    const plural = !enFlat.has(key) && enFlat.has(`${key}_other`);
    const enKey = plural ? `${key}_other` : key;
    if (!enFlat.has(enKey)) problems.push(`en: нет ключа ${key} (используется в Rust, ${where})`);
    else if (enFlat.get(enKey) !== template)
      problems.push(`en: ${enKey} отличается от шаблона в Rust (${where}): «${enFlat.get(enKey)}» ≠ «${template}»`);
    if (plural && !params.includes("count")) problems.push(`${where}: ${key} — плюральный ключ без параметра count`);
    const used = new Set(placeholders(template));
    for (const p of used) if (!params.includes(p)) problems.push(`${where}: ${key} — нет параметра ${p} для {{${p}}}`);
    for (const p of params) if (!used.has(p)) problems.push(`${where}: ${key} — лишний параметр ${p}, в шаблоне нет {{${p}}}`);
    const dup = params.filter((p, i) => params.indexOf(p) !== i);
    for (const p of new Set(dup)) problems.push(`${where}: ${key} — параметр ${p} передан дважды`);
  }
  return problems;
}

const CYR = /[А-Яа-яЁё]/;

/** Индекс сразу после элемента, помеченного `#[cfg(test)]` (атрибут кончается в позиции pos):
 * до `;` или до парной `}` первого блока на верхнем уровне. */
function skipItem(src, pos) {
  let depth = 0;
  for (let i = pos; i < src.length; ) {
    const lit = literalAt(src, i);
    if (lit) {
      i = lit.end;
      continue;
    }
    const c = src[i];
    if (c === "{") depth++;
    else if (c === "}") {
      depth--;
      if (depth === 0) return i + 1;
    } else if (c === ";" && depth === 0) return i + 1;
    i++;
  }
  return src.length;
}

/** Литерал или комментарий Rust в позиции i: { end, text?, comment? } или null. */
function literalAt(src, i) {
  if (src.startsWith("//", i)) {
    const nl = src.indexOf("\n", i);
    return { end: nl < 0 ? src.length : nl, comment: true };
  }
  if (src.startsWith("/*", i)) {
    const e = src.indexOf("*/", i + 2);
    return { end: e < 0 ? src.length : e + 2, comment: true };
  }
  const raw = /^(?:[bc])?r(#*)"/.exec(src.slice(i, i + 64));
  if (raw && !/\w/.test(src[i - 1] ?? "")) {
    const close = src.indexOf(`"${raw[1]}`, i + raw[0].length);
    const end = close < 0 ? src.length : close + 1 + raw[1].length;
    return { end, text: src.slice(i, end) };
  }
  if (src[i] === '"') {
    const end = stringEnd(src, i);
    return { end: end < 0 ? src.length : end, text: src.slice(i, end < 0 ? src.length : end) };
  }
  if (src[i] === "'") {
    const ch = /^'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^\\'\n])'/u.exec(src.slice(i, i + 16));
    if (ch) return { end: i + ch[0].length, text: ch[0] };
  }
  return null;
}

/**
 * files: { path: content }. Строковые и символьные литералы Rust с кириллицей вне комментариев
 * и вне элементов под `#[cfg(test)]` (модули тестов, тестовые помощники). Возвращает ["path:line"].
 */
export function findRustCyrillic(files) {
  const hits = [];
  for (const [path, content] of Object.entries(files)) {
    const lines = new Set();
    for (let i = 0; i < content.length; ) {
      if (content.startsWith("#[cfg(test)]", i)) {
        i = skipItem(content, i + "#[cfg(test)]".length);
        continue;
      }
      const lit = literalAt(content, i);
      if (!lit) {
        i++;
        continue;
      }
      if (lit.text && CYR.test(lit.text)) lines.add(content.slice(0, i).split("\n").length);
      i = Math.max(lit.end, i + 1);
    }
    for (const l of [...lines].sort((a, b) => a - b)) hits.push(`${path}:${l}`);
  }
  return hits;
}

export const GUIDE_CHAPTERS = ["getting-started", "updates"];

const KEBAB = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

/** Разбор главы руководства: заголовок, якоря разделов, метки `[[ui:ключ]]`, ссылки между главами и найденные нарушения формата. */
export function parseGuideChapter(text) {
  const problems = [];
  const lines = text.split("\n");
  const first = lines.find((l) => l.trim() !== "");
  const title = first && /^# \S/.test(first) ? first.slice(2).trim() : null;
  if (title === null) problems.push("первая непустая строка должна быть заголовком «# …»");
  const anchors = [];
  let fence = false;
  const prose = [];
  for (const line of lines) {
    if (/^\s*(```|~~~)/.test(line)) {
      fence = !fence;
      continue;
    }
    if (fence) continue;
    // Якорь {#id} — только у `##` и у каждого `##`: так разбирает главу приложение (src/guide/parse.ts).
    const h = /^## .*?\{#([^}]*)\}\s*$/.exec(line);
    // Inline-код заменяется нейтральным символом, чтобы не склеивать соседние слова для типографских проверок.
    const code = line.replace(/`[^`]*`/g, "\u0001");
    if (h) {
      if (!KEBAB.test(h[1])) problems.push(`якорь {#${h[1]}} не в kebab-case (латиница, цифры, дефисы)`);
      else if (anchors.includes(h[1])) problems.push(`якорь {#${h[1]}} — повтор в главе, якоря должны быть уникальными`);
      anchors.push(h[1]);
    } else if (/^##\s/.test(line)) problems.push(`«${line.trim()}» — у раздела ## нужен якорь {#id}`);
    else if (code.includes("{#")) problems.push(`«${line.trim()}» — якорь {#id} допустим только у ##`);
    prose.push(code);
  }
  const body = prose.join("\n");
  if (/<[A-Za-z]/.test(body)) problems.push("сырой HTML в главе");
  const labels = [...body.matchAll(/\[\[ui:([^\]]*)\]\]/g)].map((m) => m[1]);
  const links = [];
  for (const m of body.matchAll(/\[[^\]]*\]\(([^)\s]*)\)/g)) {
    const target = m[1];
    const md = /^([\w-]+)\.md(?:#([\w-]+))?$/.exec(target);
    if (md) links.push({ chapter: md[1], anchor: md[2] ?? null });
    else if (!target.startsWith("https://")) problems.push(`ссылка ${target} — допустимы только https:// и главы «глава.md#якорь»`);
  }
  // Проза для типографских проверок: без кода, якорей {#id} и адресов ссылок.
  const plain = body
    .replace(/\{#[^}]*\}/g, "")
    .replace(/\]\([^)]*\)/g, "]")
    .replace(/https?:\/\/\S+/g, "\u0001");
  return { title, anchors, labels, links, problems, prose: plain };
}

const MD_PUNCT = /[\p{P}\p{S}\u0001]/u;
const MD_SPACE = /\s/u;

/**
 * Метка → `**значение**`: по CommonMark `**` перед значением, которое начинается с пунктуации («…», «（», «「»),
 * не открывает жирное, если слева буква или иероглиф; так же `**` после значения, которое кончается пунктуацией,
 * не закрывает его, если справа буква. Вплотную к иероглифам (zh, ja) это частый случай.
 */
function labelFlankingProblems(text, flat) {
  const problems = [];
  // В заголовках метка подставляется без жирного.
  const prose = text.split("\n").map((l) => (/^#{1,6}\s/.test(l) ? "" : l)).join("\n");
  for (const m of prose.matchAll(/\[\[ui:([^\]]*)\]\]/g)) {
    const value = flat.get(m[1]);
    if (!value) continue;
    const before = prose[m.index - 1];
    let end = m.index + m[0].length;
    // Точку сразу после метки рендер убирает, если значение кончается на «.» или «:» (src/guide/parse.ts).
    if (prose[end] === "." && /[.:]$/.test(value)) end += 1;
    const after = prose[end];
    const tight = (ch) => ch !== undefined && !MD_SPACE.test(ch) && !MD_PUNCT.test(ch);
    const startsP = MD_PUNCT.test([...value][0]);
    const endsP = MD_PUNCT.test([...value].at(-1));
    if ((startsP && tight(before)) || (endsP && tight(after)))
      problems.push(`метка ${m[0]} («${value}») вплотную к тексту: **…** вокруг значения с пунктуацией на краю не станет жирным — поставьте рядом пробел или пунктуацию (в zh и ja — пунктуацию) либо перестройте фразу`);
  }
  return problems;
}

/** Метки → значения языка (с той же точкой, что убирает рендер) — для типографских проверок прозы. */
function withLabelValues(prose, flat) {
  return prose.replace(/\[\[ui:([^\]]*)\]\](\.)?/g, (m, key, dot) => {
    const value = flat.get(key);
    if (value === undefined) return m;
    return value + (dot && !/[.:]$/.test(value) ? dot : "");
  });
}

/**
 * files: { "<код>/<глава>.md": текст }. Глав у каждого языка ровно `chapters`; якоря как в en; метки — ключи en без параметров;
 * ссылки — на существующие главы и якоря. langFlat: { код: flatten(словарь) } — для проверки жирного у меток.
 */
export function checkGuide(files, codes, enFlat, chapters = GUIDE_CHAPTERS, langFlat = {}) {
  const problems = [];
  const parsed = {};
  for (const code of codes) {
    for (const ch of chapters) {
      const path = `${code}/${ch}.md`;
      if (!(path in files)) problems.push(`${path}: нет главы`);
      else parsed[path] = parseGuideChapter(files[path]);
    }
  }
  for (const path of Object.keys(files)) {
    const [code, name] = path.split("/");
    if (codes.includes(code) && !chapters.includes((name ?? "").replace(/\.md$/, ""))) problems.push(`${path}: лишний файл, такой главы нет в списке`);
  }
  for (const [path, ch] of Object.entries(parsed)) {
    const [code, name] = path.split("/");
    for (const pr of ch.problems) problems.push(`${path}: ${pr}`);
    if (code !== "en" && files[path] === files[`en/${name}`]) problems.push(`${path}: глава не переведена (совпадает с en)`);
    const prose = langFlat[code] ? withLabelValues(ch.prose, langFlat[code]) : ch.prose;
    for (const pr of cjkProblems(prose, code)) problems.push(`${path}: ${pr}`);
    if (langFlat[code]) for (const pr of labelFlankingProblems(ch.prose, langFlat[code])) problems.push(`${path}: ${pr}`);
    const enAnchors = parsed[`en/${name}`]?.anchors;
    if (code !== "en" && enAnchors && ch.anchors.join(" ") !== enAnchors.join(" ")) {
      problems.push(`${path}: якоря [${ch.anchors.join(", ")}] не совпадают с en [${enAnchors.join(", ")}]`);
    }
    for (const key of ch.labels) {
      if (!enFlat.has(key)) problems.push(`${path}: метка [[ui:${key}]] — нет ключа ${key} в en`);
      else if (enFlat.get(key).includes("{{")) problems.push(`${path}: метка [[ui:${key}]] — у ключа есть параметры {{…}}, метка для него невозможна`);
    }
    for (const l of ch.links) {
      const target = parsed[`${code}/${l.chapter}.md`];
      if (!target) problems.push(`${path}: ссылка на несуществующую главу ${l.chapter}.md`);
      else if (l.anchor && !target.anchors.includes(l.anchor)) problems.push(`${path}: ссылка ${l.chapter}.md#${l.anchor} — в главе нет якоря #${l.anchor}`);
    }
  }
  return problems;
}

function walk(dir, exts, out = []) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, exts, out);
    else if (exts.some((e) => p.endsWith(e))) out.push(p);
  }
  return out;
}

/** Аргументы `--rust-cyrillic`: файлы как есть, каталоги — все `.rs` внутри (рекурсивно). */
export function expandRustPaths(paths) {
  return paths.flatMap((p) => (statSync(p).isDirectory() ? walk(p, [".rs"]) : [p]));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = process.cwd();
  const locales = join(root, "src/locales");
  const dicts = Object.fromEntries(
    readdirSync(locales)
      .filter((f) => f.endsWith(".json"))
      .map((f) => [f.replace(/\.json$/, ""), JSON.parse(readFileSync(join(locales, f), "utf8"))]),
  );
  const problems = checkDictionaries(dicts);
  const allow = JSON.parse(readFileSync(join(root, "docs/i18n/same-as-en.json"), "utf8"));
  problems.push(...checkSameAsEn(dicts, allow), ...checkTypography(dicts));
  const codes = languageCodes(readFileSync(join(root, "src/i18n/languages.ts"), "utf8"));
  problems.push(...checkLanguageFiles(codes, Object.keys(dicts), { requireAll: process.argv.includes("--require-all-languages") }));
  if (process.argv.includes("--lengths")) for (const line of lengthReport(dicts)) console.log(`↔ ${line}`);
  const rustFiles = Object.fromEntries(
    walk(join(root, "src-tauri/src"), [".rs"]).map((p) => [relative(root, p), readFileSync(p, "utf8")]),
  );
  const rust = rustKeys(rustFiles);
  problems.push(...rust.problems, ...checkRustKeys(rust.calls, flatten(dicts.en)));
  const srcFiles = Object.fromEntries(
    walk(join(root, "src"), [".ts", ".tsx"]).map((p) => [relative(root, p), readFileSync(p, "utf8")]),
  );
  problems.push(...checkFrontendKeys(frontendKeys(srcFiles), flatten(dicts.en)));
  problems.push(...checkHints(hintKeys(srcFiles), flatten(dicts.en)));
  const guideDir = join(root, "docs/guide");
  const guideFiles = Object.fromEntries(walk(guideDir, [".md"]).map((p) => [relative(guideDir, p).split(sep).join("/"), readFileSync(p, "utf8")]));
  const langFlat = Object.fromEntries(Object.entries(dicts).map(([c, d]) => [c, flatten(d)]));
  problems.push(...checkGuide(guideFiles, codes, flatten(dicts.en), GUIDE_CHAPTERS, langFlat));
  if (!process.argv.includes("--no-cyrillic-scan")) {
    const files = srcFiles;
    for (const hit of findCyrillic(files)) problems.push(`кириллица в коде: ${hit}`);
  }
  const rcIdx = process.argv.indexOf("--rust-cyrillic");
  if (rcIdx >= 0) {
    const paths = process.argv.slice(rcIdx + 1).filter((a) => !a.startsWith("--"));
    const expanded = expandRustPaths(paths);
    const files = Object.fromEntries(expanded.map((p) => [p, readFileSync(p, "utf8")]));
    for (const hit of findRustCyrillic(files)) problems.push(`кириллица в Rust: ${hit}`);
  }
  if (problems.length) {
    console.error(problems.map((p) => `✗ ${p}`).join("\n"));
    process.exit(1);
  }
  console.log(`✓ словари ${Object.keys(dicts).join(", ")} согласованы, вызовов из Rust: ${rust.calls.length}`);
}
