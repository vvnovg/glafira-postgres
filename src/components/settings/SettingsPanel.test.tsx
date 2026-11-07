import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
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

describe("SettingsPanel", () => {
  it("раздел «LLM-ассистенты (MCP)» — после «Отображения»", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    renderWithClient(<SettingsPanel />);
    expect(await screen.findByRole("checkbox", { name: "Разрешить MCP-сервер" })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent)).toEqual(["Язык", "Отображение", "LLM-ассистенты (MCP)"]);
  });

  it("открытие с разделом MCP (индикатор) прокручивает к нему, каждый раз; обычное — нет", async () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll;
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    useWorkspace.setState({ tabs: [{ kind: "settings", id: "settings" }], activeId: "settings" });
    renderWithClient(<SettingsPanel />);
    await screen.findByRole("heading", { level: 3, name: "LLM-ассистенты (MCP)" });
    expect(scroll).not.toHaveBeenCalled();
    act(() => useWorkspace.getState().open({ kind: "settings", section: "mcp" }));
    await waitFor(() => expect(scroll).toHaveBeenCalledTimes(1));
    // Повторный щелчок по индикатору — новый объект спеки: прокрутка снова.
    act(() => useWorkspace.getState().open({ kind: "settings", section: "mcp" }));
    await waitFor(() => expect(scroll).toHaveBeenCalledTimes(2));
  });

  it("F1: в разделе MCP — глава «LLM-ассистенты (MCP)», в остальных — «Начало работы»", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    renderWithClient(<SettingsPanel />);
    const toggle = await screen.findByRole("checkbox", { name: "Разрешить MCP-сервер" });
    expect(useHelpContext.getState().byTab.settings).toBe("getting-started");
    act(() => toggle.focus());
    expect(useHelpContext.getState().byTab.settings).toBe("llm-mcp");
    act(() => screen.getByLabelText("Язык интерфейса").focus());
    expect(useHelpContext.getState().byTab.settings).toBe("getting-started");
  });

  it("H2: F1 после щелчка мышью — по разделу под указателем (WKWebView не переносит фокус на кнопку и флажок при щелчке)", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    renderWithClient(<SettingsPanel />);
    const mcpHeading = await screen.findByRole("heading", { level: 3, name: "LLM-ассистенты (MCP)" });
    expect(useHelpContext.getState().byTab.settings).toBe("getting-started");
    // Щелчок по тексту раздела MCP (не фокусируемому): фокус не переходит, раздел всё равно текущий.
    fireEvent.pointerDown(mcpHeading);
    expect(document.activeElement).toBe(document.body);
    expect(useHelpContext.getState().byTab.settings).toBe("llm-mcp");
    // Щелчок в разделе «Язык» — снова «Начало работы».
    fireEvent.pointerDown(screen.getByRole("heading", { level: 3, name: "Язык" }));
    expect(useHelpContext.getState().byTab.settings).toBe("getting-started");
    // И в разделе «Отображение» тоже.
    fireEvent.pointerDown(mcpHeading);
    expect(useHelpContext.getState().byTab.settings).toBe("llm-mcp");
    fireEvent.pointerDown(screen.getByRole("heading", { level: 3, name: "Отображение" }));
    expect(useHelpContext.getState().byTab.settings).toBe("getting-started");
  });

  it("F1 после открытия раздела индикатором — глава «LLM-ассистенты (MCP)»", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    useWorkspace.setState({ tabs: [{ kind: "settings", section: "mcp", id: "settings" }], activeId: "settings" });
    renderWithClient(<SettingsPanel />);
    await waitFor(() => expect(useHelpContext.getState().byTab.settings).toBe("llm-mcp"));
  });

  it("открытие с подразделом «Фоновый ассистент» (индикатор во время запуска): прокрутка к подразделу, F1 — раздел #background", async () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll;
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    useWorkspace.setState({ tabs: [{ kind: "settings", section: "agent", id: "settings" }], activeId: "settings" });
    renderWithClient(<SettingsPanel />);
    const agent = await screen.findByRole("region", { name: "Фоновый ассистент" });
    await waitFor(() => expect(scroll).toHaveBeenCalledTimes(1));
    expect(scroll.mock.contexts[0]).toBe(agent);
    expect(useHelpContext.getState().byTab.settings).toBe("llm-mcp");
    expect(useHelpContext.getState().anchorByTab.settings).toBe("background");
  });

  it("F1 в подразделе «Фоновый ассистент» — глава «LLM-ассистенты (MCP)», раздел #background; в остальном разделе — без раздела", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    renderWithClient(<SettingsPanel />);
    const agentToggle = await screen.findByRole("checkbox", { name: "Запускать ассистента в фоне" });
    act(() => agentToggle.focus());
    expect(useHelpContext.getState().byTab.settings).toBe("llm-mcp");
    expect(useHelpContext.getState().anchorByTab.settings).toBe("background");
    act(() => screen.getByRole("checkbox", { name: "Разрешить MCP-сервер" }).focus());
    expect(useHelpContext.getState().byTab.settings).toBe("llm-mcp");
    expect(useHelpContext.getState().anchorByTab.settings).toBeUndefined();
    act(() => screen.getByLabelText("Язык интерфейса").focus());
    expect(useHelpContext.getState().byTab.settings).toBe("getting-started");
    expect(useHelpContext.getState().anchorByTab.settings).toBeUndefined();
  });

  it("щелчок по тексту подраздела «Фоновый ассистент» без фокуса (WKWebView) — F1 открывает раздел #background", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    useWorkspace.setState({ tabs: [{ kind: "settings", id: "settings" }], activeId: "settings" });
    renderWithClient(<SettingsPanel />);
    const agent = await screen.findByRole("region", { name: "Фоновый ассистент" });
    const help = () => useWorkspace.getState().tabs.find((x) => x.id === "help");
    fireEvent.pointerDown(within(agent).getByText(/уходит модели агента без маскирования/));
    expect(document.activeElement).toBe(document.body);
    act(() => openContextHelp());
    expect(help()).toMatchObject({ kind: "help", chapter: "llm-mcp", anchor: "background" });
    // Щелчок в разделе MCP вне подраздела — глава без раздела.
    act(() => useWorkspace.getState().activate("settings"));
    fireEvent.pointerDown(screen.getByRole("heading", { level: 3, name: "LLM-ассистенты (MCP)" }));
    act(() => openContextHelp());
    expect(help()).toMatchObject({ kind: "help", chapter: "llm-mcp" });
    expect((help() as { anchor?: string }).anchor).toBeUndefined();
    // И в разделе «Язык» — «Начало работы».
    act(() => useWorkspace.getState().activate("settings"));
    fireEvent.pointerDown(screen.getByRole("heading", { level: 3, name: "Язык" }));
    act(() => openContextHelp());
    expect(help()).toMatchObject({ kind: "help", chapter: "getting-started" });
    expect((help() as { anchor?: string }).anchor).toBeUndefined();
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
