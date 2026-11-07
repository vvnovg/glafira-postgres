import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import { LANGUAGES } from "../../i18n/languages";
import { i18n, setLanguage } from "../../i18n";
import { useMcpStatus } from "../../lib/useMcpStatus";
import { useHelpChapter } from "../../state/helpContext";
import { type SettingsSection, tabId, useWorkspace } from "../../state/workspace";
import { ErrorBanner } from "../common/ErrorBanner";
import { McpAgentSection, McpAuditSection, McpServerSection, McpSessionsSection } from "../mcp/McpSection";
import { MCP_SECTIONS, SECTION_HELP, SECTION_LABELS } from "./sections";

const SETTINGS_TAB = tabId({ kind: "settings" });

/** «Язык»: язык интерфейса; задан политикой — список неактивен с пометкой. Не прочитался — ошибка с «Повторить». */
function LanguageSettings() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const lang = useQuery({ queryKey: ["language"], queryFn: api.getLanguageSettings });
  const change = useMutation({
    mutationFn: (value: string) => api.setLanguage(value || null),
    onSuccess: (st) => {
      qc.setQueryData(["language"], st);
      void setLanguage(st.effective);
    },
  });
  if (lang.error) return <ErrorBanner error={lang.error} onRetry={() => void lang.refetch()} />;
  const s = lang.data;
  if (!s) return <p className="muted">{t("common.loading")}</p>;

  const byCode = new Map(LANGUAGES.map((l) => [l.code, l]));
  const systemName = s.systemMatch ? byCode.get(s.systemMatch as never)?.nativeName : undefined;
  const byPolicy = s.policy !== null;
  // Псевдоязык — только dev-сборка, без сохранения: выбор не уходит в бэкенд.
  const pseudoOn = import.meta.env.DEV && i18n.language === "x-pseudo";
  return (
    <div className="settings-block">
      <div className="field">
        <label htmlFor="settings-language">{t("settings.language.label")}</label>
        <select
          id="settings-language"
          value={pseudoOn ? "x-pseudo" : byPolicy ? s.policy ?? "" : s.chosen ?? ""}
          disabled={byPolicy || change.isPending}
          onChange={(e) => (e.target.value === "x-pseudo" ? void setLanguage("x-pseudo") : change.mutate(e.target.value))}
        >
          <option value="">{systemName ? t("settings.language.system", { name: systemName }) : t("settings.language.systemUnknown")}</option>
          {LANGUAGES.filter((l) => s.languages.includes(l.code)).map((l) => (
            <option key={l.code} value={l.code}>
              {l.nativeName}
            </option>
          ))}
          {import.meta.env.DEV && <option value="x-pseudo">Pseudo (dev)</option>}
        </select>
      </div>
      {byPolicy && <p className="hint">{t("settings.language.policy")}</p>}
      {change.error && <ErrorBanner error={change.error} />}
    </div>
  );
}

/** «Отображение»: системные схемы и роли pg_* в дереве. Каталог в дереве перечитывается сам — ключи запросов зависят от значения. */
function DisplaySettings() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const ui = useQuery({ queryKey: ["uiSettings"], queryFn: api.getUiSettings });
  const changeUi = useMutation({
    mutationFn: (showSystemObjects: boolean) => api.setUiSettings({ showSystemObjects }),
    onSuccess: (saved) => qc.setQueryData(["uiSettings"], saved),
  });
  return (
    <div className="settings-block">
      <label className="inline wrap">
        <input
          type="checkbox"
          checked={ui.data?.showSystemObjects ?? false}
          disabled={ui.isPending || changeUi.isPending}
          onChange={(e) => changeUi.mutate(e.target.checked)}
        />
        {t("settings.showSystemObjects")}
      </label>
      {ui.error && <ErrorBanner error={ui.error} onRetry={() => void ui.refetch()} />}
      {changeUi.error && <ErrorBanner error={changeUi.error} />}
    </div>
  );
}

/**
 * Раздел страницы: заголовок — как пункт навигации; невыбранный скрыт атрибутом `hidden`, но остаётся смонтированным —
 * черновик «Фонового ассистента», выбор клиента и прочее состояние разделов живут, пока открыта вкладка.
 */
