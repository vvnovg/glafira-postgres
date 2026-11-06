import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppError, LlmAccess, McpStatus } from "../../../api/types";
import { resetEventBus } from "../../../test/events";
import { connection } from "../../../test/fixtures";
import { optimizationRequest } from "../../../test/llm";
import { mcpStatus } from "../../../test/mcp";
import { planTree } from "../../../test/plans";
import { renderWithClient } from "../../../test/render";

const mcp = vi.hoisted(() => ({ value: { status: undefined as unknown, loading: false, error: null as unknown, refetch: () => {} } }));
vi.mock("../../../lib/useMcpStatus", () => ({ useMcpStatus: () => mcp.value }));
vi.mock("@tauri-apps/api/event", async () => (await import("../../../test/events")).tauriEventMock());
vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return {
    ...orig,
    api: { listConnections: vi.fn(), listOptimizationRequests: vi.fn(), listLlmProposals: vi.fn(), createOptimizationRequest: vi.fn(), cancelOptimizationRequest: vi.fn() },
  };
});

import { api } from "../../../api/client";
import { AskLlmButton, type SentNotice } from "./AskLlmButton";

const SQL = "SELECT count(*) FROM shop.orders WHERE status = $1";
const props = { connectionId: "c", member: "pg-1", database: "perf", source: { kind: "console" } as const, sql: SQL, params: ["disputed"], plan: null };
const LINE = "Оптимизируй запрос r3a9f1c из Glafira (или /optimize_query r3a9f1c)";
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
    ["сервер выключен", { server: "stopped" }, "diagnostics", /включите его в «Настройки» → «LLM-ассистенты \(MCP\)»/],
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
  });

  it("читает только запросы подключения, предложения не запрашивает", async () => {
    renderWithClient(<AskLlmButton {...props} />);
    await waitFor(() => expect(screen.getByRole("button", { name: LABEL })).toBeEnabled());
    expect(api.listOptimizationRequests).toHaveBeenCalledWith("c");
    expect(api.listLlmProposals).not.toHaveBeenCalled();
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
    expect(await screen.findByText(`Скопировано: «${LINE}». Вставьте строку в чат LLM-ассистента (Qwen Code, Claude Code…)`)).toBeInTheDocument();
    expect(await screen.findByText("Ждёт ответа LLM: запрос r3a9f1c")).toBeInTheDocument();
    // Пока запрос ждёт, второй не создаётся.
    expect(button).toBeDisabled();
  });

  it("буфер недоступен — уведомление просит набрать строку руками", async () => {
    vi.mocked(api.createOptimizationRequest).mockResolvedValue(optimizationRequest());
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    const button = screen.getByRole("button", { name: LABEL });
    await waitFor(() => expect(button).toBeEnabled());
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("denied"));
    await user.click(button);
    expect(await screen.findByText(`Не удалось скопировать. Напишите в чате LLM-ассистента: ${LINE}`)).toBeInTheDocument();
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
    expect(await screen.findByText(/Не хватает прав на таблицы запроса/)).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(/Не хватает прав на таблицы запроса/);
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
    const waitingText = await screen.findByText("Ждёт ответа LLM: запрос r3a9f1c");
    expect(screen.getByRole("status")).toContainElement(waitingText);
    expect(screen.getByRole("status")).not.toContainElement(screen.getByRole("button", { name: "Отменить запрос" }));
    unmount();
    setStatus(undefined, { error: { code: "INTERNAL", message: "boom", retryable: true } });
    const second = renderWithClient(<AskLlmButton {...props} />);
    expect(screen.getByRole("status")).toContainElement(await screen.findByText("Статус MCP недоступен"));
    expect(screen.getByRole("status")).not.toContainElement(screen.getByRole("button", { name: "Повторить" }));
    second.unmount();
    setStatus(mcpStatus());
    vi.mocked(api.listOptimizationRequests).mockRejectedValue({ code: "INTERNAL", message: "disk", retryable: true });
    renderWithClient(<AskLlmButton {...props} />);
    expect(screen.getByRole("status")).toContainElement(await screen.findByText("Не удалось прочитать запросы этого подключения"));
  });

  it("быстрый второй клик между созданием и перечитыванием списка не создаёт дубль", async () => {
    vi.mocked(api.createOptimizationRequest).mockResolvedValue(optimizationRequest());
    const user = userEvent.setup();
    renderWithClient(<AskLlmButton {...props} />);
    const button = screen.getByRole("button", { name: LABEL });
    await waitFor(() => expect(button).toBeEnabled());
    // Список ещё не знает о новом запросе (остаётся пустым).
    await user.click(button);
    await screen.findByText(/Скопировано/);
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
    await screen.findByText(/Скопировано/);
    rerender(<Host shown={false} />);
    expect(screen.getByText(/Скопировано/)).toBeInTheDocument();
  });

  it("область уведомлений присутствует всегда (живая область)", () => {
    renderWithClient(<AskLlmButton {...props} />);
    expect(screen.getByRole("status")).toBeInTheDocument();
  });
});
