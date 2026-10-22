import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checkDist, checkDistDir } from "./check-dist-compat.mjs";

const POLYFILL = "Object.defineProperty(Object,`hasOwn`,{value:(e,t)=>Object.prototype.hasOwnProperty.call(e,t)})";

test("чистая сборка — без проблем", () => {
  assert.deepEqual(checkDist({ "index.js": "let a=Object.hasOwnProperty.call(b,`x`);const r=/(?<name>a)/;" }), []);
});

test("lookbehind в регулярном выражении — ошибка", () => {
  const p = checkDist({ "a.js": "const r=/(?<=a)b/;", "b.js": "new RegExp(`(?<!x)y`)" });
  assert.equal(p.length, 2, p.join("\n"));
  assert.match(p[0], /a\.js: .*lookbehind/);
  assert.match(p[1], /b\.js: .*lookbehind/);
});

test("приватные поля и статические блоки классов — ошибка", () => {
  const p = checkDist({ "a.js": "class A{#x=1;static{init()}m(){return this.#x}}" });
  assert.ok(p.some((s) => /приватн/.test(s)), p.join("\n"));
  assert.ok(p.some((s) => /static/.test(s)), p.join("\n"));
});

test("якорь {#id} в тексте главы не считается приватным полем", () => {
  assert.deepEqual(checkDist({ "ch.js": "var e=`## Раздел {#mtls}\\n\\nтекст`;" }), []);
});

test("Object.hasOwn без полифила — ошибка, с полифилом в любом чанке — нет", () => {
  const use = { "HelpPanel.js": "for(t in a)if(Object.hasOwn(a,t))x()" };
  const p = checkDist(use);
  assert.equal(p.length, 1, p.join("\n"));
  assert.match(p[0], /HelpPanel\.js: Object\.hasOwn/);
  assert.deepEqual(checkDist({ ...use, "index.js": POLYFILL }), []);
  // минификатор переименовывает параметр: installPolyfills(e=Object){…defineProperty(e,"hasOwn",…)}
  assert.deepEqual(checkDist({ ...use, "index.js": 'function n(e=Object){typeof e.hasOwn!=`function`&&Object.defineProperty(e,"hasOwn",{value:1})}' }), []);
});

test("structuredClone допустим только с проверкой typeof в том же чанке", () => {
  assert.equal(checkDist({ "a.js": "structuredClone(e)" }).length, 1);
  assert.deepEqual(checkDist({ "a.js": "x=typeof structuredClone==`function`?e=>structuredClone(e):f" }), []);
});

test("API новее Safari 14 без полифила — ошибка", () => {
  const src = [
    "a.at(-1)",
    "a.findLast(f)",
    "a.findLastIndex(f)",
    "a.toSorted()",
    "Object.groupBy(a,f)",
    "Promise.withResolvers()",
    "Array.fromAsync(i)",
    "s.isWellFormed()",
    "AbortSignal.timeout(5)",
    "crypto.randomUUID()",
    "x.union(y)",
  ];
  for (const s of src) assert.equal(checkDist({ "a.js": s }).length, 1, s);
});

test("checkDistDir читает только .js и падает на пустом каталоге", () => {
  const dir = mkdtempSync(join(tmpdir(), "dist-compat-"));
  try {
    assert.throws(() => checkDistDir(dir), /нет \.js/);
    mkdirSync(join(dir, "sub"));
    writeFileSync(join(dir, "a.js"), "const r=/(?<=a)/;");
    writeFileSync(join(dir, "a.css"), "a{b:c}/*(?<=*/");
    const p = checkDistDir(dir);
    assert.equal(p.length, 1, p.join("\n"));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