function Section({ id, shown, children }: { id: SettingsSection; shown: boolean; children: ReactNode }) {
  const { t } = useTranslation();
  return (
    <section className="settings-section" aria-labelledby={`settings-${id}-title`} hidden={!shown}>
      <h3 id={`settings-${id}-title`}>{t(SECTION_LABELS[id])}</h3>
      {children}
    </section>
  );
}

/**
 * «Настройки» (смоук P4c, R9): слева — разделы («Язык», «Отображение», группа «LLM-ассистенты (MCP)»: сервер и подключение
 * клиентов, фоновый ассистент, сессии и клиенты, журнал), справа — только выбранный. Выбор хранится в спеке вкладки
 * (`section`, без него — «Язык»): индикатор LLM открывает «Сервер и подключение клиентов» (`mcp`), а во время фонового
 * запуска — «Фоновый ассистент» (`agent`). Пункта «Фоновый ассистент» нет, если бэкенд о нём не сообщает (статус MCP без
 * `agent`); тогда `agent` открывает раздел сервера. F1 — по выбранному разделу (`SECTION_HELP`).
 */
export function SettingsPanel() {
  const { t } = useTranslation();
  const spec = useWorkspace((st) => st.tabs.find((x) => x.id === SETTINGS_TAB));
  const open = useWorkspace((st) => st.open);
  const { status } = useMcpStatus();
  const asked = spec?.kind === "settings" ? spec.section : undefined;
  const noAgent = status !== undefined && !status.agent;
  const selected: SettingsSection = asked === undefined ? "language" : asked === "agent" && noAgent ? "mcp" : asked;
  const help = SECTION_HELP[selected];
  useHelpChapter(SETTINGS_TAB, help.chapter, help.anchor);
  // Выбор раздела и каждое открытие индикатором (новый объект спеки): заголовок раздела — в видимую часть, если страница
  // прокручена ниже него. jsdom не знает scrollIntoView — вызов необязательный.
  const contentRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    contentRef.current?.querySelector(`#settings-${selected}-title`)?.scrollIntoView?.({ block: "nearest" });
  }, [spec, selected]);

  const item = (id: SettingsSection) => (
    <li key={id}>
      <button type="button" aria-current={id === selected ? "page" : undefined} onClick={() => open({ kind: "settings", section: id })}>
        {t(SECTION_LABELS[id])}
      </button>
    </li>
  );
  // Раздел «Фоновый ассистент» есть, пока статус не сказал, что ассистента нет (до ответа — «Загрузка…» в разделе).
  const showAgent = status?.agent != null || (selected === "agent" && !noAgent);
  return (
    <div className="page">
      <div className="toolbar">
        <h2>{t("settings.title")}</h2>
      </div>
      <div className="settings-layout">
        <nav className="settings-nav" aria-label={t("settings.nav.label")}>
          <ul>
            {item("language")}
            {item("display")}
            <li>
              <span id="settings-nav-mcp" className="settings-nav-group">
                {t("settings.mcp.section")}
              </span>
              <ul aria-labelledby="settings-nav-mcp">{MCP_SECTIONS.filter((x) => x !== "agent" || status?.agent != null).map(item)}</ul>
            </li>
          </ul>
        </nav>
        <div className="settings-content" ref={contentRef}>
          <Section id="language" shown={selected === "language"}>
            <LanguageSettings />
          </Section>
          <Section id="display" shown={selected === "display"}>
            <DisplaySettings />
          </Section>
          <Section id="mcp" shown={selected === "mcp"}>
            <McpServerSection />
          </Section>
          {showAgent && (
            <Section id="agent" shown={selected === "agent"}>
              <McpAgentSection />
            </Section>
          )}
          <Section id="sessions" shown={selected === "sessions"}>
            <McpSessionsSection />
          </Section>
          <Section id="audit" shown={selected === "audit"}>
            <McpAuditSection />
          </Section>
        </div>
      </div>
    </div>
  );
}
