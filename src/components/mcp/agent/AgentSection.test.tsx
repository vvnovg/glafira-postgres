import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentSettings, AgentStatus, AppInfo, McpStatus } from "../../../api/types";
import { useWorkspace } from "../../../state/workspace";
import { emitEvent, listeners, resetEventBus } from "../../../test/events";
import { connection } from "../../../test/fixtures";
import { agentRun } from "../../../test/llm";
import { agentSettings, agentStatus, mcpStatus } from "../../../test/mcp";
import { renderWithClient } from "../../../test/render";

vi.mock("@tauri-apps/api/event", async () => (await import("../../../test/events")).tauriEventMock());
vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return {
    ...orig,
    api: {
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
      listConnections: vi.fn(),
      getAppInfo: vi.fn(),
    },
  };
});

import { api } from "../../../api/client";
import { AgentSection } from "./AgentSection";

const TOGGLE = "Запускать ассистента в фоне";
/** Настройки с заданной программой Qwen Code. */
const configured: AgentSettings = agentSettings({ qwen: { path: "/opt/homebrew/bin/qwen", model: "" } });
/** Glafira на этой ОС (`AppInfo.os` — правила абсолютного пути). */
const appInfo = (os: string): AppInfo => ({ ...{ name: "Glafira for PostgreSQL", version: "0.6.0", commit: "abc1234", os: "macos", arch: "aarch64", dataDir: "/d", logDir: "/l" }, os });
/** Статус MCP: сервер работает, агент настроен. */
const ready = (patch: Partial<AgentStatus> = {}): McpStatus => mcpStatus({ agent: agentStatus({ configured: true, ...patch }) });

