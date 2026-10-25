export const GUIDE_CHAPTERS = ["getting-started", "connections", "objects", "diagnostics", "updates"] as const;

export type ChapterId = (typeof GUIDE_CHAPTERS)[number];

export function isChapterId(value: unknown): value is ChapterId {
  return typeof value === "string" && (GUIDE_CHAPTERS as readonly string[]).includes(value);
}

/** Главы docs/guide/<код>/<глава>.md — ленивые модули: в сборку попадают отдельными чанками и грузятся при открытии справки. */
const modules = import.meta.glob<string>("../../docs/guide/*/*.md", { query: "?raw", import: "default" });

function chapterPath(lang: string, id: ChapterId): string {
  return `../../docs/guide/${lang}/${id}.md`;
}

/** Главы языка; у языка без глав (x-pseudo, неизвестный код) — английские. */
export async function loadGuide(lang: string): Promise<Record<ChapterId, string>> {
  const code = chapterPath(lang, GUIDE_CHAPTERS[0]) in modules ? lang : "en";
  const texts = await Promise.all(
    GUIDE_CHAPTERS.map((id) => {
      const load = modules[chapterPath(code, id)];
      if (!load) throw new Error(`guide chapter missing: ${code}/${id}`);
      return load();
    }),
  );
  return Object.fromEntries(GUIDE_CHAPTERS.map((id, i) => [id, texts[i]])) as Record<ChapterId, string>;
}
