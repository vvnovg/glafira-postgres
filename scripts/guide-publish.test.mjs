import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { publishChapter, publishGuide } from "./guide-publish.mjs";
import { GUIDE_CHAPTERS } from "./i18n-check.mjs";

test("метки → **значение** с экранированием Markdown", () => {
  const labels = new Map([
    ["nav.help", "Справка"],
    ["a.b", "Save *all* [x] <y> #1 | ~ ! _ ` \\"],
  ]);
  const out = publishChapter("# Глава\n\nНажмите [[ui:nav.help]] и [[ui:a.b]].\n", labels);
  assert.ok(out.includes("Нажмите **Справка** и **Save \\*all\\* \\[x\\] \\<y\\> \\#1 \\| \\~ \\! \\_ \\` \\\\**."), out);
  assert.ok(!out.includes("[[ui:"), out);
});

test("точка после метки, значение которой уже кончается на «.» или «:», не дублируется", () => {
  const labels = new Map([
    ["dot", "Read-only."],
    ["colon", "Logs:"],
    ["plain", "Help"],
    ["ell", "Import…"],
  ]);
  const out = publishChapter("# T\n\nA [[ui:dot]]. B [[ui:colon]]. C [[ui:plain]]. D [[ui:ell]]. E [[ui:dot]], [[ui:dot]].x\n## [[ui:dot]]. {#h}\n", labels);
  assert.ok(out.includes("A **Read-only.** B **Logs:** C **Help**. D **Import…**. E **Read-only.**, **Read-only.**x"), out);
  assert.ok(out.includes("## Read-only.\n"), out);
});

test("метка вплотную к иероглифам — **значение** без пробелов вокруг", () => {
  const labels = new Map([["sidebar.settings", "设置"]]);
  const out = publishChapter("# 标题\n\n文本[[ui:sidebar.settings]]文本\n", labels);
  assert.ok(out.includes("\n文本**设置**文本\n"), out);
});

test("набор экранируемых символов совпадает с src/guide/parse.ts", () => {
  const literal = (text) => /const SPECIAL = (\/.*\/g);/.exec(text)?.[1];
  const parse = literal(readFileSync(new URL("../src/guide/parse.ts", import.meta.url), "utf8"));
  const publish = literal(readFileSync(new URL("./guide-publish.mjs", import.meta.url), "utf8"));
  assert.ok(parse, "SPECIAL не найден в parse.ts");
  assert.equal(publish, parse);
});

test("неизвестная метка — ошибка", () => {
  assert.throws(() => publishChapter("# T\n\n[[ui:nope]]\n", new Map()), /nope/);
});

test("метки в заголовках — значение без жирного; в блоке кода — как есть", () => {
  const labels = new Map([["k", "Producer"]]);
  const out = publishChapter("# [[ui:k]]\n\n## Вкладка [[ui:k]] {#tab}\n\n```\n[[ui:k]]\n## не раздел {#x}\n```\n", labels);
  assert.ok(out.startsWith("# Producer\n"), out);
  assert.ok(out.includes('<a id="tab"></a>\n\n## Вкладка Producer\n'), out);
  assert.ok(out.includes("```\n[[ui:k]]\n## не раздел {#x}\n```"), out);
});

test("якоря {#id} → HTML-якорь перед заголовком; ссылки на главы без изменений", () => {
  const out = publishChapter("# T\n\nтекст\n## Раздел {#my-anchor}\n\nСм. [здесь](browse.md#filters) и [сайт](https://example.com).\n", new Map());
  assert.ok(out.includes('текст\n<a id="my-anchor"></a>\n\n## Раздел\n'), out);
  assert.ok(!out.includes("{#"), out);
  assert.ok(out.includes("[здесь](browse.md#filters)"), out);
  assert.ok(out.includes("[сайт](https://example.com)"), out);
});

test("publishGuide: главы всех языков и README со всеми языками", () => {
  const root = mkdtempSync(join(tmpdir(), "guide-pub-"));
  const out = join(root, "out");
  try {
    const langs = [
      ["en", "English"],
      ["ru", "Русский"],
      ["pt-BR", "Português (Brasil)"],
    ];
    mkdirSync(join(root, "src/i18n"), { recursive: true });
    writeFileSync(
      join(root, "src/i18n/languages.ts"),
      `export const LANGUAGES = [\n${langs.map(([c, n]) => `  { code: "${c}", nativeName: "${n}" },`).join("\n")}\n];\n`,
    );
    mkdirSync(join(root, "src/locales"), { recursive: true });
    for (const [c] of langs) {
      writeFileSync(join(root, "src/locales", `${c}.json`), JSON.stringify({ nav: { help: `Help-${c}` } }));
      mkdirSync(join(root, "docs/guide", c), { recursive: true });
      for (const ch of ["getting-started", "browse"]) {
        writeFileSync(join(root, "docs/guide", c, `${ch}.md`), `# ${ch} ${c}\n\n## Раздел {#sec}\n\n[[ui:nav.help]] — [ссылка](browse.md#sec)\n`);
      }
    }
    publishGuide(root, out, ["getting-started", "browse"]);
    for (const [c] of langs) {
      const files = readdirSync(join(out, c)).sort();
      assert.deepEqual(files, ["browse.md", "getting-started.md"]);
      const text = readFileSync(join(out, c, "getting-started.md"), "utf8");
      assert.ok(text.includes(`**Help-${c}**`), text);
      assert.ok(text.includes('<a id="sec"></a>'), text);
      for (const f of files) {
        const t = readFileSync(join(out, c, f), "utf8");
        assert.ok(!t.includes("[[ui:") && !t.includes("{#"), `${c}/${f}: ${t}`);
      }
    }
    const readme = readFileSync(join(out, "README.md"), "utf8");
    for (const [c, n] of langs) {
      assert.ok(readme.includes(`[${n}](${c}/getting-started.md)`), readme);
      assert.ok(readme.includes(`(${c}/browse.md)`), readme);
    }
    assert.ok(readme.includes("browse en"), readme);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("publishGuide на реальном репозитории: главы без меток и {#", () => {
  const out = mkdtempSync(join(tmpdir(), "guide-real-"));
  try {
    publishGuide(process.cwd(), out);
    const readme = readFileSync(join(out, "README.md"), "utf8");
    assert.ok(readme.startsWith("# Glafira for PostgreSQL — User guide"), readme);
    for (const n of ["English", "Русский"]) {
      assert.ok(readme.includes(n), n);
    }
    for (const c of ["en", "ru"]) {
      const files = readdirSync(join(out, c));
      assert.equal(files.length, GUIDE_CHAPTERS.length, c);
      for (const f of files) {
        const t = readFileSync(join(out, c, f), "utf8");
        assert.ok(!t.includes("[[ui:") && !t.includes("{#"), `${c}/${f}`);
      }
    }
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});