describe("AgentSection", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    resetEventBus();
    useWorkspace.setState({ tabs: [], activeId: null });
    vi.mocked(api.getAgentSettings).mockResolvedValue(agentSettings());
    vi.mocked(api.listAgentModels).mockResolvedValue({ ids: [], default: null, source: "none" });
    vi.mocked(api.listAgentRuns).mockResolvedValue([]);
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", name: "perf" })]);
    vi.mocked(api.getAppInfo).mockResolvedValue(appInfo("macos"));
  });

  it("выключатель сохраняется сразу, статус MCP перечитывается; пометка о маскировании и расходах видна всегда", async () => {
    vi.mocked(api.setAgentSettings).mockImplementation(async (s) => s);
    const user = userEvent.setup();
    const { client } = renderWithClient(<AgentSection status={mcpStatus()} />);
    expect(screen.getByText("Снимок запроса (SQL, план, статистика, DDL) уходит модели агента без маскирования. У облачной модели каждый запуск — платный вызов.")).toBeInTheDocument();
    const toggle = await screen.findByRole("checkbox", { name: TOGGLE });
    expect(toggle).not.toBeChecked();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    await user.click(toggle);
    expect(api.setAgentSettings).toHaveBeenCalledWith(agentSettings({ enabled: true }));
    await waitFor(() => expect(toggle).toBeChecked());
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["mcpStatus"], exact: true });
    // Статус ещё говорит «не настроен»: кнопка пока работает как в P4b.
    expect(screen.getByText("Агент не настроен: укажите программу и сохраните — до этого кнопка копирует строку для чата")).toBeInTheDocument();
    // Выключатель не трогает черновик: «Сохранить» по-прежнему неактивна.
    expect(screen.getByRole("button", { name: "Сохранить" })).toBeDisabled();
  });

  it("выключатель с несохранёнными полями сохраняет только себя: остальное — по сохранённым настройкам", async () => {
    vi.mocked(api.setAgentSettings).mockImplementation(async (s) => s);
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    await user.type(await screen.findByRole("textbox", { name: "Программа" }), "/opt/homebrew/bin/qwen");
    await user.click(screen.getByRole("checkbox", { name: TOGGLE }));
    expect(api.setAgentSettings).toHaveBeenLastCalledWith(agentSettings({ enabled: true }));
    await waitFor(() => expect(screen.getByRole("checkbox", { name: TOGGLE })).toBeChecked());
    expect(screen.getByRole("textbox", { name: "Программа" })).toHaveValue("/opt/homebrew/bin/qwen");
    expect(screen.getByRole("button", { name: "Сохранить" })).toBeEnabled();
  });

  it.each<[string, McpStatus, string]>([
    ["политика", mcpStatus({ agent: agentStatus({ policyEnabled: false, configured: true }) }), "Отключено вашей организацией"],
    ["MCP-сервер выключен", mcpStatus({ server: "stopped", agent: agentStatus({ configured: true }) }), "Включите MCP-сервер выше: фоновый ассистент работает через него"],
    ["MCP-сервер в другом окне", mcpStatus({ server: "otherInstance", agent: agentStatus({ configured: true }) }), "MCP-сервер работает в другом окне Glafira: фоновым ассистентом управляет то окно"],
    ["MCP-сервер не запустился", mcpStatus({ server: "error", error: "Address already in use", agent: agentStatus({ configured: true }) }), "MCP-сервер не запустился: фоновый ассистент работает через него"],
  ])("выключатель и «Проверить» неактивны: %s", async (_name, status, note) => {
    vi.mocked(api.getAgentSettings).mockResolvedValue(configured);
    renderWithClient(<AgentSection status={status} />);
    expect(await screen.findByRole("checkbox", { name: TOGGLE })).toBeDisabled();
    expect(screen.getByText(note)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Проверить" })).toBeDisabled();
  });

  it("запрет политикой не мешает править путь и модель: включённый до запрета выключатель сохраняется как был", async () => {
    vi.mocked(api.getAgentSettings).mockResolvedValue({ ...configured, enabled: true });
    vi.mocked(api.setAgentSettings).mockImplementation(async (s) => s);
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus({ agent: agentStatus({ policyEnabled: false, enabled: true, configured: true }) })} />);
    const toggle = await screen.findByRole("checkbox", { name: TOGGLE });
    expect(toggle).toBeDisabled();
    expect(toggle).not.toBeChecked();
    const model = screen.getByRole("combobox", { name: "Модель" });
    expect(model).toBeEnabled();
    await user.type(model, "qwen3.8:27b");
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    expect(api.setAgentSettings).toHaveBeenCalledWith({ ...configured, enabled: true, qwen: { path: "/opt/homebrew/bin/qwen", model: "qwen3.8:27b" } });
    expect(await screen.findByText("Сохранено")).toBeInTheDocument();
  });

  it("Claude Code: «Через Ollama» и предел расходов; через Ollama предела нет, модель обязательна; у Qwen Code — ни того ни другого", async () => {
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    await user.click(await screen.findByRole("radio", { name: "Claude Code" }));
    await waitFor(() => expect(api.listAgentModels).toHaveBeenLastCalledWith("claude"));
    expect(screen.getByRole("textbox", { name: "Предел расходов на запуск, $" })).toHaveValue("1");
    // Что значит пустое поле «Модель» — видимым текстом под полем, а не плейсхолдером (контраст AA).
    expect(screen.getByText("Пусто — модель клиента по умолчанию")).toHaveClass("muted");
    expect(screen.getByRole("combobox", { name: "Модель" })).not.toHaveAttribute("placeholder");
    await user.click(screen.getByRole("checkbox", { name: "Через Ollama" }));
    expect(screen.queryByRole("textbox", { name: "Предел расходов на запуск, $" })).toBeNull();
    expect(screen.getByRole("combobox", { name: "Модель" })).toHaveAccessibleDescription("Нужна модель Ollama");
    await user.type(screen.getByRole("combobox", { name: "Модель" }), "qwen3.6-64k:35b-a3b");
    expect(screen.queryByText("Нужна модель Ollama")).toBeNull();
    await user.click(screen.getByRole("radio", { name: "Qwen Code" }));
    expect(screen.queryByRole("checkbox", { name: "Через Ollama" })).toBeNull();
    expect(screen.queryByRole("textbox", { name: "Предел расходов на запуск, $" })).toBeNull();
  });

  it("«Найти» подставляет путь и показывает версию; ниже минимальной — предупреждение; версию не узнать — пометка; не нашлась — просьба указать путь", async () => {
    // Ответы — как у Rust (`agent::detect`): `error` бывает только у найденной программы (не удался `--version`), не
    // нашлась — `Detected::default()`.
    vi.mocked(api.detectAgentCli)
      .mockResolvedValueOnce({ path: "/opt/homebrew/bin/qwen", version: "0.26.5", belowMinimum: false, error: null })
      .mockResolvedValueOnce({ path: "/usr/local/bin/qwen", version: "0.25.1", belowMinimum: true, error: null })
      .mockResolvedValueOnce({ path: "/Users/me/.npm-global/bin/qwen", version: null, belowMinimum: false, error: "/Users/me/.npm-global/bin/qwen --version did not answer within 10s" })
      .mockResolvedValueOnce({ path: null, version: null, belowMinimum: false, error: null });
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    const path = await screen.findByRole("textbox", { name: "Программа" });
    await user.click(screen.getByRole("button", { name: "Найти" }));
    expect(api.detectAgentCli).toHaveBeenLastCalledWith("qwen");
    await waitFor(() => expect(path).toHaveValue("/opt/homebrew/bin/qwen"));
    expect(screen.getByText("Версия 0.26.5")).toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: "Найти" }));
    expect(await screen.findByText("Версия 0.25.1 ниже минимальной 0.26: флаги запуска могут не сработать")).toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: "Найти" }));
    const versionError = await screen.findByText("Версию не узнать: /Users/me/.npm-global/bin/qwen --version did not answer within 10s");
    expect(versionError).toHaveClass("diag-warn");
    expect(path).toHaveValue("/Users/me/.npm-global/bin/qwen");
    expect(screen.queryByText(/Программа не найдена/)).toBeNull();
    await user.click(await screen.findByRole("button", { name: "Найти" }));
    expect(await screen.findByText("Программа не найдена: укажите путь вручную")).toBeInTheDocument();
    expect(screen.queryByText(/Версию не узнать/)).toBeNull();
    // Не нашлось — введённый путь остаётся.
    expect(path).toHaveValue("/Users/me/.npm-global/bin/qwen");
    // Итог «Найти» — у своего агента: у Claude Code его нет.
    await user.click(screen.getByRole("radio", { name: "Claude Code" }));
    expect(screen.queryByText("Программа не найдена: укажите путь вручную")).toBeNull();
  });

  it("модель: список из настроек клиента и свободный ввод; неверное имя — ошибка у поля, «Сохранить» неактивна", async () => {
    vi.mocked(api.listAgentModels).mockResolvedValue({ ids: ["qwen3.6-64k:35b-a3b", "qwen3.8:27b"], default: "qwen3.6-64k:35b-a3b", source: "qwen" });
    const user = userEvent.setup();
    const { container } = renderWithClient(<AgentSection status={mcpStatus()} />);
    const model = await screen.findByRole("combobox", { name: "Модель" });
    expect(await screen.findByText("По умолчанию: qwen3.6-64k:35b-a3b")).toBeInTheDocument();
    // Что значит пустое поле, скринридер читает при входе в поле.
    expect(model).toHaveAccessibleDescription("По умолчанию: qwen3.6-64k:35b-a3b");
    expect(Array.from(container.querySelectorAll("datalist option")).map((o) => o.getAttribute("value"))).toEqual(["qwen3.6-64k:35b-a3b", "qwen3.8:27b"]);
    await user.type(model, "bad model");
    expect(model).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByText("Латиница, цифры и . _ : / @ + -, до 128 символов")).toBeInTheDocument();
    expect(model).toHaveAccessibleDescription("Латиница, цифры и . _ : / @ + -, до 128 символов");
    expect(screen.getByRole("button", { name: "Сохранить" })).toBeDisabled();
  });

  it("список моделей не прочитан — свободный ввод с пояснением", async () => {
    vi.mocked(api.listAgentModels).mockRejectedValue({ code: "INTERNAL", message: "boom", retryable: false });
    renderWithClient(<AgentSection status={mcpStatus()} />);
    expect(await screen.findByText("Список моделей не прочитан: введите имя вручную")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Модель" })).toBeEnabled();
  });

  it("сроки и предел проверяются по пределам Rust; «Сохранить» отправляет настройки целиком", async () => {
    vi.mocked(api.setAgentSettings).mockImplementation(async (s) => s);
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    const timeout = await screen.findByRole("textbox", { name: "Срок запуска, мин" });
    const save = screen.getByRole("button", { name: "Сохранить" });
    expect(save).toBeDisabled();
    await user.clear(timeout);
    await user.type(timeout, "1");
    expect(screen.getByText("Целое число от 2 до 60")).toBeInTheDocument();
    expect(save).toBeDisabled();
    await user.type(timeout, "0");
    const keep = screen.getByRole("textbox", { name: "Держать модель в памяти после запуска, мин" });
    await user.clear(keep);
    await user.type(keep, "121");
    expect(screen.getByText("Целое число от 0 до 120")).toBeInTheDocument();
    await user.clear(keep);
    await user.type(keep, "0");
    await user.click(screen.getByRole("radio", { name: "Claude Code" }));
    const budget = screen.getByRole("textbox", { name: "Предел расходов на запуск, $" });
    await user.clear(budget);
    await user.type(budget, "0");
    expect(screen.getByText("Число от 0,01 до 100 или пусто")).toBeInTheDocument();
    await user.clear(budget);
    await user.type(budget, "2,5");
    await user.type(screen.getByRole("textbox", { name: "Программа" }), "/Users/me/.local/bin/claude");
    await user.click(save);
    expect(api.setAgentSettings).toHaveBeenCalledWith(
      agentSettings({ preset: "claude", claude: { path: "/Users/me/.local/bin/claude", model: "", viaOllama: false, budgetUsd: 2.5 }, timeoutMin: 10, keepWarmMin: 0 }),
    );
    expect(await screen.findByText("Сохранено")).toBeInTheDocument();
    expect(save).toBeDisabled();
    // Правка после сохранения снимает «Сохранено».
    await user.type(timeout, "5");
    expect(screen.queryByText("Сохранено")).toBeNull();
  });

  it("запись настроек — одна за раз: пока идёт «Сохранить», выключатель не пишет", async () => {
    let finish: (s: AgentSettings) => void = () => {};
    vi.mocked(api.setAgentSettings)
      .mockImplementationOnce(() => new Promise<AgentSettings>((r) => (finish = r)))
      .mockImplementation(async (s) => s);
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    await user.type(await screen.findByRole("textbox", { name: "Программа" }), "/opt/homebrew/bin/qwen");
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    const toggle = screen.getByRole("checkbox", { name: TOGGLE });
    expect(toggle).toHaveAttribute("aria-disabled", "true");
    await user.click(toggle);
    expect(api.setAgentSettings).toHaveBeenCalledTimes(1);
    expect(toggle).not.toBeChecked();
    const saved = agentSettings({ qwen: { path: "/opt/homebrew/bin/qwen", model: "" } });
    finish(saved);
    await waitFor(() => expect(toggle).not.toHaveAttribute("aria-disabled"));
    await user.click(toggle);
    // Выключатель берёт остальное из только что сохранённых настроек.
    expect(api.setAgentSettings).toHaveBeenLastCalledWith({ ...saved, enabled: true });
  });

  it("ответ проверки пути, пришедший после новой правки, отброшен: он о прежнем пути", async () => {
    let answer: (d: { path: string; version: string; belowMinimum: boolean; error: null }) => void = () => {};
    vi.mocked(api.detectAgentCli).mockImplementationOnce(() => new Promise((r) => (answer = r)));
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    const path = await screen.findByRole("textbox", { name: "Программа" });
    await user.type(path, "/usr/local/bin/qwen");
    await user.tab();
    expect(api.detectAgentCli).toHaveBeenCalledWith("qwen", "/usr/local/bin/qwen");
    // Пока проверка идёт, путь поправили.
    await user.type(path, "2");
    answer({ path: "/usr/local/bin/qwen", version: "0.25.1", belowMinimum: true, error: null });
    await waitFor(() => expect(api.detectAgentCli).toHaveBeenCalledTimes(1));
    await new Promise((r) => setTimeout(r, 0));
    expect(screen.queryByText(/Версия 0.25.1/)).toBeNull();
    expect(path).toHaveValue("/usr/local/bin/qwen2");
  });

  it("отказ Rust при сохранении — ошибка, черновик остаётся", async () => {
    vi.mocked(api.setAgentSettings).mockRejectedValue({ code: "INTERNAL", message: "settings.json: disk full", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    const path = await screen.findByRole("textbox", { name: "Программа" });
    await user.type(path, "/opt/homebrew/bin/qwen");
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("settings.json: disk full");
    expect(path).toHaveValue("/opt/homebrew/bin/qwen");
    expect(screen.getByRole("button", { name: "Сохранить" })).toBeEnabled();
  });

  it("абсолютный путь — по ОС из AppInfo.os, как у Rust (Path::is_absolute); ОС не прочитана — любой вид, решит Rust", async () => {
    const user = userEvent.setup();
    const mac = renderWithClient(<AgentSection status={mcpStatus()} />);
    await user.type(await screen.findByRole("textbox", { name: "Программа" }), "C:\\Tools\\qwen.cmd");
    expect(await screen.findByText("Нужен абсолютный путь")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Сохранить" })).toBeDisabled();
    mac.unmount();
    vi.mocked(api.getAppInfo).mockResolvedValue(appInfo("windows"));
    const win = renderWithClient(<AgentSection status={mcpStatus()} />);
    const path = await screen.findByRole("textbox", { name: "Программа" });
    await waitFor(() => expect(api.getAppInfo).toHaveBeenCalled());
    await user.type(path, "C:\\Tools\\qwen.cmd");
    expect(screen.queryByText("Нужен абсолютный путь")).toBeNull();
    await user.clear(path);
    await user.type(path, "/usr/local/bin/qwen");
    expect(await screen.findByText("Нужен абсолютный путь")).toBeInTheDocument();
    win.unmount();
    vi.mocked(api.getAppInfo).mockRejectedValue({ code: "INTERNAL", message: "no info", retryable: false });
    renderWithClient(<AgentSection status={mcpStatus()} />);
    await user.type(await screen.findByRole("textbox", { name: "Программа" }), "C:\\Tools\\qwen.cmd");
    await waitFor(() => expect(api.getAppInfo).toHaveBeenCalledTimes(3));
    expect(screen.queryByText("Нужен абсолютный путь")).toBeNull();
    expect(screen.getByRole("button", { name: "Сохранить" })).toBeEnabled();
  });

  it("путь, указанный руками, при уходе из поля — версия этой программы (detect_agent_cli с path); путь не меняется", async () => {
    vi.mocked(api.detectAgentCli).mockResolvedValue({ path: "/usr/local/bin/qwen", version: "0.25.1", belowMinimum: true, error: null });
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    const path = await screen.findByRole("textbox", { name: "Программа" });
    // Уход из поля без правки — без проверки.
    await user.click(path);
    await user.tab();
    expect(api.detectAgentCli).not.toHaveBeenCalled();
    await user.type(path, "/usr/local/bin/qwen");
    // Пока набирается — не проверяется.
    expect(api.detectAgentCli).not.toHaveBeenCalled();
    await user.tab();
    expect(api.detectAgentCli).toHaveBeenCalledWith("qwen", "/usr/local/bin/qwen");
    expect(await screen.findByText("Версия 0.25.1 ниже минимальной 0.26: флаги запуска могут не сработать")).toBeInTheDocument();
    expect(path).toHaveValue("/usr/local/bin/qwen");
    // Неверный путь не проверяется: ошибка у поля.
    await user.clear(path);
    await user.type(path, "qwen");
    await user.tab();
    expect(api.detectAgentCli).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(/Версия 0.25.1/)).toBeNull();
  });

  it("запись настроек — одна за раз: пока пишется выключатель, «Сохранить» не отправляет прежний снимок", async () => {
    let finish: (s: AgentSettings) => void = () => {};
    vi.mocked(api.setAgentSettings)
      .mockImplementationOnce(() => new Promise<AgentSettings>((r) => (finish = r)))
      .mockImplementation(async (s) => s);
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    await user.type(await screen.findByRole("textbox", { name: "Программа" }), "/opt/homebrew/bin/qwen");
    await user.click(screen.getByRole("checkbox", { name: TOGGLE }));
    const save = screen.getByRole("button", { name: "Сохранить" });
    expect(save).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("checkbox", { name: TOGGLE })).toHaveAttribute("aria-disabled", "true");
    await user.click(save);
    expect(api.setAgentSettings).toHaveBeenCalledTimes(1);
    finish(agentSettings({ enabled: true }));
    await waitFor(() => expect(save).not.toHaveAttribute("aria-disabled"));
    await user.click(save);
    // «Сохранить» берёт выключатель из сохранённых настроек — уже включённый.
    expect(api.setAgentSettings).toHaveBeenLastCalledWith(agentSettings({ enabled: true, qwen: { path: "/opt/homebrew/bin/qwen", model: "" } }));
  });

  it("ошибка в полях другого агента названа у «Сохранить»", async () => {
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    await user.click(await screen.findByRole("radio", { name: "Claude Code" }));
    await user.type(screen.getByRole("textbox", { name: "Программа" }), "claude");
    await user.click(screen.getByRole("radio", { name: "Qwen Code" }));
    expect(screen.getByText("Есть ошибка в настройках Claude Code: выберите этот агент и исправьте")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Сохранить" })).toBeDisabled();
  });

  it("адрес Ollama не локальный — пометка; «Выгрузить модель» неактивна во время запуска, иначе выгружает", async () => {
    vi.mocked(api.unloadAgentModel).mockResolvedValue(undefined);
    const user = userEvent.setup();
    const first = renderWithClient(<AgentSection status={mcpStatus({ agent: agentStatus({ running: { runId: "a1b2c3d", requestId: "r3a9f1c" } }) })} />);
    expect(await screen.findByRole("button", { name: "Выгрузить модель" })).toBeDisabled();
    first.unmount();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    await user.click(await screen.findByRole("button", { name: "Выгрузить модель" }));
    expect(api.unloadAgentModel).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("Модель выгружена")).toBeInTheDocument();
    const url = screen.getByRole("textbox", { name: "Адрес Ollama" });
    await user.clear(url);
    await user.type(url, "http://10.0.0.5:11434");
    expect(screen.getByText("Адрес не локальный: Glafira не управляет моделью (только localhost, 127.0.0.1, ::1)")).toBeInTheDocument();
  });

  it.each<[string, AgentSettings, string]>([
    ["Claude Code без Ollama", agentSettings({ preset: "claude" }), "У Claude Code без Ollama локальной модели нет: выгружать нечего"],
    ["«держать модель» = 0", agentSettings({ keepWarmMin: 0 }), "«Держать модель в памяти» — 0: Glafira моделью не управляет"],
  ])("Glafira моделью не управляет (%s) — «Выгрузить модель» неактивна с пояснением", async (_name, saved, note) => {
    vi.mocked(api.getAgentSettings).mockResolvedValue(saved);
    renderWithClient(<AgentSection status={mcpStatus()} />);
    expect(await screen.findByRole("button", { name: "Выгрузить модель" })).toBeDisabled();
    expect(screen.getByText(note)).toBeInTheDocument();
  });

  it("сохранённый адрес Ollama не локальный — «Выгрузить модель» неактивна", async () => {
    vi.mocked(api.getAgentSettings).mockResolvedValue(agentSettings({ ollamaUrl: "http://10.0.0.5:11434" }));
    renderWithClient(<AgentSection status={mcpStatus()} />);
    expect(await screen.findByRole("button", { name: "Выгрузить модель" })).toBeDisabled();
    expect(screen.getByText("Адрес не локальный: Glafira не управляет моделью (только localhost, 127.0.0.1, ::1)")).toBeInTheDocument();
  });

  it("«Проверить»: короткий запуск, итог приходит событием — «Работает: …» или «Не работает» с журналом", async () => {
    vi.mocked(api.getAgentSettings).mockResolvedValue(configured);
    const check = agentRun({ id: "a0c0c0c", kind: "check", requestId: null, connectionId: null, queuedAtMs: 10 });
    vi.mocked(api.checkAgent).mockResolvedValue(check);
    vi.mocked(api.readAgentLog).mockResolvedValue("stderr: unknown model");
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={ready()} />);
    const button = await screen.findByRole("button", { name: "Проверить" });
    await waitFor(() => expect(button).toBeEnabled());
    await user.click(button);
    expect(await screen.findByText("Проверка…")).toBeInTheDocument();
    // Пока проверка идёт, вторую не запустить; кнопка не теряет фокус (aria-disabled, а не disabled).
    expect(screen.getByRole("button", { name: "Проверить" })).toHaveAttribute("aria-disabled", "true");
    await user.click(screen.getByRole("button", { name: "Проверить" }));
    expect(api.checkAgent).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(listeners("agent://runs").length).toBeGreaterThan(0));
    emitEvent("agent://runs", { ...check, status: "done", startedAtMs: 1_000, endedAtMs: 24_000, calls: 1 });
    expect(await screen.findByText("Работает: Qwen Code 0.26.5, qwen3.6-64k:35b-a3b, 23 с")).toBeInTheDocument();
    // Удачной проверке журнал в строке итога не нужен (он есть в «Запусках»).
    expect(screen.queryByRole("button", { name: "Показать журнал проверки" })).toBeNull();
    vi.mocked(api.checkAgent).mockResolvedValue({ ...check, id: "a0c0c0d", queuedAtMs: 11 });
    await user.click(screen.getByRole("button", { name: "Проверить" }));
    expect(await screen.findByText("Проверка…")).toBeInTheDocument();
    emitEvent("agent://runs", { ...check, id: "a0c0c0d", queuedAtMs: 11, status: "failed", reason: "notConnected", startedAtMs: 1_000, endedAtMs: 5_000 });
    expect(await screen.findByText("Не работает: агент не подключился к Glafira")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Показать журнал проверки" }));
    expect(api.readAgentLog).toHaveBeenCalledWith("a0c0c0d");
    expect(await screen.findByText("stderr: unknown model")).toBeInTheDocument();
  });

  it("«Проверить» отклонена Rust — ошибка", async () => {
    vi.mocked(api.getAgentSettings).mockResolvedValue(configured);
    vi.mocked(api.checkAgent).mockRejectedValue({ code: "INVALID_INPUT", key: "errors.agent.disabledByPolicy", message: "The background assistant is disabled by your organization", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={ready()} />);
    const button = await screen.findByRole("button", { name: "Проверить" });
    await waitFor(() => expect(button).toBeEnabled());
    await user.click(button);
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.queryByText("Проверка…")).toBeNull();
  });

  it("«Проверить» неактивна, пока программа не задана или есть несохранённые изменения", async () => {
    const user = userEvent.setup();
    const first = renderWithClient(<AgentSection status={mcpStatus()} />);
    expect(await screen.findByText("Укажите программу и сохраните, чтобы проверить")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Проверить" })).toBeDisabled();
    first.unmount();
    vi.mocked(api.getAgentSettings).mockResolvedValue(configured);
    renderWithClient(<AgentSection status={ready()} />);
    const timeout = await screen.findByRole("textbox", { name: "Срок запуска, мин" });
    await waitFor(() => expect(screen.getByRole("button", { name: "Проверить" })).toBeEnabled());
    await user.type(timeout, "0");
    expect(screen.getByRole("button", { name: "Проверить" })).toBeDisabled();
    expect(screen.getByText("Сохраните изменения, чтобы проверить")).toBeInTheDocument();
  });

  it("Claude Code через Ollama без модели: агент не настроен — пометка и «Проверить» называют модель", async () => {
    vi.mocked(api.getAgentSettings).mockResolvedValue(
      agentSettings({ enabled: true, preset: "claude", claude: { path: "/Users/me/.local/bin/claude", model: "", viaOllama: true, budgetUsd: 1 } }),
    );
    renderWithClient(<AgentSection status={mcpStatus({ agent: agentStatus({ enabled: true, preset: "claude" }) })} />);
    expect(
      await screen.findByText("Агент не настроен: Claude Code через Ollama нужна модель — укажите её и сохраните. До этого кнопка копирует строку для чата"),
    ).toBeInTheDocument();
    expect(screen.getByText("Укажите модель и сохраните, чтобы проверить")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Проверить" })).toBeDisabled();
  });

  it("запуски читаются — «Загрузка…», не прочитались — ошибка с «Повторить», а не «Запусков пока нет»", async () => {
    vi.mocked(api.listAgentRuns).mockRejectedValueOnce({ code: "INTERNAL", message: "agent-runs.json: broken", retryable: false }).mockResolvedValue([]);
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("agent-runs.json: broken");
    expect(screen.queryByText("Запусков пока нет")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(await screen.findByText("Запусков пока нет")).toBeInTheDocument();
  });

  it("настройки не прочитались — ошибка с «Повторить»", async () => {
    vi.mocked(api.getAgentSettings).mockRejectedValueOnce({ code: "INTERNAL", message: "disk", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("disk");
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(await screen.findByRole("checkbox", { name: TOGGLE })).toBeInTheDocument();
  });

  it("открытие подраздела индикатором (`section: \"agent\"`) прокручивает его в видимую часть", async () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll;
    useWorkspace.setState({ tabs: [{ kind: "settings", section: "agent", id: "settings" }], activeId: "settings" });
    renderWithClient(<AgentSection status={mcpStatus()} />);
    await waitFor(() => expect(scroll).toHaveBeenCalledTimes(1));
    expect(scroll.mock.contexts[0]).toBe(screen.getByRole("region", { name: "Фоновый ассистент" }));
    delete (Element.prototype as { scrollIntoView?: unknown }).scrollIntoView;
  });
});
