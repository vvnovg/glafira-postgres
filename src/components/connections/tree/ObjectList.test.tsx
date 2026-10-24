import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ObjectItem, ObjectKind, ObjectPage } from "../../../api/types";
import { useWorkspace } from "../../../state/workspace";
import { renderWithClient } from "../../../test/render";

// В jsdom у элементов нет размеров, и настоящий виртуализатор ничего не показывает: окно задаётся тестом.
const win = vi.hoisted(() => ({ from: 0, size: 10 }));
vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: (o: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: Math.max(0, Math.min(win.size, o.count - win.from)) }, (_, i) => ({ index: win.from + i, start: (win.from + i) * 22 })),
    getTotalSize: () => o.count * 22,
    measureElement: () => {},
  }),
}));

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return { ...orig, api: { listObjects: vi.fn(), listPartitions: vi.fn() } };
});

import { api } from "../../../api/client";
import { ObjectList, PAGE } from "./ObjectList";

const item = (oid: number, patch: Partial<ObjectItem> = {}): ObjectItem => ({
  oid,
  name: `t${oid}`,
  schema: "shop",
  relkind: "r",
  partitions: null,
  estRows: null,
  totalBytes: null,
  signature: null,
  ...patch,
});

/** Страница из `count` объектов с номерами от `from`; nextOffset — если за ней есть ещё. */
const page = (from: number, count: number, total: number): ObjectPage => ({
  total,
  items: Array.from({ length: count }, (_, i) => item(from + i + 1)),
  nextOffset: from + count < total ? from + count : null,
});

const listEl = (count: number) => <ObjectList connectionId="c" db="orders" schema="shop" kind="tables" count={count} label="Таблицы" />;

function renderList(count: number) {
  return renderWithClient(listEl(count));
}

/** Список объектов другого вида (функции, последовательности, представления). */
function renderListOf(kind: ObjectKind, count: number) {
  return renderWithClient(<ObjectList connectionId="c" db="orders" schema="shop" kind={kind} count={count} label="Объекты" />);
}

/** Повторная отрисовка с тем же клиентом: окно виртуализатора в тесте читается при каждом рендере. */
function rerenderList(r: ReturnType<typeof renderList>, count: number) {
  r.rerender(listEl(count));
}

beforeEach(() => {
  vi.clearAllMocks();
  win.from = 0;
  win.size = 10;
  useWorkspace.setState({ tabs: [], activeId: null });
});

