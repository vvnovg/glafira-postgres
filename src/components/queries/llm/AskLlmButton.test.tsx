import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentStatus, AppError, LlmAccess, McpStatus } from "../../../api/types";
import { useShownProposals } from "../../../state/shownProposals";
import { emitEvent, listeners, resetEventBus } from "../../../test/events";
import { connection } from "../../../test/fixtures";
import { agentRun, optimizationRequest } from "../../../test/llm";
import { agentStatus, mcpStatus } from "../../../test/mcp";
import { planTree } from "../../../test/plans";
import { renderWithClient } from "../../../test/render";

const mcp = vi.hoisted(() => ({ value: { status: undefined as unknown, loading: false, error: null as unknown, refetch: () => {} } }));
vi.mock("../../../lib/useMcpStatus", () => ({ useMcpStatus: () => mcp.value }));
vi.mock("@tauri-apps/api/event", async () => (await import("../../../test/events")).tauriEventMock());
vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return {
    ...orig,
    api: {
      listConnections: vi.fn(),
      listOptimizationRequests: vi.fn(),
      listLlmProposals: vi.fn(),
      createOptimizationRequest: vi.fn(),
      cancelOptimizationRequest: vi.fn(),
      listAgentRuns: vi.fn(),
      startAgentRun: vi.fn(),
    },
  };
});

import { api } from "../../../api/client";
import { AskLlmButton, type SentNotice } from "./AskLlmButton";

const SQL = "SELECT count(*) FROM shop.orders WHERE status = $1";
const props = { connectionId: "c", member: "pg-1", database: "perf", source: { kind: "console" } as const, sql: SQL, params: ["disputed"], plan: null };
const LINE = "Оптимизируй запрос r3a9f1c из Glafira";
const NOTICE = "Запрос r3a9f1c передан. Скажите ассистенту «оптимизируй запрос из Glafira» — он заберёт его сам (строка с id уже в буфере обмена).";
const LABEL = "Передать LLM-ассистенту";
const refetch = vi.fn();

function setStatus(status: McpStatus | undefined, extra: { loading?: boolean; error?: AppError | null } = {}) {
  mcp.value = { status, loading: extra.loading ?? false, error: extra.error ?? null, refetch };
}

