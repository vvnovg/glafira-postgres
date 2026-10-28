// Контраст и различимость цветов темы: чистые функции для theme-contrast.test.mjs (`pnpm test:scripts`).
// Разбирает переменные --bg и --wait-* из src/styles.css; считает WCAG-контраст и ΔE в OKLab (формулы CSS Color 4).

/** Переменные, которые разбирает `parseThemeVars`: фон темы и цвета классов ожиданий. */
const isThemeVar = (name) => name === "--bg" || name.startsWith("--wait-");

/** Убирает комментарии `/* … *\/`: в них бывают примеры объявлений, которые не должны попасть в разбор. */
const stripComments = (css) => css.replace(/\/\*[\s\S]*?\*\//g, "");

/** Верхний уровень текста как список правил `{ selector, body }`; тело вложенных блоков остаётся строкой (скобки сбалансированы). */
function splitRules(text) {
  const rules = [];
  let depth = 0;
  let selectorStart = 0;
  let bodyStart = 0;
  for (let i = 0; i < text.length; i++) {
    if (text[i] === "{") {
      if (depth === 0) {
        bodyStart = i + 1;
      }
      depth++;
    } else if (text[i] === "}") {
      depth--;
      if (depth === 0) {
        rules.push({ selector: text.slice(selectorStart, bodyStart - 1).trim().replace(/\s+/g, " "), body: text.slice(bodyStart, i) });
        selectorStart = i + 1;
      }
    }
  }
  return rules;
}

/** Объявления `--имя: #rrggbb` из тела правила → Map «имя → hex в нижнем регистре»; значение не `#rrggbb` — ошибка. */
function themeDeclarations(body, into) {
  for (const m of body.matchAll(/(--[\w-]+)\s*:\s*([^;]*?)\s*(?:;|$)/g)) {
    const [, name, value] = m;
    if (!isThemeVar(name)) continue;
    if (!/^#[0-9a-f]{6}$/i.test(value)) {
      throw new Error(`${name}: значение «${value}» — не #rrggbb, контраст не посчитать`);
    }
    into.set(name, value.toLowerCase());
  }
}

/**
 * Цвета темы из текста styles.css: `:root` верхнего уровня — светлая тема, `:root` внутри
 * `@media (prefers-color-scheme: dark)` — тёмная (только то, что она переопределяет).
 * @returns {{ light: Map<string, string>, dark: Map<string, string> }}
 */
export function parseThemeVars(css) {
  const light = new Map();
  const dark = new Map();
  for (const rule of splitRules(stripComments(css))) {
    if (rule.selector === ":root") {
      themeDeclarations(rule.body, light);
    } else if (/^@media\s*\(\s*prefers-color-scheme\s*:\s*dark\s*\)$/.test(rule.selector)) {
      for (const inner of splitRules(rule.body)) {
        if (inner.selector === ":root") themeDeclarations(inner.body, dark);
      }
    }
  }
  return { light, dark };
}

/** `#rrggbb` → компоненты sRGB 0…1. */
function srgb(hex) {
  if (!/^#[0-9a-f]{6}$/i.test(hex)) throw new Error(`«${hex}» — не #rrggbb`);
  return [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
}

/** Снятие гаммы sRGB: компонент 0…1 → линейный. */
const toLinear = (c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);

/** Относительная яркость WCAG 2.x. */
function luminance(hex) {
  const [r, g, b] = srgb(hex).map(toLinear);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** Контраст двух цветов по WCAG 2.x: от 1 до 21 (порядок аргументов не важен). */
export function contrastRatio(hexA, hexB) {
  const [a, b] = [luminance(hexA), luminance(hexB)];
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

// Матрицы CSS Color 4: линейный sRGB → XYZ (D65) → LMS → OKLab.
const SRGB_TO_XYZ = [
  [506752 / 1228815, 87881 / 245763, 12673 / 70218],
  [87098 / 409605, 175762 / 245763, 12673 / 175545],
  [7918 / 409605, 87881 / 737289, 1001167 / 1053270],
];
const XYZ_TO_LMS = [
  [0.819022437996703, 0.3619062600528904, -0.1288737815209879],
  [0.0329836539323885, 0.9292868615863434, 0.0361446663506424],
  [0.0481771893596242, 0.2642395317527308, 0.6335478284694309],
];
const LMS_TO_OKLAB = [
  [0.210454268309314, 0.7936177747023054, -0.0040720430116193],
  [1.9779985324311684, -2.42859224204858, 0.450593709617411],
  [0.0259040424655478, 0.7827717124575296, -0.8086757549230774],
];
const mul = (m, v) => m.map((row) => row[0] * v[0] + row[1] * v[1] + row[2] * v[2]);

/** `#rrggbb` → [L, a, b] в OKLab (L от 0 до 1). */
export function oklab(hex) {
  const lms = mul(XYZ_TO_LMS, mul(SRGB_TO_XYZ, srgb(hex).map(toLinear)));
  return mul(LMS_TO_OKLAB, lms.map(Math.cbrt));
}

/**
 * Расстояние между цветами: евклидово в OKLab, умноженное на 100 (шкала навыка dataviz: порог «8» — это 0,08 в единицах OKLab).
 * Одинаковые цвета дают 0, чёрный и белый — 100.
 */
export function deltaEOk(hexA, hexB) {
  const [a, b] = [oklab(hexA), oklab(hexB)];
  return 100 * Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
}
