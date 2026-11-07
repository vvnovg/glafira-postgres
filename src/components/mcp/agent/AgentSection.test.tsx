import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentSettings, AgentStatus, AppInfo, CloudModel, McpStatus, OllamaPrices as OllamaPricesInfo } from "../../../api/types";
import { i18n } from "../../../i18n";
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
      getOllamaPrices: vi.fn(),
      refreshOllamaPrices: vi.fn(),
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
    vi.mocked(api.getOllamaPrices).mockResolvedValue({ date: "2026-11-07", source: "https://ollama.com/pricing", saved: false, models: 24, refreshAllowed: true });
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
    expect(screen.getByRole("combobox", { name: "Модель" })).toBeEnabled();
    await typeModel(user, "qwen3.8:27b");
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
    // Что значит пустая модель — пунктом списка, а не плейсхолдером (контраст AA).
    expect(screen.getByRole("combobox", { name: "Модель" })).toHaveValue("");
    expect(screen.getByRole("option", { name: "По умолчанию (модель клиента)", selected: true })).toBeInTheDocument();
    // То же у пустого «Предела расходов»: строка .muted под полем, в описании поля.
    const budget = screen.getByRole("textbox", { name: "Предел расходов на запуск, $" });
    expect(screen.queryByText("Пусто — без предела")).toBeNull();
    expect(budget).not.toHaveAccessibleDescription();
    await user.clear(budget);
    expect(screen.getByText("Пусто — без предела")).toHaveClass("muted");
    expect(budget).toHaveAccessibleDescription("Пусто — без предела");
    expect(budget).not.toHaveAttribute("placeholder");
    await user.type(budget, "0");
    expect(budget).toHaveAccessibleDescription("Число от 0,01 до 100 или пусто");
    await user.clear(budget);
    await user.type(budget, "1");
    await user.click(screen.getByRole("checkbox", { name: "Через Ollama" }));
    expect(screen.queryByRole("textbox", { name: "Предел расходов на запуск, $" })).toBeNull();
    expect(screen.getByRole("option", { name: "Не выбрана: нужна модель Ollama", selected: true })).toBeInTheDocument();
    // В поле ручного ввода пустое имя у Claude Code через Ollama — тоже пояснение под полем.
    const name = await typeModel(user, "");
    expect(name).toHaveAccessibleDescription("Нужна модель Ollama");
    await user.type(name, "qwen3.6-64k:35b-a3b");
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

  /** Облачные модели Ollama из таблицы цен (`list_agent_models`, группа «Ollama Cloud»). */
  const CLOUD: CloudModel[] = [
    { id: "glm-5.3-flash:cloud", prices: { input: 0.15, cachedInput: 0.03, output: 0.5 }, offPeak: null },
    { id: "gpt-oss:120b-cloud", prices: { input: 0.15, cachedInput: 0.014, output: 0.6 }, offPeak: null },
    { id: "nemotron-3-super:cloud", prices: { input: 0.015, cachedInput: 0.015, output: 0.6 }, offPeak: null },
    { id: "deepseek-v4.1-flash:cloud", prices: { input: 0.3, cachedInput: 0.006, output: 1.2 }, offPeak: { input: 0.15, cachedInput: 0.003, output: 0.6 } },
  ];
  /**
   * Список «Модель» (смоук P4c, R19): пункты вне групп и группы `<optgroup>` по порядку — `[подпись группы или null, [[значение,
   * текст пункта]…]]`; пробелы Intl — обычными.
   */
  const modelList = () => {
    const select = screen.getByRole("combobox", { name: "Модель" });
    const item = (o: HTMLOptionElement) => [o.value, o.textContent?.replace(/\s/g, " ") ?? ""];
    return Array.from(select.children).map((c) =>
      c instanceof HTMLOptGroupElement ? [c.label, Array.from(c.querySelectorAll("option")).map(item)] : [null, [item(c as HTMLOptionElement)]],
    );
  };
  /** Только имена моделей групп (без «По умолчанию», «Другая модель…» и модели не из списка). */
  const groupedModels = () => modelList().flatMap(([label, items]) => (label === null ? [] : (items as string[][]).map(([v]) => v)));
  /** Имя модели руками: «Другая модель…» и поле «Имя модели». */
  const typeModel = async (user: ReturnType<typeof userEvent.setup>, name: string) => {
    await user.selectOptions(screen.getByRole("combobox", { name: "Модель" }), "Другая модель…");
    const input = screen.getByRole("textbox", { name: "Имя модели" });
    await user.clear(input);
    if (name !== "") await user.type(input, name);
    return input;
  };

  it("модель — выпадающий список (R19): группы «Настройки Qwen Code», «Ollama», «Ollama Cloud» с ценой в тексте пункта, текущая выбрана", async () => {
    vi.mocked(api.getAgentSettings).mockResolvedValue(agentSettings({ qwen: { path: "/opt/homebrew/bin/qwen", model: "nemotron-3-super:cloud" } }));
    vi.mocked(api.listAgentModels).mockResolvedValue({
      ids: ["glm-5.3-flash:cloud", "qwen3.8:27b"],
      default: "qwen3.8:27b",
      source: "qwen",
      ollama: ["gemma3:4b", "qwen3.8:27b", "gpt-oss:120b-cloud", "qwen3.6-64k:35b-a3b"],
      cloud: CLOUD,
    });
    renderWithClient(<AgentSection status={mcpStatus()} />);
    await waitFor(() => expect(groupedModels()).toContain("nemotron-3-super:cloud"));
    expect(modelList()).toEqual([
      [null, [["", "По умолчанию (qwen3.8:27b)"]]],
      [
        "Настройки Qwen Code",
        [
          ["glm-5.3-flash:cloud", "glm-5.3-flash:cloud"],
          ["qwen3.8:27b", "qwen3.8:27b"],
        ],
      ],
      [
        "Ollama",
        [
          ["gemma3:4b", "gemma3:4b"],
          ["qwen3.6-64k:35b-a3b", "qwen3.6-64k:35b-a3b"],
        ],
      ],
      [
        "Ollama Cloud",
        [
          ["gpt-oss:120b-cloud", "gpt-oss:120b-cloud · ≈ 0,15 $ / 0,60 $ за 1 млн токенов, вход / выход"],
          ["nemotron-3-super:cloud", "nemotron-3-super:cloud · ≈ 0,015 $ / 0,60 $ за 1 млн токенов, вход / выход"],
          ["deepseek-v4.1-flash:cloud", "deepseek-v4.1-flash:cloud · ≈ 0,30 $ / 1,20 $ за 1 млн токенов, вход / выход; вне пика — 0,15 $ / 0,60 $"],
        ],
      ],
      [null, [["*other", "Другая модель…"]]],
    ]);
    const select = screen.getByRole("combobox", { name: "Модель" });
    expect(select).toHaveValue("nemotron-3-super:cloud");
    expect(screen.queryByRole("textbox", { name: "Имя модели" })).toBeNull();
    // Выбор из списка — в черновик: «Сохранить» становится активной.
    const user = userEvent.setup();
    await user.selectOptions(select, "qwen3.6-64k:35b-a3b");
    expect(select).toHaveValue("qwen3.6-64k:35b-a3b");
    expect(screen.getByRole("button", { name: "Сохранить" })).toBeEnabled();
    await user.selectOptions(select, "По умолчанию (qwen3.8:27b)");
    expect(select).toHaveValue("");
  });

  it("модель не из списка — отдельным пунктом сверху; «Другая модель…» — поле ручного ввода с прежней проверкой имени", async () => {
    vi.mocked(api.getAgentSettings).mockResolvedValue(agentSettings({ qwen: { path: "/opt/homebrew/bin/qwen", model: "my-model:1b" } }));
    vi.mocked(api.listAgentModels).mockResolvedValue({ ids: ["qwen3.8:27b"], default: null, source: "qwen", cloud: CLOUD.slice(0, 1) });
    vi.mocked(api.setAgentSettings).mockImplementation(async (s) => s);
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    await waitFor(() => expect(groupedModels()).toEqual(["qwen3.8:27b", "glm-5.3-flash:cloud"]));
    const select = screen.getByRole("combobox", { name: "Модель" });
    expect(modelList()[0]).toEqual([null, [["my-model:1b", "my-model:1b"]]]);
    expect(modelList()[1]).toEqual([null, [["", "По умолчанию (модель клиента)"]]]);
    expect(select).toHaveValue("my-model:1b");
    // «Другая модель…»: поле с текущим именем, фокус остаётся на списке (смена пункта контекста не меняет).
    await user.selectOptions(select, "Другая модель…");
    expect(select).toHaveValue("*other");
    expect(select).toHaveFocus();
    const input = screen.getByRole("textbox", { name: "Имя модели" });
    expect(input).toHaveValue("my-model:1b");
    await user.clear(input);
    await user.type(input, "bad model");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(input).toHaveAccessibleDescription("Сначала латинская буква или цифра, дальше латиница, цифры и . _ : / @ + -, до 128 символов");
    expect(screen.getByRole("button", { name: "Сохранить" })).toBeDisabled();
    await user.clear(input);
    // Пустое имя — модель клиента по умолчанию: это сказано под полем.
    expect(input).toHaveAccessibleDescription("Пусто — модель клиента по умолчанию");
    await user.type(input, "qwen3.9:9b");
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    expect(api.setAgentSettings).toHaveBeenCalledWith(agentSettings({ qwen: { path: "/opt/homebrew/bin/qwen", model: "qwen3.9:9b" } }));
    expect(await screen.findByText("Сохранено")).toBeInTheDocument();
    // Пункт из списка закрывает поле ручного ввода.
    await user.selectOptions(select, "glm-5.3-flash:cloud");
    expect(screen.queryByRole("textbox", { name: "Имя модели" })).toBeNull();
    expect(select).toHaveValue("glm-5.3-flash:cloud");
  });

  it("Claude Code через Ollama — группы «Ollama» и «Ollama Cloud», пустой пункт просит модель; без «Через Ollama» групп нет, сразу", async () => {
    vi.mocked(api.getAgentSettings).mockResolvedValue(
      agentSettings({ preset: "claude", claude: { path: "/Users/me/.local/bin/claude", model: "", viaOllama: true, budgetUsd: 1 } }),
    );
    vi.mocked(api.listAgentModels).mockResolvedValue({ ids: ["qwen3.6-64k:35b-a3b"], default: null, source: "ollama", ollama: [], cloud: CLOUD.slice(1, 2) });
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    await waitFor(() => expect(groupedModels()).toEqual(["qwen3.6-64k:35b-a3b", "gpt-oss:120b-cloud"]));
    expect(modelList().map(([label]) => label)).toEqual([null, "Ollama", "Ollama Cloud", null]);
    expect(modelList()[0]).toEqual([null, [["", "Не выбрана: нужна модель Ollama"]]]);
    expect(screen.getByRole("combobox", { name: "Модель" })).toHaveValue("");
    const calls = vi.mocked(api.listAgentModels).mock.calls.length;
    await user.click(screen.getByRole("checkbox", { name: "Через Ollama" }));
    expect(groupedModels()).toEqual([]);
    expect(modelList()).toEqual([[null, [["", "По умолчанию (модель клиента)"]]], [null, [["*other", "Другая модель…"]]]]);
    expect(api.listAgentModels).toHaveBeenCalledTimes(calls);
  });

  it("облачные модели по-английски — своя подпись группы и цены", async () => {
    await act(() => i18n.changeLanguage("en"));
    try {
      vi.mocked(api.listAgentModels).mockResolvedValue({ ids: ["qwen3.8:27b"], default: null, source: "qwen", ollama: [], cloud: CLOUD.slice(1, 2) });
      renderWithClient(<AgentSection status={mcpStatus()} />);
      await screen.findByRole("combobox", { name: "Model" });
      const groups = () => Array.from(document.querySelectorAll("optgroup")).map((g) => [g.label, Array.from(g.querySelectorAll("option")).map((o) => o.textContent?.replace(/\s/g, " "))]);
      await waitFor(() =>
        expect(groups()).toEqual([
          ["Qwen Code settings", ["qwen3.8:27b"]],
          ["Ollama Cloud", ["gpt-oss:120b-cloud · ≈ $0.15 / $0.60 per 1M tokens, input / output"]],
        ]),
      );
      expect(screen.getByRole("option", { name: "Other model…" })).toBeInTheDocument();
      expect(screen.getByRole("option", { name: "Default (the client's model)" })).toBeInTheDocument();
    } finally {
      await act(() => i18n.changeLanguage("ru"));
    }
  });

  it("список моделей перечитывается при входе в поле «Модель» и после «Найти», а не на каждый щелчок; пока читается новый, виден прежний", async () => {
    /** Ответ, который тест отдаёт сам: список, который «читается». */
    let answer: (v: { ids: string[]; default: string | null; source: "qwen" }) => void = () => {};
    vi.mocked(api.listAgentModels)
      .mockResolvedValueOnce({ ids: ["qwen3.8:27b"], default: null, source: "qwen" })
      .mockImplementationOnce(() => new Promise((resolve) => (answer = resolve)))
      .mockResolvedValue({ ids: ["qwen3.8:27b", "glm-5.3-flash:cloud", "qwen3.6-64k:35b-a3b"], default: null, source: "qwen" });
    vi.mocked(api.detectAgentCli).mockResolvedValue({ path: "/opt/homebrew/bin/qwen", version: "0.26.5", belowMinimum: false, error: null });
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    const model = await screen.findByRole("combobox", { name: "Модель" });
    await waitFor(() => expect(groupedModels()).toEqual(["qwen3.8:27b"]));
    expect(api.listAgentModels).toHaveBeenCalledTimes(1);
    // Модель добавили в ~/.qwen/settings.json при открытых «Настройках»: вход в поле перечитывает список.
    await user.click(model);
    expect(api.listAgentModels).toHaveBeenCalledTimes(2);
    expect(groupedModels()).toEqual(["qwen3.8:27b"]);
    act(() => answer({ ids: ["qwen3.8:27b", "glm-5.3-flash:cloud"], default: null, source: "qwen" }));
    await waitFor(() => expect(groupedModels()).toEqual(["qwen3.8:27b", "glm-5.3-flash:cloud"]));
    // Щелчки по полю, в котором уже фокус, списка не перечитывают (итоговое ревью фронтенда).
    await user.click(model);
    await user.click(model);
    expect(api.listAgentModels).toHaveBeenCalledTimes(2);
    // «Найти» — тоже.
    await user.click(screen.getByRole("button", { name: "Найти" }));
    await waitFor(() => expect(groupedModels()).toEqual(["qwen3.8:27b", "glm-5.3-flash:cloud", "qwen3.6-64k:35b-a3b"]));
    expect(api.listAgentModels).toHaveBeenCalledTimes(3);
    expect(api.listAgentModels).toHaveBeenLastCalledWith("qwen");
  });

  it("«Обновить модели и цены» — сразу после поля «Модель»: загружает цены, затем перечитывает список; дата цен рядом с кнопкой", async () => {
    let prices: (p: OllamaPricesInfo) => void = () => {};
    vi.mocked(api.refreshOllamaPrices).mockImplementation(() => new Promise((r) => (prices = r)));
    vi.mocked(api.listAgentModels).mockResolvedValue({ ids: ["qwen3.8:27b"], default: null, source: "qwen", cloud: CLOUD.slice(0, 1) });
    const user = userEvent.setup();
    const { container } = renderWithClient(<AgentSection status={mcpStatus()} />);
    const select = await screen.findByRole("combobox", { name: "Модель" });
    const button = screen.getByRole("button", { name: "Обновить модели и цены" });
    // Кнопка — в строке поля «Модель», до «Срока запуска»; строка о дне цен — рядом с ней; под «Запусками» цен больше нет.
    const control = select.closest(".agent-control") as HTMLElement;
    expect(control).toContainElement(button);
    expect(await within(control).findByText("Стоимость облачных запусков через Ollama — оценка по ценам Ollama Cloud на 07.11.2026, встроенным в Glafira")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Подсказка: Обновить модели и цены" })).toBeInTheDocument();
    const runsBlock = container.querySelectorAll(".settings-block")[1] as HTMLElement;
    expect(within(runsBlock).queryByRole("button", { name: /Обновить/ })).toBeNull();
    expect(runsBlock).not.toHaveTextContent("ollama.com/pricing");
    await waitFor(() => expect(api.listAgentModels).toHaveBeenCalledTimes(1));
    await user.click(button);
    expect(api.refreshOllamaPrices).toHaveBeenCalledTimes(1);
    // Пока идёт загрузка — кнопка не теряет фокус и второй раз не нажимается; список ждёт новых цен.
    expect(button).toHaveTextContent("Обновление моделей и цен…");
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveFocus();
    await user.click(button);
    expect(api.refreshOllamaPrices).toHaveBeenCalledTimes(1);
    expect(api.listAgentModels).toHaveBeenCalledTimes(1);
    vi.mocked(api.listAgentModels).mockResolvedValue({ ids: ["qwen3.8:27b", "qwen3.9:9b"], default: null, source: "qwen", cloud: CLOUD.slice(0, 2) });
    act(() => prices({ date: "2026-12-01", source: "https://ollama.com/pricing", saved: true, fetchedAtMs: 1_796_117_400_000, models: 25, refreshAllowed: true }));
    await waitFor(() => expect(groupedModels()).toEqual(["qwen3.8:27b", "qwen3.9:9b", "glm-5.3-flash:cloud", "gpt-oss:120b-cloud"]));
    expect(api.listAgentModels).toHaveBeenCalledTimes(2);
    expect(await within(control).findByText("Стоимость облачных запусков через Ollama — оценка по ценам Ollama Cloud на 01.12.2026, загруженным с ollama.com/pricing")).toBeInTheDocument();
    expect(within(control).getByText("Список моделей обновлён · Цены обновлены: моделей 25")).toBeInTheDocument();
    expect(button).toHaveTextContent("Обновить модели и цены");
    expect(button).not.toHaveAttribute("aria-disabled");
  });

  it("«Обновить модели и цены»: политика запрещает загрузку цен — кнопка перечитывает только список; ошибка цен — список всё равно перечитан", async () => {
    vi.mocked(api.getOllamaPrices).mockResolvedValue({ date: "2026-11-07", source: "https://ollama.com/pricing", saved: false, models: 24, refreshAllowed: false });
    const user = userEvent.setup();
    const first = renderWithClient(<AgentSection status={mcpStatus()} />);
    const button = await screen.findByRole("button", { name: "Обновить модели и цены" });
    await waitFor(() => expect(button).toHaveAccessibleDescription("Политика организации не разрешает загружать цены: оценки — по этой таблице"));
    expect(button).toBeEnabled();
    await waitFor(() => expect(api.listAgentModels).toHaveBeenCalledTimes(1));
    await user.click(button);
    expect(await screen.findByText("Список моделей обновлён")).toBeInTheDocument();
    expect(api.listAgentModels).toHaveBeenCalledTimes(2);
    expect(api.refreshOllamaPrices).not.toHaveBeenCalled();
    first.unmount();
    vi.mocked(api.listAgentModels).mockClear();
    vi.mocked(api.getOllamaPrices).mockResolvedValue({ date: "2026-11-07", source: "https://ollama.com/pricing", saved: false, models: 24, refreshAllowed: true });
    vi.mocked(api.refreshOllamaPrices).mockRejectedValue({ code: "INTERNAL", key: "errors.agent.pricesParse", message: "Could not parse the Ollama price page", retryable: false });
    renderWithClient(<AgentSection status={mcpStatus()} />);
    await waitFor(() => expect(api.listAgentModels).toHaveBeenCalledTimes(1));
    await user.click(await screen.findByRole("button", { name: "Обновить модели и цены" }));
    expect(await screen.findByText("Не удалось разобрать страницу цен Ollama")).toBeInTheDocument();
    await waitFor(() => expect(api.listAgentModels).toHaveBeenCalledTimes(2));
    expect(screen.getByText("Стоимость облачных запусков через Ollama — оценка по ценам Ollama Cloud на 07.11.2026, встроенным в Glafira")).toBeInTheDocument();
  });

  it("список моделей не прочитан — пояснение, имя вводится через «Другая модель…»", async () => {
    vi.mocked(api.listAgentModels).mockRejectedValue({ code: "INTERNAL", message: "boom", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={mcpStatus()} />);
    expect(await screen.findByText("Список моделей не прочитан: введите имя вручную")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Модель" })).toBeEnabled();
    expect(screen.getByRole("combobox", { name: "Модель" })).toHaveAccessibleDescription("Список моделей не прочитан: введите имя вручную");
    const input = await typeModel(user, "qwen3.8:27b");
    expect(input).toHaveValue("qwen3.8:27b");
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
    // Запоздалый ответ и обновления после него — внутри act, пока тест не кончился.
    await act(async () => {
      answer({ path: "/usr/local/bin/qwen", version: "0.25.1", belowMinimum: true, error: null });
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(api.detectAgentCli).toHaveBeenCalledTimes(1);
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

  it("итог «Проверить» убирается, когда в сохранённых настройках сменились агент, программа или модель; другие поля его не трогают", async () => {
    vi.mocked(api.getAgentSettings).mockResolvedValue(configured);
    vi.mocked(api.setAgentSettings).mockImplementation(async (s) => s);
    const check = agentRun({ id: "a0c0c0c", kind: "check", requestId: null, connectionId: null, queuedAtMs: 10 });
    vi.mocked(api.checkAgent).mockResolvedValue(check);
    const user = userEvent.setup();
    renderWithClient(<AgentSection status={ready()} />);
    const button = await screen.findByRole("button", { name: "Проверить" });
    await waitFor(() => expect(button).toBeEnabled());
    const passed = async (run = check) => {
      await user.click(screen.getByRole("button", { name: "Проверить" }));
      await waitFor(() => expect(listeners("agent://runs").length).toBeGreaterThan(0));
      emitEvent("agent://runs", { ...run, status: "done", startedAtMs: 1_000, endedAtMs: 24_000, calls: 1 });
      expect(await screen.findByText("Работает: Qwen Code 0.26.5, qwen3.6-64k:35b-a3b, 23 с")).toBeInTheDocument();
    };
    const save = async () => {
      await user.click(screen.getByRole("button", { name: "Сохранить" }));
      expect(await screen.findByText("Сохранено")).toBeInTheDocument();
    };
    await passed();
    // Срок запуска — не агент, не программа и не модель: итог остаётся.
    const timeout = screen.getByRole("textbox", { name: "Срок запуска, мин" });
    await user.clear(timeout);
    await user.type(timeout, "20");
    // Несохранённая правка итог не убирает: «Проверить» просит сохранить.
    expect(screen.getByText("Работает: Qwen Code 0.26.5, qwen3.6-64k:35b-a3b, 23 с")).toBeInTheDocument();
    await save();
    expect(screen.getByText("Работает: Qwen Code 0.26.5, qwen3.6-64k:35b-a3b, 23 с")).toBeInTheDocument();
    // Модель сменилась и сохранена — итог о прежних настройках убран.
    await typeModel(user, "glm-5.3-flash:cloud");
    await save();
    expect(screen.queryByText(/^Работает:/)).toBeNull();
    // Вернули прежнюю модель — итог не возвращается: его даёт только новая проверка.
    await user.selectOptions(screen.getByRole("combobox", { name: "Модель" }), "");
    await save();
    expect(screen.queryByText(/^Работает:/)).toBeNull();
    // Программа: новая проверка, затем путь сменился — итог убран; «Не работает» с журналом — тоже.
    vi.mocked(api.checkAgent).mockResolvedValue({ ...check, id: "a0c0c0d", queuedAtMs: 11 });
    await user.click(screen.getByRole("button", { name: "Проверить" }));
    emitEvent("agent://runs", { ...check, id: "a0c0c0d", queuedAtMs: 11, status: "failed", reason: "notConnected", startedAtMs: 1_000, endedAtMs: 5_000 });
    expect(await screen.findByText("Не работает: агент не подключился к Glafira")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Показать журнал проверки" })).toBeInTheDocument();
    const path = screen.getByRole("textbox", { name: "Программа" });
    await user.clear(path);
    await user.type(path, "/usr/local/bin/qwen");
    await save();
    expect(screen.queryByText(/^Не работает:/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Показать журнал проверки" })).toBeNull();
    // Агент: Qwen Code → Claude Code — тоже.
    vi.mocked(api.checkAgent).mockResolvedValue({ ...check, id: "a0c0c0e", queuedAtMs: 12 });
    await passed({ ...check, id: "a0c0c0e", queuedAtMs: 12 });
    await user.click(screen.getByRole("radio", { name: "Claude Code" }));
    await user.type(screen.getByRole("textbox", { name: "Программа" }), "/Users/me/.local/bin/claude");
    await save();
    expect(screen.queryByText(/^Работает:/)).toBeNull();
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
});