describe("AskLlmButton", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    resetEventBus();
    setStatus(mcpStatus());
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", llmAccess: "diagnostics" })]);
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([]);
    vi.mocked(api.listLlmProposals).mockResolvedValue([]);
    vi.mocked(api.listAgentRuns).mockResolvedValue([]);
  });

  it("пока статус MCP не прочитан, кнопка неактивна и без пояснения", async () => {
    setStatus(undefined, { loading: true });
    renderWithClient(<AskLlmButton {...props} />);
    await waitFor(() => expect(api.listConnections).toHaveBeenCalled());
    expect(screen.getByRole("button", { name: LABEL })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Повторить" })).toBeNull();
    expect(screen.queryByText("Статус MCP недоступен")).toBeNull();
  });

  it("статус MCP не загрузился: кнопка неактивна, об этом сказано и есть «Повторить»", async () => {
    setStatus(undefined, { error: { code: "INTERNAL", message: "boom", retryable: true } });
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    expect(await screen.findByText("Статус MCP недоступен")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: LABEL })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(refetch).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: `Подсказка: ${LABEL}` }));
    await waitFor(() => expect(screen.getByRole("tooltip")).toHaveTextContent(/Статус MCP не прочитался/));
  });

  it.each<[string, Partial<McpStatus>, LlmAccess, RegExp]>([
    ["политика", { policyEnabled: false }, "diagnostics", /отключён политикой вашей организации/],
    ["политика важнее выключенного сервера", { policyEnabled: false, server: "stopped" }, "diagnostics", /отключён политикой вашей организации/],
    ["сервер выключен", { server: "stopped" }, "diagnostics", /включите его в «Настройках» → «LLM-ассистенты \(MCP\)»/],
    ["сервер в другом окне", { server: "otherInstance" }, "diagnostics", /работает в другом окне Glafira/],
    ["сервер не запустился", { server: "error", error: "bind failed" }, "diagnostics", /не запустился/],
    ["подключение закрыто", {}, "off", /Подключение закрыто для LLM-ассистентов/],
  ])("неактивна, подсказка объясняет причину: %s", async (_name, patch, access, text) => {
    setStatus(mcpStatus(patch));
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", llmAccess: access })]);
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    await user.click(screen.getByRole("button", { name: `Подсказка: ${LABEL}` }));
    await waitFor(() => expect(screen.getByRole("tooltip")).toHaveTextContent(text));
    expect(screen.getByRole("button", { name: LABEL })).toBeDisabled();
  });

  it("уровень профиля берётся через действующий уровень: политика «ничего» гасит даже data", async () => {
    setStatus(mcpStatus({ policyEnabled: false, maxAccess: "diagnostics" }));
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", llmAccess: "data" })]);
    renderWithClient(<AskLlmButton {...props} />);
    await waitFor(() => expect(api.listConnections).toHaveBeenCalled());
    expect(screen.getByRole("button", { name: LABEL })).toBeDisabled();
  });

  it("подсказка активной кнопки говорит, что снимок уходит без маскирования", async () => {
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    await waitFor(() => expect(screen.getByRole("button", { name: LABEL })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: `Подсказка: ${LABEL}` }));
    expect(screen.getByRole("tooltip")).toHaveTextContent(/без маскирования даже на уровне «Диагностика»/);
    // Z5: в отличие от «Скопировать для LLM» — запрос, который ассистент с MCP забирает сам и отвечает в панель.
    expect(screen.getByRole("tooltip")).toHaveTextContent(/ассистент с MCP заберёт его сам/);
    expect(screen.getByRole("tooltip")).toHaveTextContent(/«Предложения LLM»/);
    // #5: что именно уходит со снимком.
    expect(screen.getByRole("tooltip")).toHaveTextContent("(текст запроса, статистика, план, DDL)");
  });

  it("читает только запросы подключения, предложения не запрашивает; без режима фона и запусков — и запуски не читает", async () => {
    renderWithClient(<AskLlmButton {...props} />);
    await waitFor(() => expect(screen.getByRole("button", { name: LABEL })).toBeEnabled());
    expect(api.listOptimizationRequests).toHaveBeenCalledWith("c");
    expect(api.listLlmProposals).not.toHaveBeenCalled();
    expect(api.listAgentRuns).not.toHaveBeenCalled();
    expect(listeners("agent://runs")).toHaveLength(0);
  });

  it("запросы ещё читаются — кнопка неактивна", async () => {
    vi.mocked(api.listOptimizationRequests).mockReturnValue(new Promise(() => {}));
    renderWithClient(<AskLlmButton {...props} />);
    await waitFor(() => expect(api.listConnections).toHaveBeenCalled());
    expect(screen.getByRole("button", { name: LABEL })).toBeDisabled();
  });

  it("запросы не прочитались: это не «ничего не ждёт» — кнопка неактивна, есть «Повторить»", async () => {
    vi.mocked(api.listOptimizationRequests).mockRejectedValueOnce({ code: "INTERNAL", message: "disk", retryable: true });
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    expect(await screen.findByText("Не удалось прочитать запросы этого подключения")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: LABEL })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    await waitFor(() => expect(screen.getByRole("button", { name: LABEL })).toBeEnabled());
    expect(screen.queryByText("Не удалось прочитать запросы этого подключения")).toBeNull();
  });

  it("создаёт запрос с планом и параметрами, копирует строку для чата и показывает ожидание", async () => {
    const plan = planTree();
    vi.mocked(api.createOptimizationRequest).mockResolvedValue(optimizationRequest());
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} plan={plan} />);
    const button = screen.getByRole("button", { name: LABEL });
    await waitFor(() => expect(button).toBeEnabled());
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    await user.click(button);
    expect(api.createOptimizationRequest).toHaveBeenCalledWith("c", { member: "pg-1", database: "perf", source: { kind: "console" }, sql: SQL, params: ["disputed"], plan });
    expect(await navigator.clipboard.readText()).toBe(LINE);
    expect(await screen.findByText(NOTICE)).toBeInTheDocument();
    expect(await screen.findByText("Ждёт ответа LLM: запрос r3a9f1c")).toBeInTheDocument();
    // Пока запрос ждёт, второй не создаётся.
    expect(button).toBeDisabled();
  });

  it("Z1: ожидание (с «Отменить запрос») и уведомление о переданном запросе — отдельные строки, не одна фраза", async () => {
    vi.mocked(api.createOptimizationRequest).mockResolvedValue(optimizationRequest());
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    const button = screen.getByRole("button", { name: LABEL });
    await waitFor(() => expect(button).toBeEnabled());
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    await user.click(button);
    const waiting = await screen.findByText("Ждёт ответа LLM: запрос r3a9f1c");
    const notice = screen.getByText(NOTICE);
    const waitingLine = waiting.closest(".llm-ask-line") as HTMLElement;
    expect(waitingLine).not.toBeNull();
    expect(waitingLine).toContainElement(screen.getByRole("button", { name: "Отменить запрос" }));
    expect(waitingLine).not.toContainElement(notice);
    expect(notice).not.toContainElement(waiting);
    expect(notice.textContent).toBe(NOTICE);
    // Обе строки — в одном блоке под кнопками.
    expect(waitingLine.parentElement).toContainElement(notice);
    expect(waitingLine.parentElement).toHaveClass("llm-ask-notes", "shown");
  });

  it("Z8: элементы ряда (`extra`) — сразу после кнопки и подсказки, до строк уведомлений", async () => {
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    renderWithClient(
      <div className="toolbar" data-testid="row">
        <AskLlmButton {...props} extra={<button type="button">extra</button>} />
      </div>,
    );
    const waiting = await screen.findByText("Ждёт ответа LLM: запрос r3a9f1c");
    const extra = screen.getByRole("button", { name: "extra" });
    expect(screen.getByRole("button", { name: `Подсказка: ${LABEL}` }).compareDocumentPosition(extra) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(extra.compareDocumentPosition(waiting) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(waiting.closest(".llm-ask-notes")).not.toContainElement(extra);
  });

  it("без уведомлений блок строк не занимает ряд", async () => {
    const { container } = renderWithClient(<AskLlmButton {...props} />);
    await waitFor(() => expect(screen.getByRole("button", { name: LABEL })).toBeEnabled());
    const notes = container.querySelector(".llm-ask-notes")!;
    expect(notes).not.toHaveClass("shown");
    expect(notes).toHaveTextContent("");
  });

  it("буфер недоступен — уведомление просит набрать строку руками", async () => {
    vi.mocked(api.createOptimizationRequest).mockResolvedValue(optimizationRequest());
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    const button = screen.getByRole("button", { name: LABEL });
    await waitFor(() => expect(button).toBeEnabled());
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("denied"));
    await user.click(button);
    expect(await screen.findByText(`Запрос r3a9f1c передан. Скажите ассистенту «оптимизируй запрос из Glafira» — он заберёт его сам (скопировать строку с id не удалось: «${LINE}»).`)).toBeInTheDocument();
  });

  it("ожидающий запрос своего источника: «Отменить запрос»; запрос карточки консоль не показывает", async () => {
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest({ id: "r1" }), optimizationRequest({ id: "r2", source: { kind: "query", queryId: "7" }, createdAtMs: 1_800_000_000_000 })]);
    vi.mocked(api.cancelOptimizationRequest).mockResolvedValue(optimizationRequest({ id: "r1", status: "cancelled" }));
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    expect(await screen.findByText("Ждёт ответа LLM: запрос r1")).toBeInTheDocument();
    expect(screen.queryByText(/запрос r2/)).toBeNull();
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest({ id: "r1", status: "cancelled" })]);
    await user.click(screen.getByRole("button", { name: "Отменить запрос" }));
    expect(api.cancelOptimizationRequest).toHaveBeenCalledWith("r1");
    await waitFor(() => expect(screen.queryByText("Ждёт ответа LLM: запрос r1")).toBeNull());
    expect(screen.getByRole("button", { name: LABEL })).toBeEnabled();
  });

  it("ошибки создания и отмены показываются текстом словаря в области статуса", async () => {
    vi.mocked(api.createOptimizationRequest).mockRejectedValue({ code: "NOT_AUTHORIZED", key: "errors.explain.notAuthorized", message: "Not enough privileges", retryable: false });
    const user = userEvent.setup();
    const { rerender } = renderWithClient(<AskLlmButton {...props} />);
    const button = screen.getByRole("button", { name: LABEL });
    await waitFor(() => expect(button).toBeEnabled());
    await user.click(button);
    const failure = await screen.findByText(/Не хватает прав на таблицы запроса/);
    expect(failure).toHaveAttribute("role", "status");
    // Снимок длиннее 64 КБ: Rust отказывает ключом errors.llm.sqlTooLong.
    vi.mocked(api.createOptimizationRequest).mockRejectedValue({ code: "INVALID_INPUT", key: "errors.llm.sqlTooLong", params: { max: "65536" }, message: "too long", retryable: false });
    await user.click(screen.getByRole("button", { name: LABEL }));
    expect(await screen.findByText(/Запрос длиннее 65536 байт: его нельзя передать кнопкой «Передать LLM-ассистенту»/)).toBeInTheDocument();
    rerender(<AskLlmButton {...props} sql="  " />);
    expect(screen.getByRole("button", { name: LABEL })).toBeDisabled();
  });

  it("статус ожидания и пояснения о нечитаемом состоянии лежат внутри живой области, кнопки — снаружи", async () => {
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    const { unmount } = renderWithClient(<AskLlmButton {...props} />);
    // Регион, где лежит текст; ни одна кнопка не внутри живой области.
    const region = (el: HTMLElement) => el.closest('[role="status"]');
    const noButtonInRegions = () => {
      for (const r of screen.getAllByRole("status")) expect(within(r).queryAllByRole("button")).toHaveLength(0);
    };
    const waitingText = await screen.findByText("Ждёт ответа LLM: запрос r3a9f1c");
    expect(region(waitingText)).not.toBeNull();
    expect(region(waitingText)).not.toContainElement(screen.getByRole("button", { name: "Отменить запрос" }));
    noButtonInRegions();
    unmount();
    setStatus(undefined, { error: { code: "INTERNAL", message: "boom", retryable: true } });
    const second = renderWithClient(<AskLlmButton {...props} />);
    const unavailable = await screen.findByText("Статус MCP недоступен");
    expect(region(unavailable)).not.toBeNull();
    expect(region(unavailable)).not.toContainElement(screen.getByRole("button", { name: "Повторить" }));
    noButtonInRegions();
    second.unmount();
    setStatus(mcpStatus());
    vi.mocked(api.listOptimizationRequests).mockRejectedValue({ code: "INTERNAL", message: "disk", retryable: true });
    renderWithClient(<AskLlmButton {...props} />);
    expect(region(await screen.findByText("Не удалось прочитать запросы этого подключения"))).not.toBeNull();
  });

  it("быстрый второй клик между созданием и перечитыванием списка не создаёт дубль", async () => {
    vi.mocked(api.createOptimizationRequest).mockResolvedValue(optimizationRequest());
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    const button = screen.getByRole("button", { name: LABEL });
    await waitFor(() => expect(button).toBeEnabled());
    // Список ещё не знает о новом запросе (остаётся пустым).
    await user.click(button);
    await screen.findByText(/^Запрос r3a9f1c передан\./);
    expect(button).toBeDisabled();
    await user.click(button);
    expect(api.createOptimizationRequest).toHaveBeenCalledTimes(1);
  });

  it("уведомление можно держать снаружи: после перемонтирования кнопки оно остаётся", async () => {
    vi.mocked(api.createOptimizationRequest).mockResolvedValue(optimizationRequest());
    const user = userEvent.setup();
    function Host({ shown }: { shown: boolean }) {
      const [sent, setSent] = useState<SentNotice | null>(null);
      return shown ? <AskLlmButton {...props} sent={sent} onSent={setSent} /> : <AskLlmButton key="b" {...props} sent={sent} onSent={setSent} />;
    }
    const { rerender } = renderWithClient(<Host shown />);
    const button = screen.getByRole("button", { name: LABEL });
    await waitFor(() => expect(button).toBeEnabled());
    await user.click(button);
    await screen.findByText(/^Запрос r3a9f1c передан\./);
    rerender(<Host shown={false} />);
    expect(screen.getByText(/^Запрос r3a9f1c передан\./)).toBeInTheDocument();
  });

  it("области уведомлений присутствуют всегда (живые области): строка ожидания и строка о переданном запросе", () => {
    renderWithClient(<AskLlmButton {...props} />);
    expect(screen.getAllByRole("status")).toHaveLength(2);
  });
});

