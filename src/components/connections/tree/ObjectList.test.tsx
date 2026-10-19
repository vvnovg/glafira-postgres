import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ObjectItem, ObjectPage } from "../../../api/types";
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

function renderList(count: number) {
  return renderWithClient(<ObjectList connectionId="c" db="orders" schema="shop" kind="tables" count={count} label="Таблицы" />);
}

beforeEach(() => {
  vi.clearAllMocks();
  win.from = 0;
  win.size = 10;
});

describe("ObjectList", () => {
  it("короткий список целиком, итог и кнопка «Ещё» догружает страницу", async () => {
    vi.mocked(api.listObjects).mockResolvedValueOnce(page(0, 3, 5)).mockResolvedValueOnce(page(3, 2, 5));
    const user = userEvent.setup();
    renderList(5);
    expect(await screen.findByText("t3")).toBeInTheDocument();
    expect(screen.getByText("Показано 3 из 5")).toBeInTheDocument();
    expect(screen.queryByLabelText("Фильтр: Таблицы")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Ещё" }));
    expect(await screen.findByText("t5")).toBeInTheDocument();
    expect(api.listObjects).toHaveBeenLastCalledWith("c", "orders", "shop", "tables", null, 3, PAGE);
    expect(screen.getByText("Показано 5 из 5")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Ещё" })).not.toBeInTheDocument();
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
    expect(screen.getByText("2,0 KB")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^t1/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^events/ }));
    expect(await screen.findByText("events_2026_10")).toBeInTheDocument();
    expect(api.listPartitions).toHaveBeenCalledWith("c", "orders", "shop", "events", 0, PAGE);
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
});
