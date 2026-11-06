import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProposalComparison } from "../../../api/types";
import { useTabMarks } from "../../../state/tabMarks";
import { useWorkspace } from "../../../state/workspace";
import { llmProposal, optimizationRequest } from "../../../test/llm";
import { planTree } from "../../../test/plans";
import { eventBus, emitEvent, resetEventBus } from "../../../test/events";
import { renderWithClient } from "../../../test/render";

vi.mock("@tauri-apps/api/event", async () => (await import("../../../test/events")).tauriEventMock());
vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return { ...orig, api: { listOptimizationRequests: vi.fn(), listLlmProposals: vi.fn(), compareLlmProposal: vi.fn(), answerLlmProposal: vi.fn() } };
});

import { api } from "../../../api/client";
import { ProposalsPanel } from "./ProposalsPanel";

const emit = (payload: unknown) => emitEvent("mcp://proposals", payload);

const ORIGINAL = "SELECT o.id, o.total\nFROM shop.orders o\nWHERE date_trunc('day', o.created_at) = $1";
const REWRITTEN = "SELECT o.id, o.total\nFROM shop.orders o\nWHERE o.created_at >= $1 AND o.created_at < $1 + interval '1 day'";

/** Шапка показанного предложения (чип статуса, кто прислал, действия). */
const cardHead = () => document.querySelector(".llm-proposal-head") as HTMLElement;

const panel = (props: Partial<Parameters<typeof ProposalsPanel>[0]> = {}) => <ProposalsPanel connectionId="c" database="perf" active tabId="explain:c" focus={null} {...props} />;