describe("AskLlmButton: фоновый ассистент", () => {
  /** Режим фона включён, агент настроен; поля — поверх. */
  const bg = (patch: Partial<AgentStatus> = {}) => setStatus(mcpStatus({ agent: agentStatus({ enabled: true, configured: true, ...patch }) }));
  const group = () => screen.findByRole("group", { name: "Фоновый ассистент: запрос r3a9f1c" });
  const QUEUED = "Запрос r3a9f1c передан фоновому ассистенту";

  beforeEach(() => {
    vi.resetAllMocks();
    resetEventBus();
    bg();
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", llmAccess: "diagnostics" })]);
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([]);
    vi.mocked(api.listLlmProposals).mockResolvedValue([]);
    vi.mocked(api.listAgentRuns).mockResolvedValue([]);
  });

  it("создаёт запрос и ставит запуск в очередь; буфер не трогает", async () => {
    vi.mocked(api.createOptimizationRequest).mockResolvedValue(optimizationRequest());
    vi.mocked(api.startAgentRun).mockResolvedValue(agentRun());
    const user = userEvent.setup();
    await navigator.clipboard.writeText("прежнее");
    renderWithClient(<AskLlmButton {...props} />);
    const button = screen.getByRole("button", { name: LABEL });
    await waitFor(() => expect(button).toBeEnabled());
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    vi.mocked(api.listAgentRuns).mockResolvedValue([agentRun()]);
    await user.click(button);
    expect(api.createOptimizationRequest).toHaveBeenCalledWith("c", { member: "pg-1", database: "perf", source: { kind: "console" }, sql: SQL, params: ["disputed"], plan: null });
    expect(api.startAgentRun).toHaveBeenCalledWith("r3a9f1c");
    expect(await navigator.clipboard.readText()).toBe("прежнее");
    expect(await screen.findByText(QUEUED)).toBeInTheDocument();
    expect(within(await group()).getByText("В очереди фонового ассистента: следующий")).toBeInTheDocument();
    // Строку ожидания P4b заменяет строка статуса фонового ассистента.
    expect(screen.queryByText("Ждёт ответа LLM: запрос r3a9f1c")).toBeNull();
    expect(button).toBeDisabled();
  });

  it("Z1: строка статуса запуска и уведомление — отдельные строки в блоке под кнопками; элементы ряда (`extra`) — до них", async () => {
    vi.mocked(api.createOptimizationRequest).mockResolvedValue(optimizationRequest());
    vi.mocked(api.startAgentRun).mockResolvedValue(agentRun());
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} extra={<button type="button">extra</button>} />);
    const button = screen.getByRole("button", { name: LABEL });
    await waitFor(() => expect(button).toBeEnabled());
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    vi.mocked(api.listAgentRuns).mockResolvedValue([agentRun()]);
    await user.click(button);
    const box = await group();
    const notice = screen.getByText(QUEUED);
    expect(box).not.toContainElement(notice);
    expect(notice).toHaveAttribute("role", "status");
    const notes = box.closest(".llm-ask-notes") as HTMLElement;
    expect(notes).toHaveClass("shown");
    expect(notes).toContainElement(notice);
    const extra = screen.getByRole("button", { name: "extra" });
    expect(notes).not.toContainElement(extra);
    expect(extra.compareDocumentPosition(box) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // Кнопки строки статуса — вне живых областей.
    for (const region of screen.getAllByRole("status")) expect(within(region).queryAllByRole("button")).toHaveLength(0);
  });

  it("подсказка называет агента и говорит, что снимок уходит модели агента без маскирования", async () => {
    bg({ preset: "claude" });
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    await waitFor(() => expect(screen.getByRole("button", { name: LABEL })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: `Подсказка: ${LABEL}` }));
    expect(screen.getByRole("tooltip")).toHaveTextContent(
      "Glafira запустит Claude Code в фоне; предложения появятся в панели «Предложения LLM». Снимок уходит модели агента без маскирования.",
    );
  });

  it("причина неактивности важнее режима фона: подсказка объясняет её", async () => {
    setStatus(mcpStatus({ server: "stopped", agent: agentStatus({ enabled: true, configured: true }) }));
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    await user.click(screen.getByRole("button", { name: `Подсказка: ${LABEL}` }));
    await waitFor(() => expect(screen.getByRole("tooltip")).toHaveTextContent(/включите его в «Настройках»/));
    expect(screen.getByRole("button", { name: LABEL })).toBeDisabled();
  });

  it.each<[string, Partial<AgentStatus>]>([
    ["агент не настроен", { configured: false }],
    ["режим выключен", { enabled: false }],
    ["фон запрещён политикой", { policyEnabled: false }],
  ])("%s — как в P4b: строка для чата в буфере, запуска нет", async (_name, patch) => {
    bg(patch);
    vi.mocked(api.createOptimizationRequest).mockResolvedValue(optimizationRequest());
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    const button = screen.getByRole("button", { name: LABEL });
    await waitFor(() => expect(button).toBeEnabled());
    await user.click(button);
    expect(await navigator.clipboard.readText()).toBe(LINE);
    expect(await screen.findByText(NOTICE)).toBeInTheDocument();
    expect(api.startAgentRun).not.toHaveBeenCalled();
    expect(api.listAgentRuns).not.toHaveBeenCalled();
  });

  it("ход работы приходит событием agent://runs: работает → закончил без предложений", async () => {
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    vi.mocked(api.listAgentRuns).mockResolvedValue([agentRun({ status: "running", startedAtMs: Date.now() - 83_000, calls: 4, lastTool: "hypothetical_index" })]);
    renderWithClient(<AskLlmButton {...props} />);
    const box = await group();
    expect(within(box).getByText("Ассистент работает")).toBeInTheDocument();
    expect(within(box).getByText(/· вызовов: 4 · hypothetical_index$/)).toBeInTheDocument();
    await waitFor(() => expect(listeners("agent://runs").length).toBeGreaterThan(0));
    emitEvent("agent://runs", agentRun({ status: "done", startedAtMs: 1, endedAtMs: 2, calls: 7, summary: "Запрос уже использует индекс" }));
    expect(await within(box).findByText("Ассистент закончил без предложений")).toBeInTheDocument();
    expect(within(box).getByRole("button", { name: "Ответ ассистента" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: LABEL })).toBeDisabled();
    expect(api.listAgentRuns).toHaveBeenCalledTimes(1);
  });

  it("очередь общая для всех подключений: перед запуском — идущий запуск другого подключения", async () => {
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    vi.mocked(api.listAgentRuns).mockResolvedValue([
      agentRun({ id: "a000001", connectionId: "other", requestId: "r000001", status: "running", startedAtMs: 1, queuedAtMs: 1 }),
      agentRun({ queuedAtMs: 2 }),
    ]);
    renderWithClient(<AskLlmButton {...props} />);
    expect(await screen.findByText("В очереди фонового ассистента, перед ним: 1")).toBeInTheDocument();
  });

  it("«Повторить» после ошибки ставит новый запуск; отказ очереди — текстом в области статуса", async () => {
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    vi.mocked(api.listAgentRuns).mockResolvedValue([agentRun({ status: "failed", reason: "notConnected", startedAtMs: 1, endedAtMs: 2 })]);
    vi.mocked(api.startAgentRun)
      .mockRejectedValueOnce({ code: "INVALID_INPUT", message: "The background assistant queue is full", retryable: false })
      .mockResolvedValue(agentRun({ id: "a2b3c4d", queuedAtMs: 1_700_000_100_000 }));
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    expect(await screen.findByText("Ошибка фонового ассистента: агент не подключился к Glafira")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(api.startAgentRun).toHaveBeenLastCalledWith("r3a9f1c");
    const failure = await screen.findByText("The background assistant queue is full");
    expect(failure).toHaveAttribute("role", "status");
    vi.mocked(api.listAgentRuns).mockResolvedValue([
      agentRun({ id: "a2b3c4d", queuedAtMs: 1_700_000_100_000 }),
      agentRun({ status: "failed", reason: "notConnected", startedAtMs: 1, endedAtMs: 2 }),
    ]);
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(await screen.findByText("В очереди фонового ассистента: следующий")).toBeInTheDocument();
    expect(screen.queryByText("The background assistant queue is full")).toBeNull();
    // Кнопка «Повторить» исчезла — фокус на строке статуса, а не на <body>.
    expect(await group()).toHaveFocus();
  });

  it("запуск не встал в очередь при создании: запрос ждёт, ошибка — вместо уведомления, в строке — «Запустить фонового ассистента»", async () => {
    vi.mocked(api.createOptimizationRequest).mockResolvedValue(optimizationRequest());
    vi.mocked(api.startAgentRun).mockRejectedValueOnce({ code: "INVALID_INPUT", message: "The background assistant queue is full", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    const button = screen.getByRole("button", { name: LABEL });
    await waitFor(() => expect(button).toBeEnabled());
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    await user.click(button);
    expect(await screen.findByText("The background assistant queue is full")).toBeInTheDocument();
    expect(screen.queryByText(QUEUED)).toBeNull();
    const box = await group();
    expect(within(box).getByText("Ждёт ответа LLM: запрос r3a9f1c")).toBeInTheDocument();
    vi.mocked(api.startAgentRun).mockResolvedValue(agentRun());
    vi.mocked(api.listAgentRuns).mockResolvedValue([agentRun()]);
    await user.click(within(box).getByRole("button", { name: "Запустить фонового ассистента" }));
    expect(await within(box).findByText("В очереди фонового ассистента: следующий")).toBeInTheDocument();
  });

  it("«Отменить запрос» во время работы отменяет запрос (Rust останавливает и запуск)", async () => {
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    vi.mocked(api.listAgentRuns).mockResolvedValue([agentRun({ status: "running", startedAtMs: Date.now() })]);
    vi.mocked(api.cancelOptimizationRequest).mockResolvedValue(optimizationRequest({ status: "cancelled" }));
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    const box = await group();
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest({ status: "cancelled" })]);
    vi.mocked(api.listAgentRuns).mockResolvedValue([agentRun({ status: "cancelled", startedAtMs: 1, endedAtMs: 2 })]);
    await user.click(within(box).getByRole("button", { name: "Отменить запрос" }));
    expect(api.cancelOptimizationRequest).toHaveBeenCalledWith("r3a9f1c");
    await waitFor(() => expect(screen.queryByRole("group")).toBeNull());
    expect(screen.getByRole("button", { name: LABEL })).toBeEnabled();
  });

  it("режим выключили во время запуска: статус запуска виден до конца", async () => {
    bg({ enabled: false, running: { runId: "a1b2c3d", requestId: "r3a9f1c" } });
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    vi.mocked(api.listAgentRuns).mockResolvedValue([agentRun({ status: "running", startedAtMs: Date.now() })]);
    renderWithClient(<AskLlmButton {...props} />);
    expect(within(await group()).getByText("Ассистент работает")).toBeInTheDocument();
  });

  it("режим выключен, а запуск запроса уже кончился — строка P4b, без «Повторить» запуска", async () => {
    bg({ enabled: false, queued: 1 });
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    vi.mocked(api.listAgentRuns).mockResolvedValue([agentRun({ status: "failed", reason: "exit", exitCode: 1, startedAtMs: 1, endedAtMs: 2 })]);
    renderWithClient(<AskLlmButton {...props} />);
    expect(await screen.findByText("Ждёт ответа LLM: запрос r3a9f1c")).toBeInTheDocument();
    await waitFor(() => expect(api.listAgentRuns).toHaveBeenCalled());
    expect(screen.queryByRole("group")).toBeNull();
    expect(screen.queryByRole("button", { name: "Повторить" })).toBeNull();
  });

  it("запуски не прочитались: строка ожидания P4b, пояснение и «Повторить»", async () => {
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest()]);
    vi.mocked(api.listAgentRuns).mockRejectedValueOnce({ code: "INTERNAL", message: "disk", retryable: true });
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    expect(await screen.findByText("Не удалось прочитать запуски фонового ассистента")).toBeInTheDocument();
    expect(screen.getByText("Ждёт ответа LLM: запрос r3a9f1c")).toBeInTheDocument();
    vi.mocked(api.listAgentRuns).mockResolvedValue([agentRun()]);
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(await screen.findByText("В очереди фонового ассистента: следующий")).toBeInTheDocument();
    expect(screen.queryByText("Не удалось прочитать запуски фонового ассистента")).toBeNull();
  });

  it("запуски не прочитались, а ждущего запроса нет — кнопке это не мешает, пояснения нет", async () => {
    vi.mocked(api.listAgentRuns).mockRejectedValue({ code: "INTERNAL", message: "disk", retryable: true });
    renderWithClient(<AskLlmButton {...props} />);
    await waitFor(() => expect(screen.getByRole("button", { name: LABEL })).toBeEnabled());
    await waitFor(() => expect(api.listAgentRuns).toHaveBeenCalled());
    expect(screen.queryByText("Не удалось прочитать запуски фонового ассистента")).toBeNull();
  });

  it.each<[string, Parameters<typeof agentRun>[0]]>([
    ["предложения уже показаны панелью", { status: "done", proposalIds: ["p1"], startedAtMs: 1, endedAtMs: 2 }],
    ["запуск пропущен: ответил чат", { status: "skipped" }],
    ["запуск упал после ответа", { status: "failed", reason: "exit", exitCode: 1, startedAtMs: 1, endedAtMs: 2 }],
  ])("отвеченный запрос, строке статуса нечего сказать (%s) — блок строк не занимает ряд", async (_name, patch) => {
    useShownProposals.setState({ shown: { p1: true } });
    try {
      vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest({ status: "answered", proposalIds: ["p1"] })]);
      vi.mocked(api.listAgentRuns).mockResolvedValue([agentRun(patch)]);
      const { container, client } = renderWithClient(<AskLlmButton {...props} />);
      await waitFor(() => expect(screen.getByRole("button", { name: LABEL })).toBeEnabled());
      // Запуски прочитаны: иначе строки статуса не было бы и без правки.
      await waitFor(() => expect(client.getQueryData(["agentRuns"])).toBeDefined());
      const notes = container.querySelector(".llm-ask-notes")!;
      expect(notes).not.toHaveClass("shown");
      expect(notes).toHaveTextContent("");
      expect(screen.queryByRole("group", { name: "Фоновый ассистент: запрос r3a9f1c" })).toBeNull();
    } finally {
      // Упавший тест не должен оставить «показанные» предложения соседям.
      useShownProposals.setState({ shown: {} });
    }
  });

  it("отвеченный запрос: «Ассистент прислал N предложений», пока панель их не показала; кнопка снова активна", async () => {
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([optimizationRequest({ status: "answered", proposalIds: ["p1"] })]);
    vi.mocked(api.listAgentRuns).mockResolvedValue([agentRun({ status: "done", proposalIds: ["p1"], startedAtMs: 1, endedAtMs: 2 })]);
    renderWithClient(<AskLlmButton {...props} />);
    expect(within(await group()).getByText("Ассистент прислал 1 предложение")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: LABEL })).toBeEnabled();
  });
});
