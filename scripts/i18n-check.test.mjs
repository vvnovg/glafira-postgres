import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CJK_TYPO_LANGS, GUIDE_CHAPTERS, HAN_SPACE_LANGS, cjkProblems, visualWidth, checkChangelogs, checkGuide, parseGuideChapter, checkDictionaries, checkFrontendKeys, checkHints, hintKeys, checkLanguageFiles, checkSameAsEn, checkTypography, languageCodes, lengthReport, tags, checkRustKeys, frontendKeys, expandRustPaths, findCyrillic, findRustCyrillic, flatten, placeholders, rustKeys } from "./i18n-check.mjs";

test("flatten и placeholders", () => {
  assert.deepEqual([...flatten({ a: { b: "x", c: { d: "y" } } }).keys()], ["a.b", "a.c.d"]);
  assert.deepEqual(placeholders("Подключение «{{name}}» ({{ count }})").sort(), ["count", "name"]);
});

test("паритет ключей и параметров", () => {
  const en = { a: "Hello {{name}}", b: "x", items_one: "{{count}} item", items_other: "{{count}} items" };
  const ru = { a: "Привет, {{name}}", b: "х", items_one: "{{count}} элемент", items_few: "{{count}} элемента", items_many: "{{count}} элементов", items_other: "{{count}} элемента" };
  assert.deepEqual(checkDictionaries({ en, ru }), []);
  const broken = checkDictionaries({ en, ru: { ...ru, a: "Привет" } });
  assert.ok(broken.some((p) => p.includes("ru") && p.includes("a") && p.includes("name")), broken.join("\n"));
  const missing = checkDictionaries({ en, ru: { b: "х", items_one: "1", items_few: "2", items_many: "5", items_other: "x" } });
  assert.ok(missing.some((p) => p.includes("нет ключа a")), missing.join("\n"));
});

test("плюральные формы считаются по правилам языка", () => {
  const en = { n_one: "{{count}} x", n_other: "{{count}} xs" };
  // ru без _few/_many — ошибка
  const p = checkDictionaries({ en, ru: { n_one: "{{count}} х", n_other: "{{count}} х" } });
  assert.ok(p.some((x) => x.includes("ru") && x.includes("n_few")), p.join("\n"));
});

test("кириллица в коде вне словарей и тестов", () => {
  const files = {
    "src/a.tsx": "const x = 1;\n<button>Сохранить</button>\n",
    "src/b.tsx": "// комментарий по-русски допустим\nconst y = 2;\n",
    "src/a.test.tsx": "expect('Сохранить')\n",
    "src/i18n/languages.ts": "{ nativeName: \"Русский\" }\n",
  };
  assert.deepEqual(findCyrillic(files), ["src/a.tsx:2"]);
});

const keysOf = (r) => r.calls.map((c) => c.key).sort();

test("ключи из Rust", () => {
  const rust = rustKeys({
    "a.rs": 'app_err!(ErrorCode::ReadOnly, "errors.guard.readOnly", "Connection “{{name}}” is read-only", name = n)',
    "b.rs": 'let t = ltext!("messages.x", "Hi");',
  });
  assert.deepEqual(rust.problems, []);
  assert.deepEqual(keysOf(rust), ["errors.guard.readOnly", "messages.x"]);
  const en = flatten({ errors: { guard: { readOnly: "Connection “{{name}}” is read-only" } }, messages: { x: "Hello" } });
  const p = checkRustKeys(rust.calls, en);
  assert.deepEqual(p.length, 1);
  assert.ok(p[0].includes("messages.x") && p[0].includes("b.rs:1"), p[0]);
});

test("ключи из Rust: многострочный вызов после rustfmt и экранированные кавычки", () => {
  const rust = rustKeys({
    "c.rs": 'fn f() {\n    Err(app_err!(\n            ErrorCode::ConfirmationRequired,\n            "errors.guard.typeToConfirm",\n            "Type \\"{{expected}}\\" to confirm",\n            expected = expected,\n        ))\n}',
  });
  assert.deepEqual(rust.problems, []);
  assert.deepEqual(rust.calls, [
    { key: "errors.guard.typeToConfirm", template: 'Type "{{expected}}" to confirm', params: ["expected"], where: "c.rs:2" },
  ]);
  assert.deepEqual(checkRustKeys(rust.calls, new Map()), ["en: нет ключа errors.guard.typeToConfirm (используется в Rust, c.rs:2)"]);
});

