import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { contrastRatio, deltaEOk, mixSrgb, oklab, parseThemeVars } from "./theme-contrast.mjs";

// Порядок классов в столбце графика профиля ожиданий (WAIT_CLASS_ORDER в WaitsChart.tsx): восемь основных, затем три серых.
const MAIN = ["cpu", "lock", "lwlock", "io", "ipc", "client", "timeout", "bufferPin"];
const GREYS = ["extension", "activity", "other"];
const ALL = [...MAIN, ...GREYS];
// WCAG 1.4.11: нетекстовые элементы (столбцы, образцы легенды) — не ниже 3:1 к фону.
const MIN_CONTRAST = 3;
// WCAG 1.4.3 (AA): обычный текст — не ниже 4,5:1 к своему фону.
const MIN_TEXT_CONTRAST = 4.5;
// ΔE здесь — евклидово расстояние в OKLab, умноженное на 100 (шкала навыка dataviz: «8» значит 0,08 в единицах OKLab).
const MIN_DELTA_E = 8;
// Серый — «нейтральный», пока его цветность OKLCH (C = √(a² + b²)) не выше этого значения (хроматические цвета палитры — от 0,10).
const MAX_GREY_CHROMA = 0.04;

const css = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");
const { light, dark } = parseThemeVars(css);
// Тёмная тема переопределяет не всё: что в ней не названо, наследуется от :root (так это и работает в каскаде).
const themes = { light, dark: new Map([...light, ...dark]) };
// Доля --warn в цвете .diag-warn — из styles.css: правка смеси не пройдёт мимо проверки контраста.
const DIAG_WARN = /\.diag-warn\s*\{\s*color:\s*color-mix\(in srgb, var\(--warn\) (\d+)%, var\(--fg\)\)/.exec(css);
const DIAG_WARN_SHARE = DIAG_WARN ? Number(DIAG_WARN[1]) / 100 : NaN;

test("parseThemeVars: переменные из :root и из блока тёмной темы, остальное и комментарии игнорируются", () => {
  const sample = `
    :root {
      --bg: #FFFFFF;
      --bg-alt: #F4F5F7;
      --fg: #111111; /* --wait-fake: #000000; */
      --muted: #5f6673;
      --accent: #2563EB;
      --accent-fill: #2563eb;
      --danger-text: var(--danger);
      --wait-cpu: #2A78D6;
      --wait-bufferPin: #e34948;
      font-size: 13px;
    }
    .x { --wait-skipped: #123456; --accent: #000000; }
    @media (prefers-color-scheme: dark) {
      :root { --bg: #1b1d21; --accent: #5b9cf8; --wait-cpu: #3987e5; }
    }
    @media (min-width: 600px) {
      :root { --wait-cpu: #000001; }
    }
  `;
  const { light: l, dark: d } = parseThemeVars(sample);
  assert.deepEqual([...l], [
    ["--bg", "#ffffff"],
    ["--bg-alt", "#f4f5f7"],
    ["--fg", "#111111"],
    ["--muted", "#5f6673"],
    ["--accent", "#2563eb"],
    ["--accent-fill", "#2563eb"],
    ["--wait-cpu", "#2a78d6"],
    ["--wait-bufferPin", "#e34948"],
  ]);
  assert.deepEqual([...d], [["--bg", "#1b1d21"], ["--accent", "#5b9cf8"], ["--wait-cpu", "#3987e5"]]);
});

test("parseThemeVars: значение не #rrggbb — ошибка с именем переменной (иначе проверка контраста молча пропустила бы её)", () => {
  assert.throws(() => parseThemeVars(":root { --bg: #fff; --wait-cpu: red; }"), /--bg|--wait-cpu/);
  assert.throws(() => parseThemeVars(":root { --wait-cpu: var(--accent); }"), /--wait-cpu/);
  assert.throws(() => parseThemeVars(":root { --accent-fill: var(--accent); }"), /--accent-fill/);
  assert.throws(() => parseThemeVars(":root { --muted: #888; }"), /--muted/);
});

test("contrastRatio: формула WCAG на известных парах", () => {
  assert.equal(contrastRatio("#000000", "#ffffff"), 21);
  assert.equal(contrastRatio("#ffffff", "#000000"), 21);
  assert.equal(contrastRatio("#336699", "#336699"), 1);
  // #767676 на белом — классические 4,54:1, граница AA для текста.
  assert.ok(Math.abs(contrastRatio("#767676", "#ffffff") - 4.54) < 0.01);
});

test("mixSrgb: color-mix(in srgb, …) — доля первого цвета, компоненты с гаммой, округление до 8 бит", () => {
  assert.equal(mixSrgb("#000000", "#ffffff", 0.5), "#808080");
  assert.equal(mixSrgb("#ff0000", "#0000ff", 1), "#ff0000");
  assert.equal(mixSrgb("#ff0000", "#0000ff", 0), "#0000ff");
  // .diag-warn светлой темы: --warn 55 % с --fg.
  assert.equal(mixSrgb("#f59e0b", "#1d2129", 0.55), "#946619");
});

test("deltaEOk: ΔE OKLab (×100) на известных парах", () => {
  assert.equal(deltaEOk("#336699", "#336699"), 0);
  assert.ok(Math.abs(deltaEOk("#000000", "#ffffff") - 100) < 0.01);
  // Красный и зелёный sRGB: OKLab (0,62796; 0,22486; 0,12585) и (0,86644; −0,23389; 0,17950) из CSS Color 4 → 51,98.
  assert.ok(Math.abs(deltaEOk("#ff0000", "#00ff00") - 51.98) < 0.01);
  assert.equal(deltaEOk("#2a78d6", "#eb6834"), deltaEOk("#eb6834", "#2a78d6"));
});

for (const theme of ["light", "dark"]) {
  const vars = themes[theme];
  const bg = vars.get("--bg");
  const color = (cls) => vars.get(`--wait-${cls}`);

  test(`${theme}: заданы фон и все 11 цветов классов`, () => {
    assert.ok(bg, "нет --bg");
    for (const cls of ALL) assert.ok(color(cls), `нет --wait-${cls}`);
  });

  // Пары текста и фона из правки контраста P4b (смоук 2026-11-07): заливка «Принять» и значка данных, ссылки и полосы
  // выбора цветом акцента, приглушённый текст на панелях с фоном --bg-alt.
  test(`${theme}: акцент, его заливка, приглушённый текст и предупреждение — не ниже ${MIN_TEXT_CONTRAST}:1 (WCAG AA)`, () => {
    assert.ok(DIAG_WARN, "в styles.css нет правила .diag-warn вида color-mix(in srgb, var(--warn) N%, var(--fg))");
    const diagWarn = vars.get("--warn") && vars.get("--fg") ? mixSrgb(vars.get("--warn"), vars.get("--fg"), DIAG_WARN_SHARE) : undefined;
    const pairs = [
      ["белый на --accent-fill («Принять», значок данных, флажки)", "#ffffff", vars.get("--accent-fill")],
      ["--accent на --bg (ссылки, «Предложения LLM: N — открыть в консоли»)", vars.get("--accent"), bg],
      ["--accent на --bg-alt (ссылки боковой панели и находок)", vars.get("--accent"), vars.get("--bg-alt")],
      ["--muted на --bg-alt (приглушённый текст на панелях)", vars.get("--muted"), vars.get("--bg-alt")],
      // .diag-warn: color-mix(in srgb, var(--warn) 55%, var(--fg)) — предупреждения видов и строки статуса фонового ассистента.
      ["предупреждение .diag-warn на --bg (виды диагностики)", diagWarn, bg],
      ["предупреждение .diag-warn на --bg-alt (строка статуса фонового ассистента, пометки видов)", diagWarn, vars.get("--bg-alt")],
    ];
    for (const [name, fg, back] of pairs) assert.ok(fg && back, `нет цвета для пары: ${name}`);
    const bad = pairs
      .map(([name, fg, back]) => ({ name, fg, back, ratio: contrastRatio(fg, back) }))
      .filter((p) => p.ratio < MIN_TEXT_CONTRAST);
    assert.deepEqual(
      bad.map((p) => `${p.name}: ${p.fg} к ${p.back} — ${p.ratio.toFixed(2)}:1`),
      [],
    );
  });

  test(`${theme}: каждый --wait-* даёт контраст не ниже ${MIN_CONTRAST}:1 к --bg`, () => {
    const bad = ALL.map((cls) => ({ cls, hex: color(cls), ratio: contrastRatio(color(cls), bg) })).filter((c) => c.ratio < MIN_CONTRAST);
    assert.deepEqual(
      bad.map((c) => `--wait-${c.cls} ${c.hex} к ${bg}: ${c.ratio.toFixed(2)}:1`),
      [],
    );
  });

  test(`${theme}: соседние классы в порядке столбца различимы (ΔE OKLab не ниже ${MIN_DELTA_E})`, () => {
    const bad = [];
    for (let i = 0; i + 1 < MAIN.length; i++) {
      const [a, b] = [MAIN[i], MAIN[i + 1]];
      const de = deltaEOk(color(a), color(b));
      if (de < MIN_DELTA_E) bad.push(`${a} ${color(a)} и ${b} ${color(b)}: ΔE ${de.toFixed(1)}`);
    }
    assert.deepEqual(bad, []);
  });

  test(`${theme}: серые классы нейтральны и различимы между собой (ΔE не ниже ${MIN_DELTA_E})`, () => {
    for (const cls of GREYS) {
      const [, a, b] = oklab(color(cls));
      assert.ok(Math.hypot(a, b) <= MAX_GREY_CHROMA, `--wait-${cls} ${color(cls)} не серый: цветность ${Math.hypot(a, b).toFixed(3)}`);
    }
    const bad = [];
    for (let i = 0; i < GREYS.length; i++) {
      for (let j = i + 1; j < GREYS.length; j++) {
        const de = deltaEOk(color(GREYS[i]), color(GREYS[j]));
        if (de < MIN_DELTA_E) bad.push(`${GREYS[i]} ${color(GREYS[i])} и ${GREYS[j]} ${color(GREYS[j])}: ΔE ${de.toFixed(1)}`);
      }
    }
    assert.deepEqual(bad, []);
  });
}