describe("ObjectList", () => {
  it("короткий список целиком, итог и кнопка «Ещё» догружает страницу", async () => {
    vi.mocked(api.listObjects).mockResolvedValueOnce(page(0, 3, 5)).mockResolvedValueOnce(page(3, 2, 5));
    const user = userEvent.setup();
    renderList(5);
    expect(await screen.findByText("t3")).toBeInTheDocument();
    expect(screen.getByText("Показано 3 из 5")).toBeInTheDocument();
    expect(screen.queryByLabelText(/^Фильтр: /)).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Ещё: Таблицы (orders.shop)" }));
    expect(await screen.findByText("t5")).toBeInTheDocument();
    expect(api.listObjects).toHaveBeenLastCalledWith("c", "orders", "shop", "tables", null, 3, PAGE);
    expect(screen.getByText("Показано 5 из 5")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Ещё/ })).not.toBeInTheDocument();
  });

  it("пустой результат фильтра", async () => {
    vi.mocked(api.listObjects).mockResolvedValue({ total: 0, items: [], nextOffset: null });
    renderList(5);
    expect(await screen.findByText("Ничего не найдено")).toBeInTheDocument();
  });

  it("длинный список виртуализируется: в DOM только окно, пока окно в начале — страница не догружается", async () => {
    vi.mocked(api.listObjects).mockResolvedValue(page(0, 300, 600));
    renderList(600);
    expect(await screen.findByText("t10")).toBeInTheDocument();
    expect(screen.queryByText("t11")).not.toBeInTheDocument();
    await new Promise((r) => setTimeout(r, 50));
    expect(api.listObjects).toHaveBeenCalledTimes(1);
    // Строки лежат прямо в списке, а не во вложенных div.
    expect(screen.getByText("t1").closest("li")?.parentElement?.tagName).toBe("UL");
    expect(screen.getByText("Показано 300 из 600")).toBeInTheDocument();
  });

  it("у конца загруженного окно подгружает следующую страницу", async () => {
    win.from = 290;
    vi.mocked(api.listObjects).mockResolvedValueOnce(page(0, 300, 600)).mockResolvedValueOnce(page(300, 300, 600));
    renderList(600);
    await waitFor(() => expect(api.listObjects).toHaveBeenCalledWith("c", "orders", "shop", "tables", null, 300, PAGE));
    expect(await screen.findByText("Показано 600 из 600")).toBeInTheDocument();
  });

  it("ошибка следующей страницы не зацикливает запросы; «Повторить» запрашивает её снова", async () => {
    win.from = 290;
    vi.mocked(api.listObjects)
      .mockResolvedValueOnce(page(0, 300, 600))
      .mockRejectedValueOnce({ code: "TIMEOUT", message: "slow", retryable: true })
      .mockResolvedValueOnce(page(300, 300, 600));
    const user = userEvent.setup();
    renderList(600);
    expect(await screen.findByRole("alert")).toHaveTextContent("slow");
    await new Promise((r) => setTimeout(r, 50));
    expect(api.listObjects).toHaveBeenCalledTimes(2);

    await user.click(screen.getByRole("button", { name: "Повторить" }));
    await waitFor(() => expect(api.listObjects).toHaveBeenCalledTimes(3));
    expect(vi.mocked(api.listObjects).mock.calls[2][5]).toBe(300);
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
  });

  it("размер партиционированной таблицы неизвестен — прочерк с пояснением, у обычной таблицы — размер", async () => {
    vi.mocked(api.listObjects).mockResolvedValue({
      total: 2,
      nextOffset: null,
      items: [item(1, { totalBytes: 8192 }), item(2, { name: "events", relkind: "p", partitions: 3, totalBytes: null })],
    });
    renderList(2);
    expect(await screen.findByText("8,0 KB")).toBeInTheDocument();
    const dash = screen.getByRole("img", { name: "Размер партиционированной таблицы в списке не считается" });
    expect(dash).toHaveTextContent("—");
    expect(screen.getAllByRole("img", { name: /Размер партиционированной/ })).toHaveLength(1);
  });

  it("партиции: у партиционированной таблицы — счётчик и раскрытие, у обычной — нет", async () => {
    vi.mocked(api.listObjects).mockResolvedValue({
      total: 2,
      nextOffset: null,
      items: [item(1), item(2, { name: "events", relkind: "p", partitions: 3, totalBytes: 2048 })],
    });
    vi.mocked(api.listPartitions).mockResolvedValue({ total: 1, nextOffset: null, items: [item(9, { name: "events_2026_10" })] });
    const user = userEvent.setup();
    renderList(2);
    expect(await screen.findByText("3 партиции")).toBeInTheDocument();
    // Стрелка и кнопка открытия — разные кнопки с разными именами (скринридер их различает).
    expect(screen.getByRole("button", { name: "Развернуть или свернуть партиции events" })).not.toBe(screen.getByRole("button", { name: "Открыть events" }));
    expect(screen.getByText("2,0 KB")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^t1/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Развернуть или свернуть партиции events" }));
    expect(await screen.findByText("events_2026_10")).toBeInTheDocument();
    expect(api.listPartitions).toHaveBeenCalledWith("c", "orders", "shop", "events", 0, PAGE);
  });

  it("клик по имени объекта открывает вкладку своего вида", async () => {
    vi.mocked(api.listObjects).mockResolvedValue({ total: 2, nextOffset: null, items: [item(1, { name: "orders" }), item(2, { name: "events", relkind: "p", partitions: 2 })] });
    const user = userEvent.setup();
    renderList(2);
    await user.click(await screen.findByRole("button", { name: "Открыть orders" }));
    expect(useWorkspace.getState().tabs).toEqual([{ kind: "table", connectionId: "c", database: "orders", schema: "shop", name: "orders", id: "table:c:orders:shop.orders" }]);
    // Партиционированная таблица: имя открывает вкладку, стрелка раскрывает партиции.
    await user.click(screen.getByRole("button", { name: "Открыть events" }));
    expect(useWorkspace.getState().activeId).toBe("table:c:orders:shop.events");
  });

  it("у партиционированной таблицы стрелка раскрывает партиции и не открывает вкладку", async () => {
    vi.mocked(api.listObjects).mockResolvedValue({ total: 1, nextOffset: null, items: [item(2, { name: "events", relkind: "p", partitions: 2 })] });
    vi.mocked(api.listPartitions).mockResolvedValue({
      total: 2,
      nextOffset: null,
      items: [item(9, { name: "events_2026_10" }), item(10, { name: "events_old", schema: "archive" })],
    });
    const user = userEvent.setup();
    renderList(1);
    await user.click(await screen.findByRole("button", { name: "Развернуть или свернуть партиции events" }));
    expect(useWorkspace.getState().tabs).toEqual([]);
    // Партиция открывается как таблица своей схемы — не схемы списка.
    await user.click(await screen.findByRole("button", { name: "Открыть archive.events_old" }));
    expect(useWorkspace.getState().tabs).toEqual([
      { kind: "table", connectionId: "c", database: "orders", schema: "archive", name: "events_old", id: "table:c:orders:archive.events_old" },
    ]);
  });

  it("функция открывается по oid, представление — с признаком мат. представления", async () => {
    vi.mocked(api.listObjects).mockResolvedValue({ total: 1, nextOffset: null, items: [item(77, { name: "order_count", signature: "c bigint" })] });
    const user = userEvent.setup();
    renderListOf("functions", 1);
    await user.click(await screen.findByRole("button", { name: "Открыть order_count(c bigint)" }));
    expect(useWorkspace.getState().tabs[0]).toMatchObject({ kind: "function", oid: 77, name: "order_count" });
    vi.mocked(api.listObjects).mockResolvedValue({ total: 1, nextOffset: null, items: [item(5, { name: "order_totals", relkind: "m" })] });
    renderListOf("matviews", 1);
    await user.click(await screen.findByRole("button", { name: "Открыть order_totals" }));
    expect(useWorkspace.getState().tabs.slice(-1)[0]).toMatchObject({ kind: "view", matview: true, name: "order_totals" });
  });

  it("обычное представление и последовательность открываются своими вкладками", async () => {
    vi.mocked(api.listObjects).mockResolvedValue({ total: 1, nextOffset: null, items: [item(5, { name: "v_orders", relkind: "v" })] });
    const user = userEvent.setup();
    const first = renderListOf("views", 1);
    await user.click(await first.findByRole("button", { name: "Открыть v_orders" }));
    expect(useWorkspace.getState().tabs.slice(-1)[0]).toMatchObject({ kind: "view", matview: false, name: "v_orders", schema: "shop" });
    first.unmount();
    vi.mocked(api.listObjects).mockResolvedValue({ total: 1, nextOffset: null, items: [item(6, { name: "orders_id_seq", relkind: "S" })] });
    renderListOf("sequences", 1);
    await user.click(await screen.findByRole("button", { name: "Открыть orders_id_seq" }));
    expect(useWorkspace.getState().tabs.slice(-1)[0]).toEqual({
      kind: "sequence",
      connectionId: "c",
      database: "orders",
      schema: "shop",
      name: "orders_id_seq",
      id: "sequence:c:orders:shop.orders_id_seq",
    });
  });

  it("функции показываются с сигнатурой", async () => {
    vi.mocked(api.listObjects).mockResolvedValue({
      total: 1,
      nextOffset: null,
      items: [item(1, { name: "total", relkind: null, signature: "integer, text" })],
    });
    renderList(1);
    expect(await screen.findByText("total(integer, text)")).toBeInTheDocument();
  });

  it("перекрывающиеся страницы: повторов нет, ключи React уникальны", async () => {
    // Между запросами список сдвинулся: объект 3 пришёл и в конце первой страницы, и в начале второй.
    vi.mocked(api.listObjects)
      .mockResolvedValueOnce({ total: 6, nextOffset: 3, items: [item(1), item(2), item(3)] })
      .mockResolvedValueOnce({ total: 6, nextOffset: null, items: [item(3), item(4), item(5)] });
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const user = userEvent.setup();
    renderList(6);
    await screen.findByText("t3");
    await user.click(screen.getByRole("button", { name: /^Ещё/ }));
    expect(await screen.findByText("t5")).toBeInTheDocument();
    expect(screen.getAllByText("t3")).toHaveLength(1);
    expect(screen.getByText("Показано 5 из 6")).toBeInTheDocument();
    expect(errors.mock.calls.filter((c) => String(c[0]).includes("same key"))).toEqual([]);
    errors.mockRestore();
  });

  it("перекрывающиеся страницы в виртуальном списке считаются без повторов", async () => {
    win.from = 290;
    vi.mocked(api.listObjects)
      .mockResolvedValueOnce(page(0, 300, 600))
      // Вторая страница начинается с объекта 296: пять объектов уже были на первой.
      .mockResolvedValueOnce({ total: 600, nextOffset: null, items: Array.from({ length: 300 }, (_, i) => item(296 + i)) });
    renderList(600);
    expect(await screen.findByText("Показано 595 из 600")).toBeInTheDocument();
  });

  it("раскрытая партиция остаётся раскрытой, когда строка уходит из окна и возвращается", async () => {
    vi.mocked(api.listObjects).mockResolvedValue({
      total: 300,
      nextOffset: null,
      items: Array.from({ length: 300 }, (_, i) => (i === 1 ? item(2, { name: "events", relkind: "p", partitions: 1 }) : item(i + 1))),
    });
    vi.mocked(api.listPartitions).mockResolvedValue({ total: 1, nextOffset: null, items: [item(900, { name: "events_2026_10" })] });
    const user = userEvent.setup();
    const r = renderList(300);
    await user.click(await screen.findByRole("button", { name: "Развернуть или свернуть партиции events" }));
    expect(await screen.findByText("events_2026_10")).toBeInTheDocument();

    win.from = 100; // строка events вышла из окна и размонтирована
    rerenderList(r, 300);
    await waitFor(() => expect(screen.queryByRole("button", { name: "Развернуть или свернуть партиции events" })).not.toBeInTheDocument());

    win.from = 0; // вернулась — всё ещё раскрыта
    rerenderList(r, 300);
    expect(await screen.findByRole("button", { name: "Развернуть или свернуть партиции events" })).toHaveAttribute("aria-expanded", "true");
    expect(await screen.findByText("events_2026_10")).toBeInTheDocument();
  });

  it("смена фильтра возвращает прокрутку виртуального списка в начало", async () => {
    vi.mocked(api.listObjects).mockResolvedValue(page(0, 300, 600));
    renderList(600);
    // Список загружается на настоящих таймерах (react-query отдаёт результат через setTimeout), фейковые — только на задержку фильтра.
    const scroller = await screen.findByRole("group", { name: "Список: Таблицы (orders.shop)" });
    scroller.scrollTop = 400;
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      // Не user.type: asyncWrapper Testing Library ждёт setTimeout(0) и продвигает его только под jest, под vitest он завис бы.
      fireEvent.change(screen.getByLabelText("Фильтр: Таблицы (orders.shop)"), { target: { value: "a" } });
      // Задержка фильтра — 300 мс от последнего нажатия: за миллисекунду до неё фильтр не применён и прокрутка на месте.
      await act(() => vi.advanceTimersByTimeAsync(299));
      expect(scroller.scrollTop).toBe(400);
      expect(api.listObjects).toHaveBeenCalledTimes(1);
      await act(() => vi.advanceTimersByTimeAsync(1));
      expect(api.listObjects).toHaveBeenLastCalledWith("c", "orders", "shop", "tables", "a", 0, PAGE);
      expect(scroller.scrollTop).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("партиция из другой схемы подписана схемой, из той же — нет", async () => {
    vi.mocked(api.listObjects).mockResolvedValue({
      total: 1,
      nextOffset: null,
      items: [item(2, { name: "events", relkind: "p", partitions: 2 })],
    });
    vi.mocked(api.listPartitions).mockResolvedValue({
      total: 2,
      nextOffset: null,
      items: [item(9, { name: "events_2026_10" }), item(10, { name: "events_old", schema: "archive" })],
    });
    const user = userEvent.setup();
    renderList(1);
    await user.click(await screen.findByRole("button", { name: "Развернуть или свернуть партиции events" }));
    expect(await screen.findByText("archive.events_old")).toBeInTheDocument();
    expect(screen.getByText("events_2026_10")).toBeInTheDocument();
    expect(screen.queryByText("shop.events_2026_10")).not.toBeInTheDocument();
  });

  it("доступные имена: прокручиваемые области и поля фильтра разных списков различимы", async () => {
    vi.mocked(api.listObjects).mockResolvedValue(page(0, 300, 600));
    renderWithClient(
      <>
        <ObjectList connectionId="c" db="orders" schema="shop" kind="tables" count={600} label="Таблицы" />
        <ObjectList connectionId="c" db="orders" schema="audit" kind="views" count={600} label="Представления" />
      </>,
    );
    await waitFor(() => expect(screen.getAllByRole("group")).toHaveLength(2));
    const scrollers = screen.getAllByRole("group");
    expect(scrollers.map((g) => g.getAttribute("aria-label"))).toEqual(["Список: Таблицы (orders.shop)", "Список: Представления (orders.audit)"]);
    // Область прокрутки достижима с клавиатуры.
    for (const g of scrollers) expect(g).toHaveAttribute("tabindex", "0");
    expect(screen.getByLabelText("Фильтр: Таблицы (orders.shop)")).toBeInTheDocument();
    expect(screen.getByLabelText("Фильтр: Представления (orders.audit)")).toBeInTheDocument();
  });

  it("доступные имена: кнопки «Ещё» разных списков называют свой список", async () => {
    vi.mocked(api.listObjects).mockResolvedValue({ total: 9, nextOffset: 3, items: [item(1), item(2), item(3)] });
    renderWithClient(
      <>
        <ObjectList connectionId="c" db="orders" schema="shop" kind="tables" count={9} label="Таблицы" />
        <ObjectList connectionId="c" db="orders" schema="audit" kind="views" count={9} label="Представления" />
      </>,
    );
    await waitFor(() => expect(screen.getAllByRole("button", { name: /^Ещё/ })).toHaveLength(2));
    expect(screen.getByRole("button", { name: "Ещё: Таблицы (orders.shop)" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Ещё: Представления (orders.audit)" })).toBeInTheDocument();
  });

  it("«Ещё» в списке партиций называет таблицу", async () => {
    vi.mocked(api.listObjects).mockResolvedValue({ total: 1, nextOffset: null, items: [item(2, { name: "events", relkind: "p", partitions: 5 })] });
    vi.mocked(api.listPartitions).mockResolvedValue({ total: 5, nextOffset: 2, items: [item(9), item(10)] });
    const user = userEvent.setup();
    renderList(1);
    await user.click(await screen.findByRole("button", { name: "Развернуть или свернуть партиции events" }));
    expect(await screen.findByRole("button", { name: "Ещё партиции: orders.shop.events" })).toBeInTheDocument();
  });

  it("индикаторы загрузки — role=status", async () => {
    vi.mocked(api.listObjects).mockResolvedValueOnce({ total: 1, nextOffset: null, items: [item(2, { name: "events", relkind: "p", partitions: 1 })] });
    vi.mocked(api.listPartitions).mockReturnValue(new Promise(() => {}));
    const user = userEvent.setup();
    const r = renderList(1);
    await user.click(await screen.findByRole("button", { name: "Развернуть или свернуть партиции events" }));
    expect(within(r.container).getByRole("status")).toHaveTextContent("Загрузка…");
    r.unmount();

    vi.mocked(api.listObjects).mockReturnValue(new Promise(() => {}));
    renderList(1);
    expect(screen.getByRole("status")).toHaveTextContent("Загрузка…");
  });
});