test("ключи из Rust: комментарии, модуль тестов и тела macro_rules не считаются", () => {
  const rust = rustKeys({
    "d.rs": [
      '/// `ltext!("messages.doc", "Doc")` → LocalizedText',
      "macro_rules! app_err {",
      "    ($code:expr, $key:literal, $en:literal) => { $crate::ltext!($key, $en) };",
      "}",
      'let a = ltext!("messages.real", "Real");',
      "#[cfg(test)]",
      "mod tests {",
      '    let t = ltext!("messages.test.only", "Test");',
      "}",
    ].join("\n"),
  });
  assert.deepEqual(rust.problems, []);
  assert.deepEqual(keysOf(rust), ["messages.real"]);
});

test("ключи из Rust: escape-последовательности и перенос строки через \\", () => {
  const rust = rustKeys({
    "e.rs": 'ltext!("m.a", "Line 1\\nLine 2 \\\\ back \\u{2014} dash \\\n        continued");',
  });
  assert.deepEqual(rust.problems, []);
  assert.equal(rust.calls[0].template, "Line 1\nLine 2 \\ back — dash continued");
});

test("ключи из Rust: макрос, который не разобрать, — ошибка с файлом и строкой", () => {
  const rust = rustKeys({
    "f.rs": [
      'let ok = ltext!("m.ok", "Ok");',
      "let a = ltext!(KEY, \"Text\");",
      'let b = app_err!(ErrorCode::Internal, "m.b", format!("x {}", y));',
      'let c = ltext!("m.c", r"raw");',
      'let d = ltext!("m.d", "Bad \\q escape");',
      'let e = ltext!["m.e", "brackets"];',
      'let f = ltext!("m.f", "Text", name);',
      'let g = ltext!("m.g", "unclosed"',
    ].join("\n"),
  });
  assert.deepEqual(keysOf(rust), ["m.ok"]);
  const lines = rust.problems.map((p) => p.match(/^f\.rs:(\d+): макрос (ltext|app_err)!, который проверка не может разобрать/)?.[1]);
  assert.deepEqual(lines, ["2", "3", "4", "5", "6", "7", "8"], rust.problems.join("\n"));
  assert.ok(rust.problems[2].includes("обычный строковый литерал"), rust.problems[2]);
  assert.ok(rust.problems[3].includes("\\q"), rust.problems[3]);
});

test("параметры вызова совпадают с {{…}} шаблона в обе стороны", () => {
  const rust = rustKeys({
    "g.rs": [
      'ltext!("m.ok", "Hi {{name}} and {{ count }}", count = n, name = x)',
      'ltext!("m.missing", "Hi {{name}} and {{count}}", name = x)',
      'ltext!("m.extra", "Hi {{name}}", name = x, count = n)',
      'ltext!("m.dup", "Hi {{name}}", name = x, name = y)',
    ].join("\n"),
  });
  assert.deepEqual(rust.problems, []);
  const en = new Map([
    ["m.ok", "Hi {{name}} and {{ count }}"],
    ["m.missing", "Hi {{name}} and {{count}}"],
    ["m.extra", "Hi {{name}}"],
    ["m.dup", "Hi {{name}}"],
  ]);
  const p = checkRustKeys(rust.calls, en);
  assert.equal(p.length, 3, p.join("\n"));
  assert.ok(p.some((x) => x.startsWith("g.rs:2: m.missing") && x.includes("нет параметра count")), p.join("\n"));
  assert.ok(p.some((x) => x.startsWith("g.rs:3: m.extra") && x.includes("лишний параметр count")), p.join("\n"));
  assert.ok(p.some((x) => x.startsWith("g.rs:4: m.dup") && x.includes("дважды")), p.join("\n"));
});

test("ключи из Rust: плюральный ключ без суффикса сверяется с формой _other", () => {
  const rust = rustKeys({
    "p.rs": 'ltext!("w.items", "{{count}} items", count = n);\nltext!("w.items", "{{count}} item", count = n);\nltext!("w.items", "{{count}} items", count = n, extra = x);',
  });
  assert.deepEqual(rust.problems, []);
  const en = new Map([
    ["w.items_one", "{{count}} item"],
    ["w.items_other", "{{count}} items"],
  ]);
  const p = checkRustKeys(rust.calls, en);
  assert.equal(p.length, 2, p.join("\n"));
  assert.ok(p.some((x) => x.includes("w.items_other отличается") && x.includes("p.rs:2")), p.join("\n"));
  assert.ok(p.some((x) => x.startsWith("p.rs:3: w.items") && x.includes("лишний параметр extra")), p.join("\n"));
  const noCount = checkRustKeys([{ key: "w.items", template: "items", params: [], where: "q.rs:1" }], new Map([["w.items_other", "items"]]));
  assert.deepEqual(noCount, ["q.rs:1: w.items — плюральный ключ без параметра count"]);
});

