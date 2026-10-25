import { act, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { diagReport, sessionRow } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";

// В jsdom у элементов нет размеров, и настоящий виртуализатор ничего не показывает: окно задаётся тестом.
const win = vi.hoisted(() => ({ from: 0, size: 10 }));
vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: (o: { count: number }) => ({
    getVirtualItems: () => Array.from({ length: Math.max(0, Math.min(win.size, o.count - win.from)) }, (_, i) => ({ index: win.from + i, start: (win.from + i) * 26 })),
    getTotalSize: () => o.count * 26,
    measureElement: () => {},
  }),
}));

import type { SessionRow, SessionsData } from "../../api/types";
import { SessionsView } from "./SessionsView";

describe("SessionsView", () => {
  afterEach(() => {
    win.from = 0;
    // В jsdom scrollIntoView нет: тесты подставляют заглушку и убирают её.
    delete (Element.prototype as { scrollIntoView?: unknown }).scrollIntoView;
  });

  const rows = [
    sessionRow({ pid: 1, state: "active", application: "api", queryAgeS: 5, query: "select 1" }),
    sessionRow({ pid: 2, state: "idle in transaction", application: "api", stateAgeS: 900, locksHeld: 3, xminAge: 10, query: "update t" }),
    sessionRow({ pid: 3, state: "idle", application: "worker" }),
    sessionRow({ pid: 4, state: "active", application: "glafira-pg", glafira: true }),
    sessionRow({ pid: 5, state: "active", waitEventType: "Lock", waitEvent: "transactionid", blockedBy: [2], application: "api" }),
  ];
  const settings = { idleInTransactionSessionTimeoutMs: 0, trackActivityQuerySize: 4096 };
  const report = (r: SessionRow[] = rows, s: SessionsData["settings"] = settings) => diagReport({ kind: "sessions", rows: r, settings: s });

  /** Строка списка по pid: первая ячейка строки — pid (имена строк содержат и длительности, поиск по имени неоднозначен). */
  const rowOf = (pid: number) => screen.getAllByRole("row").find((r) => within(r).queryAllByRole("cell")[0]?.textContent === String(pid))!;
  const pidsShown = () => screen.getAllByRole("row").slice(1).map((r) => within(r).queryAllByRole("cell")[0]?.textContent);

  it("пресеты, скрытие сессий Glafira и группировка по приложению", async () => {
    const user = userEvent.setup();
    renderWithClient(<SessionsView report={report()} preset="all" />);
    // Сессия Glafira скрыта по умолчанию: строк данных — четыре.
    expect(screen.getAllByRole("row").length - 1).toBe(4);
    await user.click(screen.getByRole("tab", { name: "Ждущие" }));
    expect(rowOf(5)).toHaveTextContent("Lock:transactionid");
    expect(rowOf(5)).toHaveTextContent("2");
    await user.selectOptions(screen.getByRole("combobox", { name: "Группировать" }), "application");
    const api = screen.getByRole("row", { name: /^api/ });
    expect(api).toHaveTextContent("3");
  });

  it("пресет idle in transaction: сортировка по простою, колонки блокировок и xmin, настройка таймаута", async () => {
    renderWithClient(<SessionsView report={report()} preset="idleTx" />);
    expect(screen.getByRole("tab", { name: "Idle in transaction", selected: true })).toBeInTheDocument();
    expect(rowOf(2)).toHaveTextContent("15 мин");
    expect(screen.getByRole("columnheader", { name: "Держит блокировок" })).toBeInTheDocument();
    // Колонки, которые в этом наборе одинаковы у всех строк, не показываются: таблица помещается в окно.
    for (const name of ["Состояние", "Ожидание", "Запрос", "Ждёт"]) expect(screen.queryByRole("columnheader", { name })).toBeNull();
    expect(screen.getByText(/idle_in_transaction_session_timeout = 0/)).toBeInTheDocument();
  });

  it("строка открывает панель с полным текстом запроса и копированием", async () => {
    const user = userEvent.setup();
    renderWithClient(<SessionsView report={report(rows, { idleInTransactionSessionTimeoutMs: null, trackActivityQuerySize: null })} preset="all" />);
    await user.click(rowOf(2));
    expect(screen.getByRole("region", { name: "Сессия 2" })).toHaveTextContent("update t");
    expect(screen.getByRole("button", { name: "Копировать запрос" })).toBeInTheDocument();
  });

  it("пресеты отбирают сессии; «Активные» — по длительности запроса, «Idle in transaction» — по простою", async () => {
    const user = userEvent.setup();
    const many = [
      sessionRow({ pid: 10, state: "active", queryAgeS: 5 }),
      sessionRow({ pid: 11, state: "active", queryAgeS: 50 }),
      sessionRow({ pid: 12, state: "idle in transaction", stateAgeS: 10 }),
      sessionRow({ pid: 13, state: "idle in transaction (aborted)", stateAgeS: 300 }),
      sessionRow({ pid: 14, state: "idle" }),
    ];
    renderWithClient(<SessionsView report={report(many)} preset="all" />);
    expect(pidsShown()).toEqual(["10", "11", "12", "13", "14"]);
    await user.click(screen.getByRole("tab", { name: "Активные" }));
    expect(pidsShown()).toEqual(["11", "10"]);
    await user.click(screen.getByRole("tab", { name: "Idle in transaction" }));
    expect(pidsShown()).toEqual(["13", "12"]);
    expect(screen.getByRole("tab", { name: "Idle in transaction", selected: true })).toBeInTheDocument();
  });

  it("флажок возвращает сессии Glafira; фильтр — подстрока без учёта регистра по pid, роли, приложению, клиенту, БД и запросу", async () => {
    const user = userEvent.setup();
    const many = [
      sessionRow({ pid: 100, user: "alice", application: "api", database: "orders", clientAddr: "10.1.1.1", query: "SELECT * FROM Invoices" }),
      sessionRow({ pid: 200, user: "bob", application: "etl", database: "dwh", clientAddr: "10.2.2.2", query: "vacuum" }),
      sessionRow({ pid: 300, application: "glafira-pg", glafira: true }),
    ];
    renderWithClient(<SessionsView report={report(many)} preset="all" />);
    expect(pidsShown()).toEqual(["100", "200"]);
    await user.click(screen.getByRole("checkbox", { name: "Скрыть сессии Glafira" }));
    expect(pidsShown()).toEqual(["100", "200", "300"]);
    await user.click(screen.getByRole("checkbox", { name: "Скрыть сессии Glafira" }));
    const filter = screen.getByRole("textbox", { name: "Фильтр" });
    for (const [text, pid] of [["invoices", "100"], ["BOB", "200"], ["etl", "200"], ["dwh", "200"], ["10.1.1", "100"], ["alice", "100"], ["200", "200"]]) {
      await user.clear(filter);
      await user.type(filter, text);
      expect(pidsShown(), text).toEqual([pid]);
    }
    await user.clear(filter);
    await user.type(filter, "нет такого");
    expect(pidsShown()).toEqual([]);
    expect(screen.getByText("Подходящих сессий нет")).toBeInTheDocument();
    await user.clear(filter);
    expect(pidsShown()).toEqual(["100", "200"]);
  });

  it("строки поиска строятся только при непустом фильтре и не пересчитываются при наборе текста", async () => {
    const user = userEvent.setup();
    // Строка 250 вне окна виртуализатора (первые 10): поле `user` читает только построитель строки поиска.
    let reads = 0;
    const many = Array.from({ length: 300 }, (_, i) => sessionRow({ pid: 1000 + i, query: `select ${i}` }));
    Object.defineProperty(many[250], "user", {
      get() {
        reads++;
        return "u";
      },
      enumerable: true,
    });
    renderWithClient(<SessionsView report={report(many)} preset="all" />);
    expect(reads).toBe(0);
    const filter = screen.getByRole("textbox", { name: "Фильтр" });
    await user.type(filter, "z");
    const afterFirst = reads;
    expect(afterFirst).toBeGreaterThan(0);
    await user.type(filter, "zz");
    expect(reads).toBe(afterFirst);
    await user.clear(filter);
    expect(pidsShown()).toHaveLength(10);
  });

  it("группировка: всего и по состояниям, группа без значения; пресет на группы не влияет", async () => {
    const user = userEvent.setup();
    const many = [
      sessionRow({ pid: 1, state: "active", user: "app" }),
      sessionRow({ pid: 2, state: "idle", user: "app" }),
      sessionRow({ pid: 3, state: "idle in transaction", user: "app" }),
      sessionRow({ pid: 4, state: "idle in transaction (aborted)", user: "app" }),
      sessionRow({ pid: 5, state: null, user: "app" }),
      sessionRow({ pid: 6, state: "active", user: null }),
    ];
    renderWithClient(<SessionsView report={report(many)} preset="idleTx" />);
    await user.selectOptions(screen.getByRole("combobox", { name: "Группировать" }), "user");
    const table = screen.getByRole("table", { name: "Группы сессий" });
    const group = (name: RegExp) => within(table).getByRole("row", { name }).querySelectorAll("th, td");
    expect([...group(/^app/)].map((c) => c.textContent)).toEqual(["app", "5", "1", "1", "2", "1"]);
    expect([...group(/^\(не задано\)/)].map((c) => c.textContent)).toEqual(["(не задано)", "1", "1", "0", "0", "0"]);
    expect(within(table).getByRole("columnheader", { name: "Роль" })).toBeInTheDocument();
    // Список сессий при группировке не показывается.
    expect(screen.queryByRole("columnheader", { name: "Простой" })).not.toBeInTheDocument();
    await user.selectOptions(screen.getByRole("combobox", { name: "Группировать" }), "none");
    expect(screen.getByRole("columnheader", { name: "Простой" })).toBeInTheDocument();
  });

  it("ждущие: pid блокирующих; 0 — prepared transaction или завершившаяся сессия, а не «сессия 0»", async () => {
    const user = userEvent.setup();
    const many = [sessionRow({ pid: 7, waitEventType: "Lock", waitEvent: "relation", blockedBy: [0, 9] }), sessionRow({ pid: 8, state: "idle" })];
    renderWithClient(<SessionsView report={report(many)} preset="all" />);
    await user.click(screen.getByRole("tab", { name: "Ждущие" }));
    expect(pidsShown()).toEqual(["7"]);
    expect(rowOf(7)).toHaveTextContent("prepared transaction или завершившаяся сессия, 9");
  });

  it("настройка таймаута: ненулевое значение — длительностью; значение неизвестно — «—»", () => {
    const { rerender } = renderWithClient(<SessionsView report={report(rows, { idleInTransactionSessionTimeoutMs: 60_000, trackActivityQuerySize: null })} preset="idleTx" />);
    expect(screen.getByText(/idle_in_transaction_session_timeout = 1 мин/)).toBeInTheDocument();
    rerender(<SessionsView report={report(rows, { idleInTransactionSessionTimeoutMs: null, trackActivityQuerySize: null })} preset="idleTx" />);
    expect(screen.getByText(/idle_in_transaction_session_timeout = —/)).toBeInTheDocument();
  });

  it("панель: поля сессии, пометка об обрезанном тексте и закрытие; <insufficient privilege> показывается как есть", async () => {
    const user = userEvent.setup();
    const many = [
      sessionRow({ pid: 20, query: "select pg_sleep(100)", queryTruncated: true, user: "alice", clientAddr: "10.0.0.9", clientPort: 4242, xactAgeS: 90, queryId: "-8123456789012345678" }),
      sessionRow({ pid: 21, query: "<insufficient privilege>" }),
    ];
    renderWithClient(<SessionsView report={report(many)} preset="all" />);
    await user.click(rowOf(20));
    const panel = screen.getByRole("region", { name: "Сессия 20" });
    expect(panel).toHaveTextContent("alice");
    expect(panel).toHaveTextContent("10.0.0.9:4242");
    expect(panel).toHaveTextContent("1 мин 30 с");
    expect(panel).toHaveTextContent("-8123456789012345678"); // 64-битный хеш показан как пришёл, без округления
    expect(panel).toHaveTextContent("Текст запроса обрезан до 16 KB");
    await user.click(rowOf(21));
    expect(screen.queryByRole("region", { name: "Сессия 20" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Сессия 21" })).toHaveTextContent("<insufficient privilege>");
    expect(screen.queryByText(/обрезан/)).not.toBeInTheDocument();
    await user.click(within(screen.getByRole("region", { name: "Сессия 21" })).getByRole("button", { name: "Закрыть" }));
    expect(screen.queryByRole("region", { name: "Сессия 21" })).not.toBeInTheDocument();
  });

  it("панель остаётся с последними данными, если сессия исчезла из списка, и обновляется, пока она есть", async () => {
    const user = userEvent.setup();
    const a = sessionRow({ pid: 30, query: "select 1", state: "active" });
    const { rerender } = renderWithClient(<SessionsView report={report([a])} preset="all" />);
    await user.click(rowOf(30));
    rerender(<SessionsView report={report([{ ...a, query: "select 2" }])} preset="all" />);
    expect(screen.getByRole("region", { name: "Сессия 30" })).toHaveTextContent("select 2");
    rerender(<SessionsView report={report([])} preset="all" />);
    const panel = screen.getByRole("region", { name: "Сессия 30" });
    expect(panel).toHaveTextContent("select 2");
    expect(panel).toHaveTextContent("Сессии больше нет в списке");
  });

  it("клавиатура: Enter на строке открывает панель, фокус уходит в её заголовок, панель прокручивается в видимую область", async () => {
    const user = userEvent.setup();
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll;
    renderWithClient(<SessionsView report={report()} preset="all" />);
    act(() => rowOf(2).focus());
    await user.keyboard("{Enter}");
    const panel = screen.getByRole("region", { name: "Сессия 2" });
    expect(within(panel).getByRole("heading", { name: "Сессия 2" })).toHaveFocus();
    expect(scroll).toHaveBeenCalledTimes(1);
    expect(scroll.mock.contexts[0]).toBe(panel);
  });

  it("клик по строке тоже переводит фокус в заголовок панели; обновление данных и смена группировки фокус не отнимают", async () => {
    const user = userEvent.setup();
    Element.prototype.scrollIntoView = vi.fn();
    const a = sessionRow({ pid: 50, query: "select 1" });
    const { rerender } = renderWithClient(<SessionsView report={report([a])} preset="all" />);
    await user.click(rowOf(50));
    expect(screen.getByRole("heading", { name: "Сессия 50" })).toHaveFocus();
    // Пользователь ушёл в фильтр: новый отчёт не должен возвращать его в панель.
    const filter = screen.getByRole("textbox", { name: "Фильтр" });
    act(() => filter.focus());
    rerender(<SessionsView report={report([{ ...a, query: "select 2" }])} preset="all" />);
    expect(screen.getByRole("region", { name: "Сессия 50" })).toHaveTextContent("select 2");
    expect(filter).toHaveFocus();
    // Группировка прячет панель, возврат к списку возвращает её, но фокус остаётся у переключателя.
    const groupBy = screen.getByRole("combobox", { name: "Группировать" });
    await user.selectOptions(groupBy, "user");
    await user.selectOptions(groupBy, "none");
    expect(screen.getByRole("region", { name: "Сессия 50" })).toBeInTheDocument();
    expect(groupBy).toHaveFocus();
    // Повторный клик по той же строке — новое открытие: фокус снова в панели.
    await user.click(rowOf(50));
    expect(screen.getByRole("heading", { name: "Сессия 50" })).toHaveFocus();
  });

  it("в порядке Tab одна строка списка, между строками — стрелки; Tab-остановка следует за фокусом", async () => {
    const user = userEvent.setup();
    const many = [sessionRow({ pid: 1 }), sessionRow({ pid: 2 }), sessionRow({ pid: 3 })];
    renderWithClient(<SessionsView report={report(many)} preset="all" />);
    const tabbable = () => screen.getAllByRole("row").filter((r) => r.tabIndex === 0).map((r) => within(r).queryAllByRole("cell")[0].textContent);
    expect(tabbable()).toEqual(["1"]);
    act(() => rowOf(1).focus());
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(rowOf(3)).toHaveFocus();
    expect(tabbable()).toEqual(["3"]);
    await user.keyboard("{ArrowUp}");
    expect(rowOf(2)).toHaveFocus();
    await user.keyboard("{ArrowUp}{ArrowUp}"); // выше первой строки — остаётся на ней
    expect(rowOf(1)).toHaveFocus();
  });

  it("длинный список виртуализируется: в DOM только окно строк, а не все", () => {
    const many = Array.from({ length: 500 }, (_, i) => sessionRow({ pid: 1000 + i, query: `select ${i}` }));
    win.from = 100;
    renderWithClient(<SessionsView report={report(many)} preset="all" />);
    expect(pidsShown()).toEqual(Array.from({ length: 10 }, (_, i) => String(1100 + i)));
    expect(screen.getByRole("table", { name: "Список сессий" })).toHaveAttribute("aria-rowcount", "501");
    expect(screen.getByText("Показано 500 из 500")).toBeInTheDocument();
  });

  it("текст запроса в списке — одной строкой и не длиннее 120 символов", () => {
    const long = `select\n  ${"x".repeat(200)}`;
    renderWithClient(<SessionsView report={report([sessionRow({ pid: 40, query: long })])} preset="all" />);
    const cells = within(rowOf(40)).getAllByRole("cell");
    const text = cells[cells.length - 1];
    expect(text.textContent).toHaveLength(121); // 120 символов и «…»
    expect(text.textContent).toMatch(/^select x+…$/);
  });
});
