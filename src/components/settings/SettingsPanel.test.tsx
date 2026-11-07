import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LanguageSettings } from "../../api/types";
import { i18n } from "../../i18n";
import { openContextHelp } from "../../lib/useHelpShortcut";
import { useHelpContext } from "../../state/helpContext";
import { useWorkspace } from "../../state/workspace";
import { agentSettings, MCP_CONFIGS, mcpStatus } from "../../test/mcp";
import { renderWithClient } from "../../test/render";

const base: LanguageSettings = {
  system: "ru-RU", systemMatch: "ru", chosen: null, policy: null, effective: "ru", reason: "system",
  languages: ["en", "ru"],
};

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return {
    ...orig,
    api: {
      getLanguageSettings: vi.fn(),
      setLanguage: vi.fn(),
      getUiSettings: vi.fn(),
      setUiSettings: vi.fn(),
      getMcpStatus: vi.fn(),
      setMcpEnabled: vi.fn(),
      revokeMcpClient: vi.fn(),
      disconnectMcpSessions: vi.fn(),
      getMcpAudit: vi.fn(),
      getMcpClientConfigs: vi.fn(),
      revealMcpAudit: vi.fn(),
      listConnections: vi.fn(),
      getAgentSettings: vi.fn(),
      setAgentSettings: vi.fn(),
      detectAgentCli: vi.fn(),
      listAgentModels: vi.fn(),
      checkAgent: vi.fn(),
      unloadAgentModel: vi.fn(),
      listAgentRuns: vi.fn(),
      stopAgentRun: vi.fn(),
      readAgentLog: vi.fn(),
      revealAgentLog: vi.fn(),
      getAppInfo: vi.fn(),
    },
  };
});

import { api } from "../../api/client";
import { SettingsPanel } from "./SettingsPanel";

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus());
  vi.mocked(api.getMcpClientConfigs).mockResolvedValue(MCP_CONFIGS);
  vi.mocked(api.getMcpAudit).mockResolvedValue([]);
  vi.mocked(api.listConnections).mockResolvedValue([]);
  useWorkspace.setState({ tabs: [], activeId: null });
  vi.mocked(api.getUiSettings).mockResolvedValue({ showSystemObjects: false });
  vi.mocked(api.getAgentSettings).mockResolvedValue(agentSettings());
  vi.mocked(api.listAgentModels).mockResolvedValue({ ids: [], default: null, source: "none" });
  vi.mocked(api.listAgentRuns).mockResolvedValue([]);
  vi.mocked(api.getAppInfo).mockResolvedValue({ name: "Glafira for PostgreSQL", version: "0.6.0", commit: "abc1234", os: "macos", arch: "aarch64", dataDir: "/d", logDir: "/l" });
});

// Выбор «en» в тесте переключает глобальный язык; сброс идёт в act, пока компоненты ещё
// смонтированы (useTranslation обновляет их по languageChanged).
afterEach(() => act(() => i18n.changeLanguage("ru")));

/** Навигация по разделам и её пункты (кнопки) по подписи. */
const nav = () => screen.getByRole("navigation", { name: "Разделы настроек" });
const navItem = (name: string) => within(nav()).getByRole("button", { name });
/** Заголовки видимых разделов (скрытые разделы в дереве доступности не видны). */
const shownHeadings = () => screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
const spec = () => useWorkspace.getState().tabs.find((x) => x.id === "settings");

