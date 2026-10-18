import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import Markdown, { type Components } from "react-markdown";
import { currentLocale } from "../../i18n";
import { type ChapterId, GUIDE_CHAPTERS, isChapterId, loadGuide } from "../../guide/chapters";
import { type GuideChapter, parseChapter, searchGuide } from "../../guide/parse";

/** Запрос на открытие справки: глава и раздел (вкладка `help` из workspace). Новый объект — новый переход. */
export interface HelpRequest {
  chapter?: string;
  anchor?: string;
}

const INTERNAL = /^([a-z-]+)\.md(?:#([a-z0-9-]+))?$/;

type Target = { chapter: ChapterId; anchor: string | null };

export function HelpPanel({ spec }: { spec: HelpRequest }) {
  const { t, i18n } = useTranslation();
  const lang = currentLocale();
  const { data: texts, isError, isPending } = useQuery({
    queryKey: ["guide", lang],
    queryFn: () => loadGuide(lang),
    staleTime: Infinity,
    // При смене языка показываем прежние главы, пока грузятся новые, — без мигания «Загрузка…».
    placeholderData: keepPreviousData,
  });

  const [target, setTarget] = useState<Target>(() => ({
    chapter: isChapterId(spec.chapter) ? spec.chapter : GUIDE_CHAPTERS[0],
    anchor: spec.anchor ?? null,
  }));
  const [query, setQuery] = useState("");
  const contentRef = useRef<HTMLDivElement>(null);

  // Повторное открытие вкладки (F1, ссылка) приносит новый spec — переходим к его главе.
  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    if (isChapterId(spec.chapter)) setTarget({ chapter: spec.chapter, anchor: spec.anchor ?? null });
  }, [spec]);

  const chapters = useMemo<GuideChapter[]>(() => {
    if (!texts) return [];
    // Неизвестная метка видна в тексте как есть — проверка i18n:check её не пропустит.
    const label = (key: string) => (i18n.exists(key) ? t(key) : `[[ui:${key}]]`);
    return GUIDE_CHAPTERS.filter((id) => texts[id] !== undefined).map((id) => parseChapter(id, texts[id], label));
    // i18n.language — перерисовка меток при смене языка.
  }, [texts, t, i18n, i18n.language]);

  const chapter = chapters.find((c) => c.id === target.chapter) ?? chapters[0];
  const hits = useMemo(() => searchGuide(chapters, query), [chapters, query]);

  useEffect(() => {
    const root = contentRef.current;
    if (!root || !chapter) return;
    const el = target.anchor ? root.querySelector(`#${CSS.escape(target.anchor)}`) : null;
    if (el) el.scrollIntoView?.({ block: "start" });
    else root.scrollTop = 0;
  }, [target, chapter]);

  const go = (id: ChapterId, anchor: string | null) => setTarget({ chapter: id, anchor });

  const components: Components = {
    a: ({ href, children }) => <GuideLink href={href} go={go}>{children}</GuideLink>,
  };

  if (isError) return <p className="empty">{t("help.loadFailed")}</p>;
  if (isPending || !chapter) return <p className="empty">{t("common.loading")}</p>;

  return (
    <div className="help-panel">
      <div className="help-side">
        <input
          type="search"
          value={query}
          placeholder={t("help.search")}
          aria-label={t("help.search")}
          onChange={(e) => setQuery(e.target.value)}
        />
        {query.trim() ? (
          hits.length === 0 ? (
            <p className="help-muted">{t("help.noResults")}</p>
          ) : (
            <ul className="help-hits">
              {hits.map((h) => {
                const ch = chapters.find((c) => c.id === h.chapter);
                const name = h.title === ch?.title ? ch.title : `${ch?.title} › ${h.title}`;
                return (
                  <li key={`${h.chapter}#${h.section ?? h.title}`}>
                    <button type="button" className="link" onClick={() => go(h.chapter, h.section)}>
                      <span className="help-hit-title">{name}</span>
                      <span className="help-muted">{h.snippet}</span>
                    </button>
                  </li>
                );
              })}
            </ul>
          )
        ) : (
          <nav aria-label={t("help.contents")}>
            <ul className="help-toc">
              {chapters.map((c) => (
                <li key={c.id}>
                  <button
                    type="button"
                    className={c.id === chapter.id ? "link active" : "link"}
                    aria-current={c.id === chapter.id ? "page" : undefined}
                    onClick={() => go(c.id, null)}
                  >
                    {c.title}
                  </button>
                  {c.id === chapter.id && (
                    <ul>
                      {c.sections
                        .filter((s) => s.id !== null)
                        .map((s) => (
                          <li key={s.id}>
                            <button type="button" className="link" onClick={() => go(c.id, s.id)}>
                              {s.title}
                            </button>
                          </li>
                        ))}
                    </ul>
                  )}
                </li>
              ))}
            </ul>
          </nav>
        )}
      </div>
      <article className="help-content" ref={contentRef}>
        <h1>{chapter.title}</h1>
        {chapter.sections.map((s, i) => (
          <section key={i}>
            {s.heading && <h2 id={s.id ?? undefined}>{s.title}</h2>}
            {/* Картинки не показываем: справка работает без сети. */}
            <Markdown skipHtml disallowedElements={["img"]} components={components}>
              {s.markdown}
            </Markdown>
          </section>
        ))}
      </article>
    </div>
  );
}

/** Ссылка в главе: «глава.md#якорь» — переход внутри справки, https — системный браузер, прочие не активны. */
function GuideLink({ href, go, children }: { href?: string; go: (id: ChapterId, anchor: string | null) => void; children: ReactNode }) {
  const internal = href ? INTERNAL.exec(href) : null;
  if (internal && isChapterId(internal[1])) {
    const id = internal[1];
    const anchor = internal[2] ?? null;
    return (
      <a
        href={href}
        onClick={(e) => {
          e.preventDefault();
          go(id, anchor);
        }}
      >
        {children}
      </a>
    );
  }
  if (href?.startsWith("https://")) {
    return (
      <a
        href={href}
        onClick={(e) => {
          e.preventDefault();
          void openUrl(href);
        }}
      >
        {children}
      </a>
    );
  }
  return <span>{children}</span>;
}
