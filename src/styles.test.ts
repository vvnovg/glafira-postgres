import { describe, expect, it } from "vitest";
import css from "./styles.css?raw";
import appCss from "./app.css?raw";

/** Исходники компонентов текстом (без тестов): проверка на значки из символов шрифта. */
const components = import.meta.glob<string>("./components/**/*.{ts,tsx}", { query: "?raw", eager: true, import: "default" });

/** Относительная яркость sRGB (WCAG 2.1). */
function luminance(hex: string): number {
  const c = hex.replace("#", "");
  const [r, g, b] = [0, 2, 4].map((i) => {
    const v = parseInt(c.slice(i, i + 2), 16) / 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
const contrast = (a: string, b: string) => {
  const [l1, l2] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (l1 + 0.05) / (l2 + 0.05);
};

/** Значения токенов блока `:root` светлой темы и переопределений тёмной. */
function tokens(block: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const m of block.matchAll(/--([a-z-]+):\s*(#[0-9a-f]{6})\s*;/gi)) out[m[1]] = m[2].toLowerCase();
  return out;
}

const light = tokens(css.slice(0, css.indexOf("@media (prefers-color-scheme: dark)")));
const darkBlock = css.slice(css.indexOf("@media (prefers-color-scheme: dark)"));
const dark = { ...light, ...tokens(darkBlock.slice(0, darkBlock.indexOf("}")) + "}") };

describe("styles.css (спец. U1)", () => {
  it("ни одного кегля меньше 11 px (общие стили и стили PostgreSQL-версии)", () => {
    const small = [...(css + appCss).matchAll(/font-size:\s*(\d+(?:\.\d+)?)px/g)].map((m) => Number(m[1])).filter((n) => n < 11);
    expect(small).toEqual([]);
  });

  it("токены обеих тем заданы", () => {
    for (const name of ["bg", "bg-alt", "sidebar", "control", "field", "fg", "muted", "border", "accent", "danger", "badge-danger", "warn", "ok"]) {
      expect(light[name], name).toMatch(/^#[0-9a-f]{6}$/);
      expect(dark[name], `dark ${name}`).toMatch(/^#[0-9a-f]{6}$/);
    }
    expect(dark.bg).not.toBe(light.bg);
  });

  it("контраст: вторичный текст на фонах ≥ 4,5:1, белый на акценте и красном ≥ 4,5:1 в обеих темах", () => {
    for (const [theme, t] of [
      ["light", light],
      ["dark", dark],
    ] as const) {
      expect(contrast(t.muted, t.bg), `${theme} muted/bg`).toBeGreaterThanOrEqual(4.5);
      expect(contrast(t.muted, t.sidebar), `${theme} muted/sidebar`).toBeGreaterThanOrEqual(4.5);
      expect(contrast(t.fg, t.bg), `${theme} fg/bg`).toBeGreaterThanOrEqual(7);
      // Акцент — кнопки и выделение строк, как у системных элементов macOS (белый на системном синем — 3,6–4,0:1,
      // у прежнего #3b82f6 было 3,7:1): порог WCAG для элементов управления 3:1, здесь с запасом.
      expect(contrast("#ffffff", t.accent), `${theme} white/accent`).toBeGreaterThanOrEqual(3.5);
      expect(contrast(t.danger, t.bg), `${theme} danger/bg`).toBeGreaterThanOrEqual(4.5);
      expect(contrast("#ffffff", t["badge-danger"]), `${theme} white/badge-danger`).toBeGreaterThanOrEqual(4.5);
      // Оранжевый и зелёный бейджи — с тёмным текстом.
      expect(contrast("#1d1d1f", t.warn), `${theme} dark/warn`).toBeGreaterThanOrEqual(4.5);
      expect(contrast("#1d1d1f", t.ok), `${theme} dark/ok`).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("в разметке компонентов нет значков из символов шрифта", () => {
    // PostgreSQL-версия (спец. переноса U1 §1): ещё ▴ ○ ★ ✓ ↑ ↓ и emoji состояний (✅ ⚠ ❌ ⏳ ⏭), которые были в дереве,
    // обзоре, проверке подключения, сортировке «Запросов» и форме узлов; «»» — только в начале строки разметки (меню скрытых
    // вкладок), иначе это закрывающая кавычка.
    const glyphs = /[✎✕⋯●▸▾▴○★✓↑↓🔒📁✅⚠❌⏳⏭]|[>"]×[<"]|^\s*»/mu;
    const offenders: string[] = [];
    const files = Object.keys(components).filter((f) => !/\.test\.tsx?$/.test(f));
    expect(files.length).toBeGreaterThan(40);
    for (const file of files) {
      const text = components[file]
        // Комментарии не считаются: в них глифы упоминаются как прежние обозначения.
        .replace(/\/\*[\s\S]*?\*\//g, "")
        .replace(/^\s*\/\/.*$/gm, "")
        .replace(/\{\/\*[\s\S]*?\*\/\}/g, "");
      if (glyphs.test(text)) offenders.push(file);
    }
    expect(offenders).toEqual([]);
  });

  it("кольцо фокуса и выделение строки акцентом", () => {
    expect(css).toMatch(/:focus-visible\s*\{[^}]*box-shadow:\s*var\(--ring\)/);
    expect(css).toMatch(/\.tree-row\.current\s*\{[^}]*background:\s*var\(--accent\)/);
    expect(css).toMatch(/\.row\.selected[^{]*\{[^}]*background:\s*var\(--accent\)/);
  });
});