test("кириллица в литералах Rust: код — находка, комментарии и #[cfg(test)] — нет", () => {
  const src = [
    "//! Модуль: описание по-русски",
    "/// Документация «в кавычках»",
    'fn a() -> String { "Ошибка".into() } // комментарий "в кавычках"',
    'fn b() -> &\'static str { "ok" /* "блок" */ }',
    'fn c() { let s = r#"сырой"#; let ch = \'ё\'; }',
    "fn d<'a>(x: &'a str) -> &'a str { x }",
    'tracing::warn!("лог {x}");',
    "#[cfg(test)]",
    'pub fn helper() -> &\'static str { "тестовый помощник" }',
    "#[cfg(test)]",
    "mod tests {",
    '    #[test] fn t() { assert_eq!(x, "по-русски {"); }',
    "}",
    'const AFTER: &str = "после тестов";',
  ].join("\n");
  assert.deepEqual(findRustCyrillic({ "x.rs": src }), ["x.rs:3", "x.rs:5", "x.rs:7", "x.rs:14"]);
  assert.deepEqual(findRustCyrillic({ "y.rs": 'let s = "multi\nстрока";' }), ["y.rs:1"]);
});

test("--rust-cyrillic: каталог раскрывается во все .rs рекурсивно, файл — как есть", () => {
  const dir = mkdtempSync(join(tmpdir(), "i18n-check-"));
  try {
    mkdirSync(join(dir, "sub"));
    writeFileSync(join(dir, "a.rs"), "");
    writeFileSync(join(dir, "sub", "b.rs"), "");
    writeFileSync(join(dir, "sub", "c.txt"), "");
    const single = join(dir, "sub", "c.txt");
    assert.deepEqual(expandRustPaths([dir]).sort(), [join(dir, "a.rs"), join(dir, "sub", "b.rs")]);
    assert.deepEqual(expandRustPaths([single]), [single]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("ключи фронтенда: статические проверяются, динамические и тесты пропускаются", () => {
  const files = {
    "src/a.tsx": [
      'const a = t("ok.key"); const b = i18n.t("typo.key", { x: 1 });',
      "const c = t(`dyn.${name}`); const d = tt('plural.stem');",
      '{ labelKey: "tabs.missing" }, <Trans i18nKey="trans.key" />',
      'const e = t("concat." + x);',
    ].join("\n"),
    "src/a.test.tsx": 't("only.in.test")',
  };
  const keys = frontendKeys(files);
  assert.deepEqual(
    keys.map((k) => k.key),
    ["ok.key", "typo.key", "plural.stem", "tabs.missing", "trans.key"],
  );
  const en = new Map([["ok.key", "x"], ["plural.stem_one", "a"], ["plural.stem_other", "b"], ["trans.key", "t"]]);
  const p = checkFrontendKeys(keys, en);
  assert.deepEqual(p.map((x) => x.split(" ")[3]), ["typo.key", "tabs.missing"]);
  assert.ok(p[0].includes("src/a.tsx:1"), p[0]);
});

test("теги совпадают с en", () => {
  assert.deepEqual(tags("a <code>{{x}}</code> <offset/> <file>"), ["</code>", "<code>", "<file>", "<offset/>"]);
  const en = { a: "Type <code>{{name}}</code>", n_one: "<total/> {{count}}", n_other: "<total/> {{count}}s" };
  const ok = { a: "Tippen <code>{{name}}</code>", n_one: "<total/> {{count}}", n_other: "<total/> {{count}}" };
  assert.deepEqual(checkDictionaries({ en, de: ok }), []);
  const p = checkDictionaries({ en, de: { ...ok, a: "Tippen {{name}}", n_other: "{{count}}" } });
  assert.ok(p.some((x) => x.includes("de: a") && x.includes("<code>")), p.join("\n"));
  assert.ok(p.some((x) => x.includes("de: n_other") && x.includes("<total/>")), p.join("\n"));
});

test("строка совпадает с en без разрешения — ошибка", () => {
  const en = { a: "Save", b: "Kafka", c: "{{message}}", n_one: "{{count}} topic", n_other: "{{count}} topics" };
  const de = { a: "Save", b: "Kafka", c: "{{message}}", n_one: "{{count}} Topic", n_other: "{{count}} topics" };
  const p = checkSameAsEn({ en, de }, { de: ["b"] });
  assert.ok(p.some((x) => x.includes("de: a ")), p.join("\n"));
  assert.ok(!p.some((x) => x.includes("de: b ")), "b разрешён");
  assert.ok(!p.some((x) => x.includes("de: c ")), "без букв вне параметров — не ошибка");
  assert.ok(p.some((x) => x.includes("de: n_other")), "плюральная форма сравнивается с en той же формы или _other");
  const stale = checkSameAsEn({ en, de: { ...de, a: "Speichern", n_other: "{{count}} Topics" } }, { de: ["a", "b"] });
  assert.deepEqual(stale, ["de: a в docs/i18n/same-as-en.json, но перевод отличается от en — уберите из списка"]);
});

test("типографика: многоточие и пробелы во французском", () => {
  const p = checkTypography({
    en: { a: "Loading...", b: "Loading…" },
    fr: { a: "Chargement…", b: "Supprimer ?", c: "Nom : x", d: "« x »", e: "Supprimer ?", f: "« x »", g: "localhost:9092" },
  });
  assert.deepEqual(p.sort(), [
    "en: a — «...» вместо «…»",
    "fr: b — обычный пробел вместо U+00A0 перед «?»",
    "fr: c — обычный пробел вместо U+00A0 перед «:»",
    "fr: d — обычный пробел вместо U+00A0 у кавычек « »",
  ]);
});

test("языки из languages.ts и файлы словарей", () => {
  const ts = 'export const LANGUAGES = [\n  { code: "en", nativeName: "English" },\n  { code: "pt-BR", nativeName: "Português (Brasil)" },\n];';
  assert.deepEqual(languageCodes(ts), ["en", "pt-BR"]);
  assert.deepEqual(checkLanguageFiles(["en", "ru", "de"], ["en", "ru"], { requireAll: false }), []);
  assert.deepEqual(checkLanguageFiles(["en", "ru", "de"], ["en", "ru"], { requireAll: true }), ["нет словаря src/locales/de.json"]);
  assert.deepEqual(checkLanguageFiles(["en", "ru"], ["en", "ru", "xx"], { requireAll: false }), ["словарь src/locales/xx.json — языка xx нет в src/i18n/languages.ts"]);
});

test("отчёт о длине: только заметно длиннее en", () => {
  const r = lengthReport({ en: { a: "Delete topic", b: "OK", c: "Save" }, de: { a: "Das Topic endgültig löschen", b: "Einverstanden", c: "Speich" } });
  assert.deepEqual(r, ["de: a — 27 / 12 (×2.3) «Das Topic endgültig löschen»"]);
});

test("пустые и не строковые значения — ошибка", () => {
  const en = { a: "Save", b: "Close" };
  for (const bad of ["", "   ", null, 5, true, []]) {
    const p = checkDictionaries({ en, de: { a: "Speichern", b: bad } });
    assert.ok(p.some((x) => x.includes("de: b") && x.includes("пустое или не строковое")), `${JSON.stringify(bad)}: ${p.join("\n")}`);
  }
  assert.ok(checkDictionaries({ en: { a: "Save", b: "" }, de: { a: "Speichern", b: "Zu" } }).some((x) => x.includes("en: b")));
  assert.deepEqual(checkDictionaries({ en, de: { a: "Speichern", b: "Schließen" } }), []);
});

test("{{count}} нужен во всех формах, кроме _one в языках, где one — ровно число 1", () => {
  const en = { n_one: "{{count}} x", n_other: "{{count}} xs" };
  // de: _one без count допустим, _other без count — ошибка
  assert.deepEqual(checkDictionaries({ en, de: { n_one: "Ein Topic", n_other: "{{count}} Topics" } }), []);
  const bad = checkDictionaries({ en, de: { n_one: "Ein Topic", n_other: "Topics" } });
  assert.ok(bad.some((x) => x.includes("de: n_other") && x.includes("{{count}}")), bad.join("\n"));
  // ru: _few/_many без count — ошибка; _one в ru покрывает 21, 31… — тоже нужен count
  const ru = { n_one: "{{count}} х", n_few: "{{count}} х", n_many: "{{count}} х", n_other: "{{count}} х" };
  assert.deepEqual(checkDictionaries({ en, ru }), []);
  const ruBad = checkDictionaries({ en, ru: { ...ru, n_few: "х", n_one: "Один" } });
  assert.ok(ruBad.some((x) => x.includes("ru: n_few")), ruBad.join("\n"));
  assert.ok(ruBad.some((x) => x.includes("ru: n_one")), ruBad.join("\n"));
  // fr и pt-BR: one выбирается и для 0 — count обязателен в _one
  for (const code of ["fr", "pt-BR"]) {
    const p = checkDictionaries({ en, [code]: { n_one: "Un", n_many: "{{count}} x", n_other: "{{count}} x" } });
    assert.ok(p.some((x) => x.includes(`${code}: n_one`) && x.includes("{{count}}")), p.join("\n"));
  }
});

test("теги: парные не перепутаны, а одиночные могут менять порядок", () => {
  const en = { a: "<code>{{x}}</code> <total/>" };
  assert.deepEqual(checkDictionaries({ en, de: { a: "<total/> <code>{{x}}</code>" } }), []);
  const p = checkDictionaries({ en, de: { a: "</code>{{x}}<code> <total/>" } });
  assert.ok(p.some((x) => x.includes("de: a") && x.includes("парн")), p.join("\n"));
  const nested = checkDictionaries({ en: { a: "<b>x</b>" }, de: { a: "<b>x</b></b><b>" } });
  assert.ok(nested.some((x) => x.includes("парн")) || nested.some((x) => x.includes("теги")), nested.join("\n"));
});

test("allow-list: ключ, которого нет в en, и неизвестный язык — ошибка", () => {
  const en = { a: "Kafka", n_one: "{{count}} x", n_other: "{{count}} xs" };
  const de = { a: "Kafka", n_one: "{{count}} x", n_other: "{{count}} xs" };
  assert.deepEqual(checkSameAsEn({ en, de }, { de: ["a", "n_one", "n_other"] }), []);
  const p = checkSameAsEn({ en, de }, { de: ["a", "n_one", "n_other", "gone"], pt_BR: ["a"] });
  assert.ok(p.some((x) => x.includes("de: gone") && x.includes("нет в en")), p.join("\n"));
  assert.ok(p.some((x) => x.includes("pt_BR") && x.includes("нет словаря")), p.join("\n"));
});

test("лишняя плюральная форма и голый ключ рядом с плюральными", () => {
  const en = { n_one: "{{count}} x", n_other: "{{count}} xs" };
  const p = checkDictionaries({ en, de: { n_one: "{{count}} x", n_other: "{{count}} x", n_few: "{{count}} x" } });
  assert.ok(p.some((x) => x.includes("de: n_few") && x.includes("лишняя форма")), p.join("\n"));
  const bare = checkDictionaries({ en, de: { n: "x", n_one: "{{count}} x", n_other: "{{count}} x" } });
  assert.ok(bare.some((x) => x.includes("de: n ") || x.includes("de: лишний ключ n")), bare.join("\n"));
});

test("подсказки: ключи есть, все используются, длина en", () => {
  const files = {
    "src/a.tsx": '<Hint k="hints.a.x" label="X" />\nconst M = { s: "hints.a.y" };',
    "src/a.test.tsx": '<Hint k="hints.zzz" label="Z" />',
  };
  const uses = hintKeys(files);
  assert.deepEqual(uses.map((u) => u.key), ["hints.a.x", "hints.a.y"]);
  assert.equal(uses[0].where, "src/a.tsx:1");
  assert.equal(uses[1].where, "src/a.tsx:2");
  const en = flatten({ hints: { label: "Hint: {{name}}", a: { x: "Short <code>acks</code>", y: "y".repeat(201), z: "unused" } } });
  const p = checkHints([...uses, { key: "hints.missing", where: "src/b.tsx:3" }], en);
  assert.deepEqual(p.sort(), [
    "en: hints.a.y — 201 символов, больше 200",
    "en: hints.a.z не используется ни в одном <Hint> — удалите или подключите",
    "en: нет ключа hints.missing (подсказка, src/b.tsx:3)",
  ].sort());
  assert.deepEqual(checkHints([], flatten({ hints: { label: "Hint: {{name}}" } })), [], "hints.label не требует использования в <Hint>");
});

test("руководство продукта: список глав", () => {
  assert.deepEqual(GUIDE_CHAPTERS, ["getting-started", "connections", "objects", "diagnostics", "updates"]);
});

test("руководство: структура, метки, ссылки", () => {
  const en = flatten({ sidebar: { settings: "Settings", conn: "Connection {{name}}" } });
  const ok = "# Start\n\nOpen [[ui:sidebar.settings]].\n\n## Install {#install}\n\nSee [mTLS](connections.md#mtls).\n";
  const conn = "# Connections\n\n## mTLS {#mtls}\n\nText.\n";
  const ru = (t) => t.replace("# ", "# ru ");
  const files = { "en/getting-started.md": ok, "en/connections.md": conn, "ru/getting-started.md": ru(ok), "ru/connections.md": ru(conn) };
  const p = (f) => checkGuide(f, ["en", "ru"], en, ["getting-started", "connections"]);
  assert.deepEqual(p(files), []);
  assert.ok(p({ ...files, "ru/connections.md": "# Подключения\n\n## mTLS {#m-tls}\n" }).some((x) => x.includes("ru/connections.md") && x.includes("mtls")));
  assert.ok(p({ ...files, "en/getting-started.md": ok.replace("sidebar.settings", "sidebar.nope") }).some((x) => x.includes("sidebar.nope")));
  assert.ok(p({ ...files, "en/getting-started.md": ok.replace("sidebar.settings", "sidebar.conn") }).some((x) => x.includes("sidebar.conn")));
  assert.ok(p({ ...files, "en/getting-started.md": ok.replace("#mtls", "#nope") }).some((x) => x.includes("#nope")));
  assert.ok(p({ "en/getting-started.md": ok, "en/connections.md": conn, "ru/getting-started.md": ru(ok) }).some((x) => x.includes("ru/connections.md")));
  assert.ok(p({ ...files, "en/connections.md": "No title\n" }).some((x) => x.includes("# ")));
  assert.ok(p({ ...files, "en/connections.md": conn + "<b>x</b>\n" }).some((x) => x.includes("HTML")));
  assert.ok(p({ ...files, "en/connections.md": conn + "[x](http://a.b)\n" }).some((x) => x.includes("http://")));
});

test("руководство: якорь {#id} только у ##, у каждого ## и без повторов в главе", () => {
  const pr = (text) => parseGuideChapter(text).problems;
  assert.deepEqual(pr("# T\n\n## A {#a}\n\n### Sub\n\nText `{#code}`.\n\n```\n## not a section\n### x {#x}\n```\n"), []);
  assert.ok(pr("# T\n\n## Без якоря\n").some((x) => x.includes("## Без якоря") && x.includes("{#id}")), pr("# T\n\n## Без якоря\n").join("\n"));
  const h3 = pr("# T\n\n## A {#a}\n\n### B {#b}\n");
  assert.ok(h3.some((x) => x.includes("### B {#b}") && x.includes("только у ##")), h3.join("\n"));
  const prose = pr("# T\n\n## A {#a}\n\nText {#b} here.\n");
  assert.ok(prose.some((x) => x.includes("{#") && x.includes("только у ##")), prose.join("\n"));
  const dup = pr("# T\n\n## A {#a}\n\n## B {#a}\n");
  assert.ok(dup.some((x) => x.includes("{#a}") && x.includes("повтор")), dup.join("\n"));
  assert.deepEqual(parseGuideChapter("# T\n\n## A {#a}\n\n### B {#b}\n").anchors, ["a"]);
});

test("CJK: полуширинная пунктуация после иероглифа и пробел между иероглифами", () => {
  assert.deepEqual(CJK_TYPO_LANGS, ["zh-CN", "zh-TW", "ja"]);
  assert.deepEqual(HAN_SPACE_LANGS, ["zh-CN", "zh-TW"]);
  assert.ok(cjkProblems("设置,保存", "zh-CN").some((x) => x.includes("полуширинная пунктуация после иероглифа")));
  for (const ch of [",", ".", ":", ";", "?", "!"]) assert.equal(cjkProblems(`设置${ch}`, "zh-TW").length, 1, ch);
  assert.deepEqual(cjkProblems("设置，保存", "zh-CN"), []);
  assert.deepEqual(cjkProblems("连接 Kafka 集群", "zh-CN"), []);
  assert.deepEqual(cjkProblems("已加载 3 条消息。", "zh-CN"), []);
  assert.ok(cjkProblems("连接 集群", "zh-CN").some((x) => x.includes("пробел между иероглифами")));
  assert.ok(cjkProblems("連線　叢集", "zh-TW").some((x) => x.includes("пробел между иероглифами")), "идеографический пробел U+3000");
  assert.deepEqual(cjkProblems("接続 クラスター", "ja"), [], "в ja пробел между словами — не эта проверка");
  assert.ok(cjkProblems("トピック.", "ja").some((x) => x.includes("полуширинная")));
  assert.ok(cjkProblems("ひらがな!", "ja").length > 0);
  assert.ok(cjkProblems("サーバー.", "ja").some((x) => x.includes("полуширинная")), "ー (U+30FC, Script=Common) — часть катаканы");
  assert.ok(cjkProblems("フォルダー,", "ja").some((x) => x.includes("полуширинная")));
  assert.deepEqual(cjkProblems("토픽.", "ko"), [], "ko — ASCII-пунктуация");
  assert.deepEqual(cjkProblems("连接 集群.", "de"), [], "для других языков проверки нет");
  assert.deepEqual(cjkProblems("运行 `a,b` 命令", "zh-CN"), [], "внутри `code` — не проверяется");
  assert.deepEqual(cjkProblems("运行`主题.名`命令", "zh-CN"), [], "внутри `code` — не проверяется");
  assert.deepEqual(cjkProblems("主题 {{name}} 已删除", "zh-CN"), [], "{{…}} не склеивает соседние иероглифы");
  assert.deepEqual(cjkProblems("主题{{名,x}}已删除", "zh-CN"), [], "внутри {{…}} — не проверяется");
  assert.deepEqual(cjkProblems("主题 <b>集群</b> 已删除", "zh-CN"), [], "теги не склеивают соседние иероглифы");
  assert.deepEqual(cjkProblems("<1>设置</1>。", "zh-CN"), []);
});

test("CJK: видимая ширина", () => {
  assert.equal(visualWidth("设置"), 4);
  assert.equal(visualWidth("ab"), 2);
  assert.equal(visualWidth("トピックab"), 10);
  assert.equal(visualWidth("ひら"), 4);
  assert.equal(visualWidth("토픽"), 4);
  assert.equal(visualWidth("（，）"), 6);
  assert.equal(visualWidth("Ünïcode"), 7);
  assert.equal(visualWidth("サーバー"), 8, "ー — полная ширина");
  assert.equal(visualWidth("！｠￠￦"), 8, "полноширинные формы");
  assert.equal(visualWidth("ｱｲ"), 2, "полуширинная катакана — 1");
  assert.equal(visualWidth("ﾡﾢ"), 2, "полуширинный хангыль — 1");
  assert.equal(visualWidth("｡"), 1, "полуширинная точка — 1");
});

test("CJK: типографика словарей и длина по видимой ширине", () => {
  const p = checkTypography({ en: { a: "Save, close" }, "zh-CN": { a: "保存,关闭", b: "连接 Kafka 集群" }, ko: { a: "저장, 닫기" } });
  assert.equal(p.length, 1, p.join("\n"));
  assert.ok(p[0].startsWith("zh-CN: a — ") && p[0].includes("полуширинная"), p[0]);
  const r = lengthReport({ en: { a: "Delete topic", b: "Save all" }, ja: { a: "トピックを完全に削除します", b: "保存" } });
  assert.deepEqual(r, ["ja: a — 26 / 12 (×2.2) «トピックを完全に削除します»"]);
});

test("CJK: проза глав руководства, вне кода и ссылок", () => {
  const en = flatten({ sidebar: { settings: "Settings" } });
  const enCh = "# Start\n\n## Install {#install}\n\nOpen [[ui:sidebar.settings]].\n";
  const zh = (body) => ({ "en/getting-started.md": enCh, "zh-CN/getting-started.md": `# 开始\n\n## 安装 {#install}\n\n${body}\n` });
  const p = (f) => checkGuide(f, ["en", "zh-CN"], en, ["getting-started"]);
  assert.deepEqual(p(zh("打开[[ui:sidebar.settings]]。参见 [文档](https://kafka.apache.org/documentation/#主题.配置)。")), []);
  assert.deepEqual(p(zh("运行：\n\n```\n主题.名 集群,x\n```\n\n命令 `主题.名` 完成。")), []);
  assert.deepEqual(p(zh("文档：https://kafka.apache.org/文档.配置 和 http://例子.cn/a,b 。")), [], "голые URL не проверяются");
  const bad = p(zh("打开[[ui:sidebar.settings]]。然后.再 保存。"));
  assert.ok(bad.some((x) => x.startsWith("zh-CN/getting-started.md: ") && x.includes("полуширинная")), bad.join("\n"));
  assert.ok(bad.some((x) => x.includes("пробел между иероглифами")), bad.join("\n"));
  assert.deepEqual(p({ "en/getting-started.md": enCh.replace("Open", "打开 集群.") , "zh-CN/getting-started.md": enCh }), [], "в en CJK-проверок нет");
});

test("CJK: метка, значение которой начинается или кончается пунктуацией, вплотную к иероглифу — жирное не сработает", () => {
  const en = flatten({ a: { exp: "Export…", set: "Settings", q: "(Advanced)" } });
  const zhFlat = flatten({ a: { exp: "导出…", set: "设置", q: "（高级）" } });
  const enCh = "# Start\n\nText.\n";
  const p = (body) =>
    checkGuide({ "en/getting-started.md": enCh, "zh-CN/getting-started.md": `# 开始\n\n${body}\n` }, ["en", "zh-CN"], en, ["getting-started"], { "zh-CN": zhFlat });
  assert.deepEqual(p("点击[[ui:a.set]]按钮。打开[[ui:a.exp]]。点击“[[ui:a.exp]]”按钮。[[ui:a.q]]"), []);
  assert.deepEqual(p("打开[[ui:a.exp]].然后"), [], "точка после метки — пунктуация, не иероглиф");
  assert.ok(!p("### 点击[[ui:a.exp]]按钮").some((x) => x.includes("**…**")), "в заголовке метка без жирного");
  const bad = p("点击[[ui:a.exp]]按钮，选择[[ui:a.q]]。");
  assert.equal(bad.length, 2, bad.join("\n"));
  assert.ok(bad.every((x) => x.startsWith("zh-CN/getting-started.md: ") && x.includes("**…**")), bad.join("\n"));
  assert.ok(bad[0].includes("[[ui:a.exp]]") && bad[1].includes("[[ui:a.q]]"), bad.join("\n"));
});

test("CJK: точка после метки со значением на «.»/«:» убирается рендером — смотрим на следующий символ", () => {
  const en = flatten({ a: { ro: "Read-only.", logs: "Logs:" } });
  const zhFlat = flatten({ a: { ro: "只读.", logs: "日志：" } });
  const flat = new Map([["a.ro", "(Read-only)."], ["a.logs", "Logs:"]]);
  const enCh = "# Start\n\nText.\n";
  const p = (code, body, lf) =>
    checkGuide({ "en/getting-started.md": enCh, [`${code}/getting-started.md`]: `# T\n\n${body}\n` }, ["en", code], en, ["getting-started"], { [code]: lf });
  assert.deepEqual(p("de", "Siehe [[ui:a.ro]]. Weiter", flat), [], "после убранной точки — пробел");
  assert.ok(p("de", "Siehe [[ui:a.ro]].Weiter", flat).some((x) => x.includes("**…**")), "после убранной точки — буква");
  assert.ok(p("zh-CN", "见[[ui:a.ro]].然后", zhFlat).some((x) => x.includes("**…**")));
});

test("CJK: в прозе главы метка подставляется значением языка — полуширинная точка после неё находится", () => {
  const en = flatten({ sidebar: { settings: "Settings" } });
  const enCh = "# Start\n\nOpen [[ui:sidebar.settings]].\n";
  const zhFlat = flatten({ sidebar: { settings: "设置" } });
  const p = (body) =>
    checkGuide({ "en/getting-started.md": enCh, "zh-CN/getting-started.md": `# 开始\n\n${body}\n` }, ["en", "zh-CN"], en, ["getting-started"], { "zh-CN": zhFlat });
  assert.ok(p("打开[[ui:sidebar.settings]].").some((x) => x.includes("полуширинная") && x.includes("置.")), p("打开[[ui:sidebar.settings]].").join("\n"));
  assert.deepEqual(p("打开[[ui:sidebar.settings]]。"), []);
  assert.ok(p("打开 [[ui:sidebar.settings]]。").some((x) => x.includes("пробел между иероглифами")), "в zh метка без пробелов");
});

test("глава, совпадающая с en байт в байт, — не переведена", () => {
  const en = flatten({ sidebar: { settings: "Settings" } });
  const enCh = "# Start\n\nOpen [[ui:sidebar.settings]].\n";
  const zhFlat = flatten({ sidebar: { settings: "设置" } });
  const p = (zh) => checkGuide({ "en/getting-started.md": enCh, "zh-CN/getting-started.md": zh }, ["en", "zh-CN"], en, ["getting-started"], { "zh-CN": zhFlat });
  assert.ok(p(enCh).some((x) => x.includes("zh-CN/getting-started.md") && x.includes("глава не переведена (совпадает с en)")));
  assert.ok(!p("# 开始\n\n打开[[ui:sidebar.settings]]。\n").some((x) => x.includes("не переведена")));
  assert.ok(p(enCh.replace("Open", "Öffnen")).some((x) => x.includes("полуширинная")), "изменённая глава проверяется типографикой");
});

test("списки изменений: файл у каждого языка, лишний файл", () => {
  const files = { en: "## 1.0.0\n- one\n", ru: "## 1.0.0\n- один\n", de: null };
  assert.deepEqual(checkChangelogs(files, ["xx.md"]), [
    "docs/changelog/xx.md: лишний файл, такого языка нет в src/i18n/languages.ts",
    "docs/changelog/de.md: нет файла",
  ]);
});

test("списки изменений: версия не из CHANGELOG.md и непереведённый раздел", () => {
  const files = { en: "## 1.0.0\n- one\n\n## 0.9.0\n- old\n", ru: "## 1.0.1\n- опечатка\n\n## 1.0.0\n- один\n\n## 0.9.0\n- old\n" };
  assert.deepEqual(checkChangelogs(files), [
    "docs/changelog/ru.md: раздел «## 1.0.1» — такой версии нет в CHANGELOG.md",
    "docs/changelog/ru.md: раздел «## 0.9.0» не переведён (совпадает с CHANGELOG.md)",
  ]);
});

test("списки изменений: типографика zh/ja как в руководстве, код в обратных кавычках не проверяется", () => {
  const files = { en: "## 1.0.0\n- one\n", "zh-CN": "## 1.0.0\n- 修复问题.\n- 设置 `a.b`。\n", ja: "## 1.0.0\n- 修正しました。\n" };
  assert.deepEqual(checkChangelogs(files), ["docs/changelog/zh-CN.md: полуширинная пунктуация после иероглифа «题.» — нужна полноширинная"]);
});
