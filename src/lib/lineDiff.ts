/** Вид строки сравнения: общая, только в исходном тексте, только в новом. */
export type DiffKind = "same" | "del" | "add";

export interface DiffLine {
  kind: DiffKind;
  text: string;
}

/** Строка в колонке сравнения: `n` — номер строки в своём тексте, с 1. */
export interface DiffCell {
  kind: DiffKind;
  text: string;
  n: number;
}

/** Ряд сравнения рядом: слева исходный текст, справа новый; `null` — у стороны в этом ряду строки нет. */
export interface DiffRow {
  left: DiffCell | null;
  right: DiffCell | null;
}

/** Предел таблицы LCS (строк исходной середины × строк новой): больше — сравнение без неё; клетка — 4 байта. */
export const LCS_CELLS = 2_000_000;

/** Строки текста; CRLF и CR — как LF; пустой текст — ни одной строки. */
function lines(s: string): string[] {
  return s === "" ? [] : s.replace(/\r\n?/g, "\n").split("\n");
}

const tagged = (kind: DiffKind) => (text: string): DiffLine => ({ kind, text });

/**
 * Построчное сравнение двух текстов (наибольшая общая подпоследовательность строк), без зависимостей. Общие начало и
 * конец снимаются сразу; середина сравнивается таблицей LCS, а если таблица больше `LCS_CELLS` клеток — целиком: удалена
 * и добавлена. В блоке изменений удаления идут перед добавлениями.
 */
export function lineDiff(a: string, b: string): DiffLine[] {
  const x = lines(a);
  const y = lines(b);
  let start = 0;
  while (start < x.length && start < y.length && x[start] === y[start]) start += 1;
  let endX = x.length;
  let endY = y.length;
  while (endX > start && endY > start && x[endX - 1] === y[endY - 1]) {
    endX -= 1;
    endY -= 1;
  }
  const mx = x.slice(start, endX);
  const my = y.slice(start, endY);
  const middle = mx.length * my.length > LCS_CELLS ? [...mx.map(tagged("del")), ...my.map(tagged("add"))] : lcs(mx, my);
  return [...x.slice(0, start).map(tagged("same")), ...middle, ...x.slice(endX).map(tagged("same"))];
}

function lcs(x: string[], y: string[]): DiffLine[] {
  const n = x.length;
  const m = y.length;
  const w = m + 1;
  // len[i * w + j] — длина LCS суффиксов x[i..] и y[j..]; одна плоская таблица (n + 1) × (m + 1).
  const len = new Uint32Array((n + 1) * w);
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      len[i * w + j] = x[i] === y[j] ? len[(i + 1) * w + j + 1] + 1 : Math.max(len[(i + 1) * w + j], len[i * w + j + 1]);
    }
  }
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (x[i] === y[j]) {
      out.push({ kind: "same", text: x[i] });
      i += 1;
      j += 1;
    } else if (len[(i + 1) * w + j] >= len[i * w + j + 1]) {
      out.push({ kind: "del", text: x[i] });
      i += 1;
    } else {
      out.push({ kind: "add", text: y[j] });
      j += 1;
    }
  }
  for (; i < n; i += 1) out.push({ kind: "del", text: x[i] });
  for (; j < m; j += 1) out.push({ kind: "add", text: y[j] });
  return out;
}

/** Ряды «рядом»: общая строка — в обеих колонках; удаления и добавления одного блока — парами по порядку. */
export function sideBySide(diff: DiffLine[]): DiffRow[] {
  const rows: DiffRow[] = [];
  let left = 0;
  let right = 0;
  let dels: string[] = [];
  let adds: string[] = [];
  const flush = () => {
    for (let k = 0; k < Math.max(dels.length, adds.length); k += 1) {
      rows.push({
        left: k < dels.length ? { kind: "del", text: dels[k], n: (left += 1) } : null,
        right: k < adds.length ? { kind: "add", text: adds[k], n: (right += 1) } : null,
      });
    }
    dels = [];
    adds = [];
  };
  for (const d of diff) {
    if (d.kind === "del") dels.push(d.text);
    else if (d.kind === "add") adds.push(d.text);
    else {
      flush();
      rows.push({ left: { kind: "same", text: d.text, n: (left += 1) }, right: { kind: "same", text: d.text, n: (right += 1) } });
    }
  }
  flush();
  return rows;
}