describe("SettingsPanel", () => {
  it("навигация: «Язык», «Отображение», группа «LLM-ассистенты (MCP)» с четырьмя пунктами; без раздела в спеке — «Язык», и показан только он", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    renderWithClient(<SettingsPanel />);
    await within(nav()).findByRole("button", { name: "Фоновый ассистент" });
    expect(within(nav()).getAllByRole("button").map((b) => b.textContent)).toEqual([
      "Язык",
      "Отображение",
      "Сервер и подключение клиентов",
      "Фоновый ассистент",
      "Сессии и клиенты",
      "Журнал",
    ]);
    const mcpGroup = within(nav()).getByRole("list", { name: "LLM-ассистенты (MCP)" });
    expect(within(mcpGroup).getAllByRole("button").map((b) => b.textContent)).toEqual(["Сервер и подключение клиентов", "Фоновый ассистент", "Сессии и клиенты", "Журнал"]);
    expect(navItem("Язык")).toHaveAttribute("aria-current", "page");
    for (const name of ["Отображение", "Сервер и подключение клиентов", "Фоновый ассистент", "Сессии и клиенты", "Журнал"]) {
      expect(navItem(name)).not.toHaveAttribute("aria-current");
    }
    expect(await screen.findByLabelText("Язык интерфейса")).toBeVisible();
    expect(shownHeadings()).toEqual(["Язык"]);
    expect(screen.getByRole("region", { name: "Язык" })).toBeVisible();
    expect(screen.queryByRole("checkbox", { name: "Разрешить MCP-сервер" })).toBeNull();
    expect(screen.queryByRole("checkbox", { name: /Показывать системные объекты/ })).toBeNull();
  });

  it("выбор пункта показывает только его раздел (заголовок — как пункт) и запоминается в спеке вкладки", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    useWorkspace.setState({ tabs: [{ kind: "settings", id: "settings" }], activeId: "settings" });
    const user = userEvent.setup();
    renderWithClient(<SettingsPanel />);
    await user.click(navItem("Сервер и подключение клиентов"));
    expect(spec()).toMatchObject({ kind: "settings", section: "mcp" });
    expect(navItem("Сервер и подключение клиентов")).toHaveAttribute("aria-current", "page");
    expect(navItem("Язык")).not.toHaveAttribute("aria-current");
    expect(await screen.findByRole("checkbox", { name: "Разрешить MCP-сервер" })).toBeVisible();
    expect(shownHeadings()).toEqual(["Сервер и подключение клиентов"]);
    expect(screen.queryByLabelText("Язык интерфейса")).not.toBeVisible();

    await user.click(navItem("Отображение"));
    expect(spec()).toMatchObject({ section: "display" });
    expect(screen.getByRole("checkbox", { name: /Показывать системные объекты/ })).toBeVisible();
    expect(shownHeadings()).toEqual(["Отображение"]);

    await user.click(navItem("Фоновый ассистент"));
    expect(spec()).toMatchObject({ section: "agent" });
    const agent = screen.getByRole("region", { name: "Фоновый ассистент" });
    expect(await within(agent).findByRole("checkbox", { name: "Запускать ассистента в фоне" })).toBeVisible();
    expect(within(agent).getByRole("heading", { level: 4, name: "Запуски" })).toBeInTheDocument();
    expect(screen.queryByRole("checkbox", { name: "Разрешить MCP-сервер" })).toBeNull();

    await user.click(navItem("Сессии и клиенты"));
    expect(spec()).toMatchObject({ section: "sessions" });
    expect(shownHeadings()).toEqual(["Сессии и клиенты"]);
    expect(screen.getAllByRole("heading", { level: 4 }).map((h) => h.textContent)).toEqual(["Активные сессии", "Подтверждённые клиенты"]);

    await user.click(navItem("Журнал"));
    expect(spec()).toMatchObject({ section: "audit" });
    expect(shownHeadings()).toEqual(["Журнал"]);
    expect(await screen.findByText("Записей нет")).toBeVisible();
    expect(screen.getByRole("button", { name: "Показать файл" })).toBeVisible();
    // Фокус остаётся на пункте навигации.
    expect(navItem("Журнал")).toHaveFocus();
  });

  it("индикатор LLM: `section: \"mcp\"` открывает «Сервер и подключение клиентов», `\"agent\"` — «Фоновый ассистент»", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    useWorkspace.setState({ tabs: [{ kind: "settings", section: "agent", id: "settings" }], activeId: "settings" });
    renderWithClient(<SettingsPanel />);
    const agent = await screen.findByRole("region", { name: "Фоновый ассистент" });
    expect(agent).toBeVisible();
    expect(await within(agent).findByRole("checkbox", { name: "Запускать ассистента в фоне" })).toBeVisible();
    expect(navItem("Фоновый ассистент")).toHaveAttribute("aria-current", "page");
    act(() => useWorkspace.getState().open({ kind: "settings", section: "mcp" }));
    expect(await screen.findByRole("checkbox", { name: "Разрешить MCP-сервер" })).toBeVisible();
    expect(navItem("Сервер и подключение клиентов")).toHaveAttribute("aria-current", "page");
    expect(shownHeadings()).toEqual(["Сервер и подключение клиентов"]);
  });

  it("бэкенд без фонового ассистента: пункта «Фоновый ассистент» нет, `section: \"agent\"` открывает раздел сервера", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    const { agent: _agent, ...old } = mcpStatus();
    vi.mocked(api.getMcpStatus).mockResolvedValue(old as never);
    useWorkspace.setState({ tabs: [{ kind: "settings", section: "agent", id: "settings" }], activeId: "settings" });
    renderWithClient(<SettingsPanel />);
    expect(await screen.findByRole("checkbox", { name: "Разрешить MCP-сервер" })).toBeVisible();
    expect(within(nav()).getAllByRole("button").map((b) => b.textContent)).toEqual(["Язык", "Отображение", "Сервер и подключение клиентов", "Сессии и клиенты", "Журнал"]);
    expect(navItem("Сервер и подключение клиентов")).toHaveAttribute("aria-current", "page");
    expect(screen.queryByRole("region", { name: "Фоновый ассистент" })).toBeNull();
    expect(api.getAgentSettings).not.toHaveBeenCalled();
  });

  it("F1 — по выбранному разделу: «Язык» и «Отображение» — «Начало работы», разделы MCP — глава «LLM-ассистенты (MCP)» и её раздел", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    useWorkspace.setState({ tabs: [{ kind: "settings", id: "settings" }], activeId: "settings" });
    const user = userEvent.setup();
    renderWithClient(<SettingsPanel />);
    const help = () => [useHelpContext.getState().byTab.settings, useHelpContext.getState().anchorByTab.settings];
    await waitFor(() => expect(help()).toEqual(["getting-started", undefined]));
    const cases: [string, string, string | undefined][] = [
      ["Сервер и подключение клиентов", "llm-mcp", "enable"],
      ["Фоновый ассистент", "llm-mcp", "background"],
      ["Сессии и клиенты", "llm-mcp", "sessions-log"],
      ["Журнал", "llm-mcp", "sessions-log"],
      ["Отображение", "getting-started", undefined],
      ["Язык", "getting-started", undefined],
    ];
    for (const [item, chapter, anchor] of cases) {
      await user.click(await within(nav()).findByRole("button", { name: item }));
      expect(help(), item).toEqual([chapter, anchor]);
    }
    // F1 открывает справку на главе и разделе выбранного раздела.
    await user.click(navItem("Фоновый ассистент"));
    act(() => openContextHelp());
    expect(useWorkspace.getState().tabs.find((x) => x.id === "help")).toMatchObject({ kind: "help", chapter: "llm-mcp", anchor: "background" });
  });

  it("F1 после открытия индикатором — раздел, который он открыл", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    useWorkspace.setState({ tabs: [{ kind: "settings", section: "mcp", id: "settings" }], activeId: "settings" });
    renderWithClient(<SettingsPanel />);
    await waitFor(() => expect(useHelpContext.getState().byTab.settings).toBe("llm-mcp"));
    expect(useHelpContext.getState().anchorByTab.settings).toBe("enable");
    act(() => useWorkspace.getState().open({ kind: "settings", section: "agent" }));
    await waitFor(() => expect(useHelpContext.getState().anchorByTab.settings).toBe("background"));
  });

  it("открытие индикатором (новый объект спеки) возвращает заголовок раздела в видимую часть, каждый раз", async () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll;
    try {
      vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
      useWorkspace.setState({ tabs: [{ kind: "settings", id: "settings" }], activeId: "settings" });
      renderWithClient(<SettingsPanel />);
      await screen.findByLabelText("Язык интерфейса");
      scroll.mockClear();
      act(() => useWorkspace.getState().open({ kind: "settings", section: "agent" }));
      await waitFor(() => expect(scroll).toHaveBeenCalledTimes(1));
      expect(scroll.mock.contexts[0]).toBe(screen.getByRole("heading", { level: 3, name: "Фоновый ассистент" }));
      expect(scroll).toHaveBeenLastCalledWith({ block: "nearest" });
      act(() => useWorkspace.getState().open({ kind: "settings", section: "agent" }));
      await waitFor(() => expect(scroll).toHaveBeenCalledTimes(2));
    } finally {
      delete (Element.prototype as { scrollIntoView?: unknown }).scrollIntoView;
    }
  });

  it("выбор клиента в «Подключить клиент» сохраняется, пока вкладка открыта: переход в другой раздел его не сбрасывает", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    useWorkspace.setState({ tabs: [{ kind: "settings", section: "mcp", id: "settings" }], activeId: "settings" });
    const user = userEvent.setup();
    renderWithClient(<SettingsPanel />);
    await user.click(await screen.findByRole("radio", { name: "Cursor" }));
    expect(screen.getByText("Cursor: файл ~/.cursor/mcp.json")).toBeVisible();
    await user.click(navItem("Журнал"));
    await user.click(navItem("Сервер и подключение клиентов"));
    expect(screen.getByRole("radio", { name: "Cursor" })).toBeChecked();
    expect(screen.getByText("Cursor: файл ~/.cursor/mcp.json")).toBeVisible();
    expect(screen.queryByText("Qwen Code: команда")).toBeNull();
  });

  it("язык не прочитался — ошибка с «Повторить» в разделе «Язык», навигация и другие разделы работают", async () => {
    vi.mocked(api.getLanguageSettings).mockRejectedValueOnce({ code: "INTERNAL", message: "settings.json: broken", retryable: false }).mockResolvedValue(base);
    const user = userEvent.setup();
    renderWithClient(<SettingsPanel />);
    expect(await screen.findByRole("alert")).toHaveTextContent("settings.json: broken");
    await user.click(navItem("Сервер и подключение клиентов"));
    expect(await screen.findByRole("checkbox", { name: "Разрешить MCP-сервер" })).toBeVisible();
    await user.click(navItem("Язык"));
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(await screen.findByLabelText("Язык интерфейса")).toBeVisible();
  });

  it("выбор языка; доступны все языки", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    vi.mocked(api.setLanguage).mockResolvedValue({ ...base, chosen: "en", effective: "en", reason: "user" });
    const user = userEvent.setup();
    renderWithClient(<SettingsPanel />);
    const select = await screen.findByLabelText("Язык интерфейса");
    for (const name of ["English", "Русский"]) {
      const option = screen.getByRole("option", { name });
      expect(option).toBeEnabled();
    }
    expect(screen.getByRole("option", { name: /Как в системе/ })).toBeInTheDocument();
    await user.selectOptions(select, "en");
    expect(api.setLanguage).toHaveBeenCalledWith("en");
    // Выбранный язык применяется к интерфейсу (onSuccess вызывает setLanguage(st.effective)).
    await vi.waitFor(() => expect(i18n.language).toBe("en"));
  });

  it("язык задан организацией", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue({ ...base, policy: "ru", reason: "policy" });
    renderWithClient(<SettingsPanel />);
    expect(await screen.findByLabelText("Язык интерфейса")).toBeDisabled();
    expect(screen.getByText(/задан вашей организацией/)).toBeInTheDocument();
  });

  it("«Показывать системные объекты» сохраняется и отражается в флажке", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    useWorkspace.setState({ tabs: [{ kind: "settings", section: "display", id: "settings" }], activeId: "settings" });
    vi.mocked(api.setUiSettings).mockResolvedValue({ showSystemObjects: true });
    const user = userEvent.setup();
    renderWithClient(<SettingsPanel />);
    const box = await screen.findByRole("checkbox", { name: /Показывать системные объекты/ });
    await vi.waitFor(() => expect(box).toBeEnabled());
    expect(box).not.toBeChecked();
    await user.click(box);
    expect(api.setUiSettings).toHaveBeenCalledWith({ showSystemObjects: true });
    await vi.waitFor(() => expect(box).toBeChecked());
  });

  it("ошибка сохранения настройки показывается, флажок остаётся прежним", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    useWorkspace.setState({ tabs: [{ kind: "settings", section: "display", id: "settings" }], activeId: "settings" });
    vi.mocked(api.setUiSettings).mockRejectedValue({ code: "INTERNAL", message: "disk full", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<SettingsPanel />);
    const box = await screen.findByRole("checkbox", { name: /Показывать системные объекты/ });
    await vi.waitFor(() => expect(box).toBeEnabled());
    await user.click(box);
    expect(await screen.findByRole("alert")).toHaveTextContent("disk full");
    expect(box).not.toBeChecked();
  });
});
