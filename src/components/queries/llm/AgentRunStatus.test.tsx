import { act, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentRun, RunStatus } from "../../../api/types";
import { useShownProposals } from "../../../state/shownProposals";
import { agentRun, optimizationRequest } from "../../../test/llm";
import { renderWithClient } from "../../../test/render";

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return { ...orig, api: { readAgentLog: vi.fn(), revealAgentLog: vi.fn() } };
});

import { api } from "../../../api/client";
import { AgentRunStatus, type AgentRunStatusProps } from "./AgentRunStatus";

const LINE = "Оптимизируй запрос r3a9f1c из Glafira";
const NOW = 1_700_000_100_000;
const onRetry = vi.fn();
const onCancel = vi.fn();

function view(patch: Partial<AgentRunStatusProps> = {}) {
  return renderWithClient(
    <AgentRunStatus request={optimizationRequest()} run={agentRun()} ahead={0} chatLine={LINE} busy={false} onRetry={onRetry} onCancel={onCancel} {...patch} />,
  );
}

const group = () => screen.getByRole("group", { name: "Фоновый ассистент: запрос r3a9f1c" });
/** Подписи действий строки статуса по порядку. */
const buttons = () =>
  within(group())
    .queryAllByRole("button")
    .map((b) => b.textContent);

describe("AgentRunStatus", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useShownProposals.setState({ shown: {} });
  });
  afterEach(() => vi.useRealTimers());

  it("в очереди: сколько запусков перед ним; «Скопировать строку для чата» и «Отменить запрос»", () => {
    view({ ahead: 2 });
    expect(screen.getByRole("status")).toHaveTextContent(/^В очереди фонового ассистента, перед ним: 2$/);
    expect(buttons()).toEqual(["Скопировать строку для чата", "Отменить запрос"]);
  });

  it("следующий в очереди — без числа", () => {
    view({ ahead: 0 });
    expect(screen.getByText("В очереди фонового ассистента: следующий")).toBeInTheDocument();
  });

  it("запускается: предупреждение Ollama — отдельной строкой внутри живой области", () => {
    view({ run: agentRun({ status: "starting", startedAtMs: Date.now(), warnings: ["evict qwen3.8:27b qwen3.6-64k:35b-a3b"] }) });
    const status = screen.getByRole("status");
    const phase = within(status).getByText("Ассистент запускается…");
    const warning = within(status).getByText("Ollama может выгрузить qwen3.8:27b, чтобы загрузить qwen3.6-64k:35b-a3b");
    expect(phase.tagName).toBe("P");
    expect(warning.tagName).toBe("P");
    expect(phase).not.toBe(warning);
  });

  it("работает: время, вызовы и последний инструмент — своей строкой вне живой области; время идёт", () => {
    vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] });
    vi.setSystemTime(NOW);
    view({ run: agentRun({ status: "running", startedAtMs: NOW - 83_000, calls: 4, lastTool: "hypothetical_index" }) });
    expect(screen.getByRole("status")).toHaveTextContent(/^Ассистент работает$/);
    const progress = screen.getByText("1:23 · вызовов: 4 · hypothetical_index");
    expect(screen.getByRole("status")).not.toContainElement(progress);
    act(() => vi.advanceTimersByTime(1_000));
    expect(screen.getByText("1:24 · вызовов: 4 · hypothetical_index")).toBeInTheDocument();
    expect(buttons()).toEqual(["Скопировать строку для чата", "Отменить запрос"]);
  });

  it("работает, вызовов ещё не было — без инструмента", () => {
    vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] });
    vi.setSystemTime(NOW);
    view({ run: agentRun({ status: "running", startedAtMs: NOW - 5_000 }) });
    expect(screen.getByText("0:05 · вызовов: 0")).toBeInTheDocument();
  });

  it("закончил без предложений: «Ответ ассистента» раскрывает итог без разметки; «Повторить», копирование, отмена", async () => {
    const user = userEvent.setup();
    view({ run: agentRun({ status: "done", startedAtMs: NOW - 60_000, endedAtMs: NOW, summary: "**Индекс уже есть**: план не улучшить" }) });
    expect(screen.getByRole("status")).toHaveTextContent(/^Ассистент закончил без предложений$/);
    expect(buttons()).toEqual(["Ответ ассистента", "Повторить", "Скопировать строку для чата", "Отменить запрос"]);
    const toggle = screen.getByRole("button", { name: "Ответ ассистента" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText(/Индекс уже есть/)).toBeNull();
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    // Разметка модели не показывается сырыми символами (смоук P4c, R16).
    expect(screen.getByText("Индекс уже есть: план не улучшить")).toBeInTheDocument();
    expect(screen.queryByText(/\*\*/)).toBeNull();
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: "Скопировать строку для чата" }));
    expect(await navigator.clipboard.readText()).toBe(LINE);
    expect(screen.getByRole("status")).toHaveTextContent(`Скопировано: «${LINE}»`);
    await user.click(screen.getByRole("button", { name: "Отменить запрос" }));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("длинный «Ответ ассистента» свёрнут до пяти строк: «Показать всё» внутри строки статуса", async () => {
    const user = userEvent.setup();
    const summary = ["SUMMARY 1", "2", "3", "4", "5", "Дальше: сравните планы"].join("\n");
    view({ run: agentRun({ status: "done", startedAtMs: NOW - 60_000, endedAtMs: NOW, summary }) });
    await user.click(screen.getByRole("button", { name: "Ответ ассистента" }));
    expect(screen.queryByText(/Дальше/)).toBeNull();
    const all = within(group()).getByRole("button", { name: "Показать всё" });
    await user.click(all);
    expect(screen.getByText(/Дальше: сравните планы/)).toBeInTheDocument();
    expect(all).toHaveTextContent("Свернуть");
  });

  it("буфер недоступен — строку предлагают набрать руками", async () => {
    const user = userEvent.setup();
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("denied"));
    view();
    await user.click(screen.getByRole("button", { name: "Скопировать строку для чата" }));
    expect(await within(screen.getByRole("status")).findByText(`Скопировать строку не удалось, наберите её в чате: «${LINE}»`)).toBeInTheDocument();
  });

  it.each<[string, Partial<AgentRun>, string]>([
    ["срок", { reason: "timeout", startedAtMs: NOW - 15 * 60_000 - 4_000, endedAtMs: NOW }, "превышен срок 15 мин"],
    ["программа не найдена", { reason: "cliNotFound" }, "программа не найдена: /opt/homebrew/bin/qwen"],
    ["код выхода", { reason: "exit", exitCode: 3 }, "код выхода 3"],
    ["не подключился", { reason: "notConnected" }, "агент не подключился к Glafira"],
    ["предел", { reason: "limit" }, "достигнут предел вызовов или расходов"],
    ["лишние инструменты", { reason: "unsafeTools", warnings: ["unsafe run_shell_command"] }, "агенту доступны не только инструменты Glafira: run_shell_command; запуск остановлен до первого хода"],
    ["нет init", { reason: "unsafeTools", warnings: ["unsafe no-init"] }, "агент не сообщил список своих инструментов; запуск остановлен"],
  ])("ошибка (%s): причина текстом, «Показать журнал», «Повторить», «Закрыть запрос»", (_name, patch, reason) => {
    // Запуск начался (`starting` ставит startedAtMs) — журнал есть.
    view({ run: agentRun({ status: "failed", startedAtMs: NOW - 10_000, endedAtMs: NOW, ...patch }) });
    expect(screen.getByRole("status")).toHaveTextContent(new RegExp(`^Ошибка фонового ассистента: ${reason.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`));
    expect(buttons()).toEqual(["Показать журнал", "Повторить", "Скопировать строку для чата", "Закрыть запрос"]);
  });

  it.each<[string, Partial<AgentRun>]>([
    ["политика", { reason: "policy" }],
    ["MCP-сервер выключили", { reason: "mcpOff" }],
    ["не настроен", { reason: "notConfigured" }],
  ])("ошибка до старта процесса (%s): журнала нет — без «Показать журнал»", (_name, patch) => {
    view({ run: agentRun({ status: "failed", startedAtMs: null, endedAtMs: NOW, ...patch }) });
    expect(buttons()).toEqual(["Повторить", "Скопировать строку для чата", "Закрыть запрос"]);
  });

  it("«Показать журнал» открывает журнал запуска, «Закрыть» закрывает", async () => {
    vi.mocked(api.readAgentLog).mockResolvedValue("stderr: model not found");
    const user = userEvent.setup();
    view({ run: agentRun({ status: "failed", reason: "exit", exitCode: 1, startedAtMs: NOW - 10_000, endedAtMs: NOW }) });
    await user.click(screen.getByRole("button", { name: "Показать журнал" }));
    expect(await screen.findByRole("dialog", { name: "Журнал запуска a1b2c3d" })).toBeInTheDocument();
    expect(await screen.findByText("stderr: model not found")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Закрыть" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("button", { name: "Показать журнал" })).toHaveFocus();
  });

  it.each<[RunStatus, string]>([
    ["interrupted", "Запуск прерван: Glafira закрылась"],
    ["cancelled", "Запуск остановлен"],
  ])("%s: «Повторить», копирование, «Закрыть запрос»", (status, text) => {
    view({ run: agentRun({ status, endedAtMs: NOW }) });
    expect(screen.getByRole("status")).toHaveTextContent(text);
    expect(buttons()).toEqual(["Повторить", "Скопировать строку для чата", "Закрыть запрос"]);
  });

  it.each<[string, Partial<AgentRun> | null, string]>([
    ["ошибка", { status: "failed", reason: "timeout", startedAtMs: NOW - 120_000, endedAtMs: NOW }, "Закрыть запрос"],
    ["прерван", { status: "interrupted", endedAtMs: NOW }, "Закрыть запрос"],
    ["остановлен", { status: "cancelled", endedAtMs: NOW }, "Закрыть запрос"],
    ["ждёт без запуска", null, "Отменить запрос"],
    ["в очереди", { status: "queued" }, "Отменить запрос"],
    ["запускается", { status: "starting", startedAtMs: NOW }, "Отменить запрос"],
    ["работает", { status: "running", startedAtMs: NOW }, "Отменить запрос"],
    ["без предложений", { status: "done", startedAtMs: NOW - 1_000, endedAtMs: NOW }, "Отменить запрос"],
  ])("R15 (%s): подпись ссылки закрытия ждущего запроса по состоянию запуска; действие то же", async (_name, patch, label) => {
    const user = userEvent.setup();
    view({ run: patch === null ? null : agentRun(patch) });
    const other = label === "Закрыть запрос" ? "Отменить запрос" : "Закрыть запрос";
    expect(within(group()).queryByRole("button", { name: other })).toBeNull();
    await user.click(within(group()).getByRole("button", { name: label }));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("ждущий запрос без запуска (или запуск пропущен): ожидание и «Запустить фонового ассистента»", async () => {
    const user = userEvent.setup();
    const { unmount } = view({ run: null });
    expect(screen.getByRole("status")).toHaveTextContent("Ждёт ответа LLM: запрос r3a9f1c");
    expect(buttons()).toEqual(["Запустить фонового ассистента", "Скопировать строку для чата", "Отменить запрос"]);
    await user.click(screen.getByRole("button", { name: "Запустить фонового ассистента" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
    unmount();
    view({ run: agentRun({ status: "skipped", endedAtMs: NOW }) });
    expect(screen.getByRole("status")).toHaveTextContent(/^Ждёт ответа LLM: запрос r3a9f1c$/);
    expect(buttons()).toEqual(["Запустить фонового ассистента", "Скопировать строку для чата", "Отменить запрос"]);
  });

  it("пока действие выполняется, «Повторить» и «Отменить запрос» неактивны, копирование — нет", () => {
    view({ run: agentRun({ status: "failed", reason: "exit", exitCode: 1 }), busy: true });
    expect(screen.getByRole("button", { name: "Повторить" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Закрыть запрос" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Скопировать строку для чата" })).toBeEnabled();
  });

  it("после «Повторить» кнопка исчезает, а фокус остаётся в строке статуса (не уходит на body)", async () => {
    const user = userEvent.setup();
    function Host() {
      const [run, setRun] = useState<AgentRun>(agentRun({ status: "failed", reason: "notConnected", endedAtMs: NOW }));
      return (
        <AgentRunStatus request={optimizationRequest()} run={run} ahead={0} chatLine={LINE} busy={false} onRetry={() => setRun(agentRun({ id: "a2b3c4d" }))} onCancel={onCancel} />
      );
    }
    renderWithClient(<Host />);
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(await screen.findByText("В очереди фонового ассистента: следующий")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Повторить" })).toBeNull();
    expect(group()).toHaveFocus();
  });

  it("ассистент прислал предложения: строка видна, пока панель их не показала; у отвеченного запроса действий нет", () => {
    view({ request: optimizationRequest({ status: "answered", proposalIds: ["p1", "p2"] }), run: agentRun({ status: "done", proposalIds: ["p1", "p2"], endedAtMs: NOW }) });
    expect(screen.getByRole("status")).toHaveTextContent(/^Ассистент прислал 2 предложения$/);
    expect(buttons()).toEqual([]);
    act(() => useShownProposals.getState().markShown(["p1", "p2"]));
    expect(screen.queryByRole("group")).toBeNull();
  });

  it("у отвеченного запроса без предложений этого запуска (ответил чат) строки нет", () => {
    view({ request: optimizationRequest({ status: "answered", proposalIds: ["p9"] }), run: agentRun({ status: "skipped", endedAtMs: NOW }) });
    expect(screen.queryByRole("group")).toBeNull();
  });

  it("предложение пришло, а ассистент ещё работает: обе строки, каждая своей строкой", () => {
    view({
      request: optimizationRequest({ status: "answered", proposalIds: ["p1"] }),
      run: agentRun({ status: "running", startedAtMs: Date.now(), calls: 6, lastTool: "propose_query_rewrite", proposalIds: ["p1"] }),
    });
    const status = screen.getByRole("status");
    expect(within(status).getByText("Ассистент работает")).toBeInTheDocument();
    expect(within(status).getByText("Ассистент прислал 1 предложение")).toBeInTheDocument();
    expect(buttons()).toEqual([]);
  });
});
