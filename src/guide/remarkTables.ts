import { gfmTableFromMarkdown } from "mdast-util-gfm-table";
import { gfmTable } from "micromark-extension-gfm-table";

/**
 * Плагин remark: таблицы GFM в справке — только они, без остального remark-gfm. Его автоссылки используют lookbehind
 * в регулярных выражениях, а старый WebKit (macOS 11–12) такой код не разбирает (scripts/check-dist-compat.mjs).
 * `this` — процессор unified, расширения разбора лежат в его `data()`.
 */
export function remarkTables(this: { data(): unknown }) {
  const data = this.data() as { micromarkExtensions?: unknown[]; fromMarkdownExtensions?: unknown[] };
  (data.micromarkExtensions ??= []).push(gfmTable());
  (data.fromMarkdownExtensions ??= []).push(gfmTableFromMarkdown());
}