describe("ProposalsPanel", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    resetEventBus();
    useWorkspace.setState({ tabs: [], activeId: null });
    useTabMarks.setState({ marks: {} });
    vi.mocked(api.listOptimizationRequests).mockResolvedValue([
      optimizationRequest({ id: "r1", status: "answered" }),
      // Запрос консоли с id по умолчанию фикстуры `llmProposal`.
      optimizationRequest({ status: "answered" }),
      optimizationRequest({ id: "r9", source: { kind: "query", queryId: "7" }, status: "answered" }),
    ]);
  });

  it("список консоли: её запросы и предложения без запроса для её базы, новые первыми; чужие скрыты", async () => {
    vi.mocked(api.listLlmProposals).mockResolvedValue([
      llmProposal({ id: "pa", requestId: "r1", status: "accepted", createdAtMs: 30, rewrittenSql: "SELECT 1 AS accepted" }),
      llmProposal({ id: "pb", requestId: null, createdAtMs: 10, rewrittenSql: "SELECT 1 AS free" }),
      llmProposal({ id: "pc", requestId: null, database: "postgres", rewrittenSql: "SELECT 1 AS other_db" }),
      llmProposal({ id: "pd", requestId: "r9", rewrittenSql: "SELECT 1 AS from_card" }),
    ]);
    renderWithClient(panel());
    expect(await screen.findByRole("button", { name: "Предложения LLM (2)" })).toHaveAttribute("aria-expanded", "true");
    const items = within(screen.getByRole("list", { name: "Список предложений" })).getAllByRole("button");
    expect(items.map((b) => b.textContent)).toEqual([
      expect.stringMatching(/^Новое qwen-code \d{4}-\d\d-\d\d \d\d:\d\d:\d\d SELECT 1 AS free$/),
      expect.stringMatching(/^Принято qwen-code \d{4}-\d\d-\d\d \d\d:\d\d:\d\d SELECT 1 AS accepted$/),
    ]);
  });

  it("Z4/Z6: пункт списка — карточка в две строки: чип статуса (цвет и слово), клиент и время; первая строка SQL моноширинно; выбор — aria-current, не ссылка", async () => {
    vi.mocked(api.listLlmProposals).mockResolvedValue([
      llmProposal({ id: "pa", createdAtMs: 30, rewrittenSql: "\nSELECT 1 AS fresh\nFROM t" }),
      llmProposal({ id: "pb", status: "accepted", createdAtMs: 20, rewrittenSql: "SELECT 2" }),
      llmProposal({ id: "pc", status: "rejected", reason: "Нужны те же строки", createdAtMs: 10, rewrittenSql: "SELECT 3" }),
    ]);
    renderWithClient(panel());
    await screen.findByRole("button", { name: "Предложения LLM (3)" });
    const [fresh, accepted, rejected] = within(screen.getByRole("list", { name: "Список предложений" })).getAllByRole("button");
    for (const item of [fresh, accepted, rejected]) {
      expect(item).not.toHaveClass("link");
      expect(item).toHaveClass("llm-item");
    }
    expect(within(fresh).getByText("Новое")).toHaveClass("llm-status", "llm-status-new");
    expect(within(accepted).getByText("Принято")).toHaveClass("llm-status", "llm-status-accepted");
    expect(within(rejected).getByText("Отклонено")).toHaveClass("llm-status", "llm-status-rejected");
    // Причина — не в пункте списка, а отдельной строкой в карточке.
    expect(rejected).not.toHaveTextContent("Нужны те же строки");
    expect(within(fresh).getByText("qwen-code")).toBeInTheDocument();
    const sql = within(fresh).getByText("SELECT 1 AS fresh");
    expect(sql.tagName).toBe("CODE");
    expect(sql).toHaveClass("llm-item-sql");
    expect(fresh).toHaveAttribute("aria-current", "true");
    expect(accepted).toHaveAttribute("aria-current", "false");
  });

  it("Z4/Z6: шапка карточки — чип статуса и «От клиента, время»; «Принять» — основная, «Отклонить» рядом; «Сравнить планы», «Открыть в консоли», «Копировать» — в шапке, до текстов и разделов", async () => {
    vi.mocked(api.listLlmProposals).mockResolvedValue([llmProposal({ originalSql: ORIGINAL, rewrittenSql: REWRITTEN, indexes: ["CREATE INDEX ON shop.orders (created_at)"] })]);
    renderWithClient(panel());
    const accept = await screen.findByRole("button", { name: "Принять" });
    expect(accept).toHaveClass("primary");
    const head = accept.closest(".llm-proposal-head") as HTMLElement;
    expect(head).not.toBeNull();
    expect(within(head).getByText("Новое")).toHaveClass("llm-status-new");
    expect(within(head).getByText(/^От qwen-code, \d{4}-\d\d-\d\d \d\d:\d\d:\d\d$/)).toBeInTheDocument();
    expect(within(head).getByRole("button", { name: "Отклонить" })).not.toHaveClass("primary");
    const table = screen.getByRole("table", { name: "Исходный и предложенный текст" });
    const indexes = screen.getByRole("heading", { name: "Индексы" });
    for (const name of ["Сравнить планы", "Открыть в консоли", "Копировать предложенный SQL"]) {
      const button = within(head).getByRole("button", { name });
      expect(button.compareDocumentPosition(table) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
      expect(button.compareDocumentPosition(indexes) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    }
    // Разделы — заголовки, а не жирный текст среди абзацев.
    expect(screen.getAllByRole("heading", { level: 4 }).map((h) => h.textContent)).toEqual(["Обоснование", "Индексы"]);
  });

  it("Z4: причина отклонения — своей строкой под чипом, не в одной строке со статусом", async () => {
    vi.mocked(api.listLlmProposals).mockResolvedValue([llmProposal({ status: "rejected", reason: "Нужны те же строки" })]);
    renderWithClient(panel());
    const reason = await screen.findByText("Причина: Нужны те же строки");
    const head = reason.closest(".llm-proposal-head") as HTMLElement;
    expect(head).not.toBeNull();
    const chip = within(head).getByText("Отклонено");
    expect(chip).toHaveClass("llm-status-rejected");
    expect(chip).not.toContainElement(reason);
    expect(reason).toHaveClass("muted");
    expect(reason.parentElement).not.toBe(chip.parentElement);
  });

  it("Z2: у таблицы сравнения текстов — colgroup: узкие колонки номеров и колонки текста", async () => {
    vi.mocked(api.listLlmProposals).mockResolvedValue([llmProposal({ originalSql: ORIGINAL, rewrittenSql: REWRITTEN })]);
    renderWithClient(panel());
    const table = await screen.findByRole("table", { name: "Исходный и предложенный текст" });
    const cols = [...table.querySelectorAll(":scope > colgroup > col")];
    expect(cols.map((c) => c.className)).toEqual(["diff-num", "diff-text", "diff-num", "diff-text"]);
  });

  it("без предложений панель свёрнута; раскрытая — объясняет, откуда они берутся", async () => {
    vi.mocked(api.listLlmProposals).mockResolvedValue([]);
    const user = userEvent.setup();
    renderWithClient(panel());
    const toggle = await screen.findByRole("button", { name: "Предложения LLM (0)" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await user.click(toggle);
    expect(screen.getByText(/^Предложений пока нет/)).toBeInTheDocument();
  });

  it("выбранное: тексты рядом с подсветкой, обоснование и эффект как текст, индексы, клиент, пометка о строках", async () => {
    vi.mocked(api.listLlmProposals).mockResolvedValue([
      llmProposal({ originalSql: ORIGINAL, rewrittenSql: REWRITTEN, rationale: "**Range** <b>instead</b> of date_trunc", expectedEffect: "Index Scan on created_at", indexes: ["CREATE INDEX ON shop.orders (created_at)"] }),
    ]);
    renderWithClient(panel());
    const table = await screen.findByRole("table", { name: "Исходный и предложенный текст" });
    expect(within(table).getByText("WHERE date_trunc('day', o.created_at) = $1").closest("td")).toHaveClass("diff-del");
    expect(within(table).getByText("WHERE o.created_at >= $1 AND o.created_at < $1 + interval '1 day'").closest("td")).toHaveClass("diff-add");
    expect(within(table).getAllByText("FROM shop.orders o")).toHaveLength(2);
    expect(within(table).getByRole("img", { name: "Удалённая строка" })).toBeInTheDocument();
    expect(within(table).getByRole("img", { name: "Добавленная строка" })).toBeInTheDocument();
    // Разметку модели панель не применяет.
    expect(screen.getByText("**Range** <b>instead</b> of date_trunc")).toBeInTheDocument();
    expect(screen.getByText("Index Scan on created_at")).toBeInTheDocument();
    expect(screen.getByText("CREATE INDEX ON shop.orders (created_at)")).toBeInTheDocument();
    expect(screen.getByText(/^От qwen-code, /)).toBeInTheDocument();
    expect(screen.getByText("Glafira не проверяет, что переписанный запрос возвращает те же строки")).toBeInTheDocument();
  });

  it("«Сравнить планы» сразу показывает итог и перечитывает список; ошибка — баннером", async () => {
    const p = llmProposal({ id: "p1" });
    const comparison: ProposalComparison = { costBefore: 120, costAfter: 8.5, indexesUsed: [], member: "pg-1", atMs: 1_700_000_000_000, planBefore: planTree(), planAfter: planTree(), recommendationsBefore: [], recommendationsAfter: [], withoutHypopg: false };
    vi.mocked(api.listLlmProposals).mockResolvedValue([p]);
    vi.mocked(api.compareLlmProposal).mockResolvedValueOnce({ ...p, comparison });
    vi.mocked(api.compareLlmProposal).mockRejectedValueOnce({ code: "FEATURE_UNAVAILABLE", key: "errors.hypo.notInstalled", message: "hypopg is not installed", retryable: false });
    const user = userEvent.setup();
    renderWithClient(panel());
    await user.click(await screen.findByRole("button", { name: "Сравнить планы" }));
    expect(api.compareLlmProposal).toHaveBeenCalledWith("p1");
    expect(await screen.findByRole("region", { name: "Сравнение планов" })).toHaveTextContent("Стоимость: была 120,00, стала 8,50");
    await waitFor(() => expect(api.listLlmProposals).toHaveBeenCalledTimes(2));
    await user.click(screen.getByRole("button", { name: "Сравнить планы" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
  });

  it("«Принять» и «Отклонить» с причиной уходят в answer_llm_proposal; у отвеченного кнопок нет", async () => {
    const p1 = llmProposal({ id: "p1", createdAtMs: 20 });
    const p2 = llmProposal({ id: "p2", createdAtMs: 10, rewrittenSql: "SELECT 2" });
    vi.mocked(api.listLlmProposals).mockResolvedValue([p1, p2]);
    vi.mocked(api.answerLlmProposal).mockImplementation(async (id, status, reason) => ({ ...(id === "p1" ? p1 : p2), status, reason }));
    const user = userEvent.setup();
    renderWithClient(panel());
    const accept = await screen.findByRole("button", { name: "Принять" });
    vi.mocked(api.listLlmProposals).mockResolvedValue([p2, { ...p1, status: "accepted" }]);
    await user.click(accept);
    expect(api.answerLlmProposal).toHaveBeenCalledWith("p1", "accepted", null);
    // Выбор остаётся на отвеченном предложении, хотя список пересортирован.
    await waitFor(() => expect(screen.queryByRole("button", { name: "Принять" })).toBeNull());
    expect(within(cardHead()).getByText("Принято")).toHaveClass("llm-status-accepted");
    await user.click(screen.getByRole("button", { name: /SELECT 2$/ }));
    await user.click(screen.getByRole("button", { name: "Отклонить" }));
    const reason = screen.getByRole("textbox", { name: "Причина (её увидит модель)" });
    expect(reason).toHaveAttribute("maxlength", "500");
    await user.type(reason, "Нужны те же строки");
    vi.mocked(api.listLlmProposals).mockResolvedValue([{ ...p2, status: "rejected", reason: "Нужны те же строки" }, { ...p1, status: "accepted" }]);
    await user.click(screen.getByRole("button", { name: "Отклонить предложение" }));
    expect(api.answerLlmProposal).toHaveBeenLastCalledWith("p2", "rejected", "Нужны те же строки");
    expect(await screen.findByText("Причина: Нужны те же строки")).toBeInTheDocument();
    expect(within(cardHead()).getByText("Отклонено")).toHaveClass("llm-status-rejected");
  });

  it("«Открыть в консоли» подставляет предложенный SQL с базой и узлом предложения; «Копировать» — его же", async () => {
    vi.mocked(api.listLlmProposals).mockResolvedValue([llmProposal({ rewrittenSql: REWRITTEN, member: "pg-2" })]);
    const user = userEvent.setup();
    renderWithClient(panel());
    await user.click(await screen.findByRole("button", { name: "Открыть в консоли" }));
    expect(useWorkspace.getState().tabs.find((t) => t.id === "explain:c")).toMatchObject({ prefill: { sql: REWRITTEN, database: "perf", member: "pg-2" } });
    await user.click(screen.getByRole("button", { name: "Копировать предложенный SQL" }));
    expect(await navigator.clipboard.readText()).toBe(REWRITTEN);
  });

  it("значок карточки: предложение запроса карточки показывается и выбирается", async () => {
    vi.mocked(api.listLlmProposals).mockResolvedValue([
      llmProposal({ id: "p1", rewrittenSql: "SELECT 1 AS console_one" }),
      llmProposal({ id: "p9", requestId: "r9", rewrittenSql: "SELECT 9 AS from_card", createdAtMs: 1 }),
    ]);
    renderWithClient(panel({ focus: { id: "p9", nonce: 1 } }));
    const table = await screen.findByRole("table", { name: "Исходный и предложенный текст" });
    expect(within(table).getByText("SELECT 9 AS from_card")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /SELECT 9 AS from_card$/ })).toHaveAttribute("aria-current", "true");
  });

  it("точка у вкладки: предложение пришло, пока консоль в фоне; показ и размонтирование снимают её", async () => {
    vi.mocked(api.listLlmProposals).mockResolvedValue([llmProposal({ id: "p1" })]);
    const { rerender, unmount } = renderWithClient(panel());
    await screen.findByRole("button", { name: "Предложения LLM (1)" });
    rerender(panel({ active: false }));
    expect(useTabMarks.getState().marks["explain:c"]).toBeUndefined();
    await waitFor(() => expect(eventBus.handlers.length).toBeGreaterThan(0));
    vi.mocked(api.listLlmProposals).mockResolvedValue([llmProposal({ id: "p2", createdAtMs: 1_800_000_000_000 }), llmProposal({ id: "p1" })]);
    emit({ kind: "proposal", connectionId: "c", proposalId: "p2" });
    await waitFor(() => expect(useTabMarks.getState().marks["explain:c"]).toBe(true));
    rerender(panel({ active: true }));
    await waitFor(() => expect(useTabMarks.getState().marks["explain:c"]).toBeUndefined());
    rerender(panel({ active: false }));
    unmount();
    expect(useTabMarks.getState().marks).toEqual({});
  });

  it("доступность: переключатель связан с областью; причина получает фокус, отмена возвращает его; после ответа фокус не уходит на body; живая область сравнения есть всегда", async () => {
    const p = llmProposal({ id: "p1", indexes: ["CREATE INDEX ON t (a)", "CREATE INDEX ON t (a)"] });
    vi.mocked(api.listLlmProposals).mockResolvedValue([p]);
    vi.mocked(api.answerLlmProposal).mockImplementation(async (_id, status, reason) => ({ ...p, status, reason }));
    const warn = vi.spyOn(console, "error").mockImplementation(() => {});
    const user = userEvent.setup();
    const { container } = renderWithClient(panel());
    const toggle = await screen.findByRole("button", { name: "Предложения LLM (1)" });
    expect(container.querySelector(`[id="${toggle.getAttribute("aria-controls")}"]`)).not.toBeNull();
    expect(container.querySelector('[aria-live="polite"]')).not.toBeNull();
    // Повтор индекса в ответе модели не даёт одинаковых ключей.
    expect(screen.getAllByText("CREATE INDEX ON t (a)")).toHaveLength(2);
    expect(warn).not.toHaveBeenCalled();
    const reject = screen.getByRole("button", { name: "Отклонить" });
    expect(reject).toHaveAttribute("aria-expanded", "false");
    await user.click(reject);
    expect(reject).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("textbox", { name: "Причина (её увидит модель)" })).toHaveFocus();
    await user.click(screen.getByRole("button", { name: "Отмена" }));
    expect(reject).toHaveFocus();
    vi.mocked(api.listLlmProposals).mockResolvedValue([{ ...p, status: "accepted" }]);
    await user.click(screen.getByRole("button", { name: "Принять" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Принять" })).toBeNull());
    expect(document.body).not.toHaveFocus();
    // Фокус — на шапке карточки: читается новый статус и кто прислал.
    expect(document.activeElement).toHaveClass("llm-proposal-meta");
    expect(document.activeElement).toHaveTextContent(/^Принято\s*От qwen-code/);
    warn.mockRestore();
  });

  it("#3: фокус после ответа приходит на шапку, где статус уже новый, — не дожидаясь перечитывания списка", async () => {
    const p = llmProposal({ id: "p1" });
    vi.mocked(api.listLlmProposals).mockResolvedValue([p]);
    vi.mocked(api.answerLlmProposal).mockImplementation(async (_id, status, reason) => ({ ...p, status, reason }));
    const user = userEvent.setup();
    renderWithClient(panel());
    const accept = await screen.findByRole("button", { name: "Принять" });
    // Что видит скринридер в момент переноса фокуса: текст элемента, получившего фокус.
    const atFocus: string[] = [];
    const record = (e: FocusEvent) => atFocus.push((e.target as HTMLElement).textContent ?? "");
    document.addEventListener("focusin", record);
    // Перечитывание списка после ответа висит: шапка не может взять новый статус из списка.
    let reread!: (v: unknown) => void;
    vi.mocked(api.listLlmProposals).mockReturnValueOnce(new Promise((r) => (reread = r)) as never);
    await user.click(accept);
    await waitFor(() => expect(document.activeElement).toHaveClass("llm-proposal-meta"));
    expect(atFocus[atFocus.length - 1]).toMatch(/^Принято\s*От qwen-code/);
    expect(screen.queryByRole("button", { name: "Принять" })).toBeNull();
    expect(screen.getByRole("button", { name: "Отклонить" })).toBeInTheDocument();
    // Список ещё старый: пункт показывает прежний статус, пока перечитывание не пришло.
    const item = within(screen.getByRole("list", { name: "Список предложений" })).getByRole("button");
    expect(item).toHaveTextContent(/^Новое/);
    reread([{ ...p, status: "accepted" }]);
    await waitFor(() => expect(item).toHaveTextContent(/^Принято/));
    expect(within(cardHead()).getByText("Принято")).toHaveClass("llm-status-accepted");

    // Отклонение с причиной: в момент фокуса — «Отклонено» и причина, хотя список ещё «Принято».
    await user.click(screen.getByRole("button", { name: "Отклонить" }));
    await user.type(screen.getByRole("textbox", { name: "Причина (её увидит модель)" }), "Нужны те же строки");
    vi.mocked(api.listLlmProposals).mockReturnValueOnce(new Promise((r) => (reread = r)) as never);
    atFocus.length = 0;
    await user.click(screen.getByRole("button", { name: "Отклонить предложение" }));
    await waitFor(() => expect(document.activeElement).toHaveClass("llm-proposal-meta"));
    expect(atFocus[atFocus.length - 1]).toMatch(/^Отклонено\s*От qwen-code/);
    expect(within(cardHead()).getByText("Причина: Нужны те же строки")).toBeInTheDocument();
    expect(item).toHaveTextContent(/^Принято/);
    reread([{ ...p, status: "rejected", reason: "Нужны те же строки" }]);
    await waitFor(() => expect(item).toHaveTextContent(/^Отклонено/));
    expect(within(cardHead()).getByText("Отклонено")).toHaveClass("llm-status-rejected");
    expect(screen.getByRole("button", { name: "Принять" })).toBeInTheDocument();
    document.removeEventListener("focusin", record);
  });

  it("I1: «Сравнить планы» при открытой причине отклонения не закрывает форму, не стирает набранное и не роняет фокус", async () => {
    const p = llmProposal({ id: "p1" });
    const comparison: ProposalComparison = { costBefore: 120, costAfter: 8.5, indexesUsed: [], member: "pg-1", atMs: 1_700_000_000_000, planBefore: planTree(), planAfter: planTree(), recommendationsBefore: [], recommendationsAfter: [], withoutHypopg: false };
    vi.mocked(api.listLlmProposals).mockResolvedValue([p]);
    vi.mocked(api.compareLlmProposal).mockResolvedValue({ ...p, comparison });
    const user = userEvent.setup();
    renderWithClient(panel());
    await user.click(await screen.findByRole("button", { name: "Отклонить" }));
    const reason = screen.getByRole("textbox", { name: "Причина (её увидит модель)" });
    await user.type(reason, "Нужны те же строки");
    await user.click(screen.getByRole("button", { name: "Сравнить планы" }));
    expect(await screen.findByRole("region", { name: "Сравнение планов" })).toBeInTheDocument();
    const still = screen.getByRole("textbox", { name: "Причина (её увидит модель)" });
    expect(still).toBe(reason);
    expect(still).toHaveValue("Нужны те же строки");
    // Фокус остаётся на нажатой кнопке, а не падает на <body>.
    expect(screen.getByRole("button", { name: "Сравнить планы" })).toHaveFocus();
  });

  it("M1: консоль, смонтированная в фоне, не ставит точку на уже существовавшие предложения; пришедшее позже — ставит", async () => {
    vi.mocked(api.listLlmProposals).mockResolvedValue([llmProposal({ id: "p1" })]);
    renderWithClient(panel({ active: false }));
    await screen.findByRole("button", { name: "Предложения LLM (1)" });
    expect(useTabMarks.getState().marks["explain:c"]).toBeUndefined();
    await waitFor(() => expect(eventBus.handlers.length).toBeGreaterThan(0));
    vi.mocked(api.listLlmProposals).mockResolvedValue([llmProposal({ id: "p2", createdAtMs: 1_800_000_000_000 }), llmProposal({ id: "p1" })]);
    emit({ kind: "proposal", connectionId: "c", proposalId: "p2" });
    await waitFor(() => expect(useTabMarks.getState().marks["explain:c"]).toBe(true));
  });

  it("M3: у отвеченного — противоположное действие; отклонение принятого спрашивает причину, принятие отклонённого уходит сразу", async () => {
    const p = llmProposal({ id: "p1", status: "accepted" });
    vi.mocked(api.listLlmProposals).mockResolvedValue([p]);
    vi.mocked(api.answerLlmProposal).mockImplementation(async (_id, status, reason) => ({ ...p, status, reason }));
    const user = userEvent.setup();
    renderWithClient(panel());
    expect(await screen.findByRole("button", { name: "Отклонить" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Принять" })).toBeNull();
    vi.mocked(api.listLlmProposals).mockResolvedValue([{ ...p, status: "rejected", reason: "Передумал" }]);
    await user.click(screen.getByRole("button", { name: "Отклонить" }));
    await user.type(screen.getByRole("textbox", { name: "Причина (её увидит модель)" }), "Передумал");
    await user.click(screen.getByRole("button", { name: "Отклонить предложение" }));
    expect(api.answerLlmProposal).toHaveBeenLastCalledWith("p1", "rejected", "Передумал");
    expect(await screen.findByRole("button", { name: "Принять" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Отклонить" })).toBeNull();
    vi.mocked(api.listLlmProposals).mockResolvedValue([{ ...p, status: "accepted" }]);
    await user.click(screen.getByRole("button", { name: "Принять" }));
    expect(api.answerLlmProposal).toHaveBeenLastCalledWith("p1", "accepted", null);
    expect(await screen.findByRole("button", { name: "Отклонить" })).toBeInTheDocument();
  });

  it("M4/M2: пока идёт сравнение, в живой области — «Сравнение…»; заметка hypopg — внутри единственной живой области, без своей role=status", async () => {
    const p = llmProposal({ id: "p1" });
    const comparison: ProposalComparison = { costBefore: 1, costAfter: 1, indexesUsed: [], member: "pg-1", atMs: 1_700_000_000_000, planBefore: planTree(), planAfter: planTree(), recommendationsBefore: [], recommendationsAfter: [], withoutHypopg: true };
    vi.mocked(api.listLlmProposals).mockResolvedValue([p]);
    let done!: (v: unknown) => void;
    vi.mocked(api.compareLlmProposal).mockReturnValue(new Promise((r) => (done = r)) as never);
    const user = userEvent.setup();
    const { container } = renderWithClient(panel());
    await user.click(await screen.findByRole("button", { name: "Сравнить планы" }));
    const live = container.querySelector('[aria-live="polite"]')!;
    expect(live).toHaveTextContent("Сравнение планов…");
    done({ ...p, comparison });
    await waitFor(() => expect(live).not.toHaveTextContent("Сравнение планов…"));
    expect(live).toHaveTextContent("В этой базе нет hypopg");
    expect(container.querySelectorAll("[aria-live]").length).toBe(1);
    expect(live.querySelector('[role="status"]')).toBeNull();
  });
});
