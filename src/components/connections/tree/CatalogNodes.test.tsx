import { act, fireEvent, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderWithClient } from "../../../test/render";
import { topologyView } from "../../../test/fixtures";
import { newProfile } from "../profile";

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return {
    ...orig,
    api: {
      getUiSettings: vi.fn(() => Promise.resolve({ showSystemObjects: false })),
      listDatabases: vi.fn(() => Promise.resolve([{ name: "orders", sizeBytes: 5 * 1024 * 1024 }])),
      listSchemas: vi.fn(() =>
        Promise.resolve([{ name: "shop", system: false, counts: { tables: 3, views: 1, matviews: 0, sequences: 3, functions: 3 } }]),
      ),
      listObjects: vi.fn(() =>
        Promise.resolve({
          total: 2,
          nextOffset: null,
          items: [
            { oid: 1, name: "customers", schema: "shop", relkind: "r", partitions: null, estRows: 100, totalBytes: 8192, signature: null },
            { oid: 2, name: "events", schema: "shop", relkind: "p", partitions: 2, estRows: null, totalBytes: 0, signature: null },
          ],
        }),
      ),
      listPartitions: vi.fn(() =>
        Promise.resolve({ total: 1, nextOffset: null, items: [{ oid: 3, name: "events_2026_10", schema: "shop", relkind: "r", partitions: null, estRows: 0, totalBytes: 0, signature: null }] }),
      ),
      listExtensions: vi.fn(() => Promise.resolve([])),
      listRoles: vi.fn(() => Promise.resolve([])),
      setCatalogMember: vi.fn(),
    },
  };
});

import { api } from "../../../api/client";
import { CatalogNodes } from "./CatalogNodes";

const conn = { ...newProfile(), id: "c", name: "billing", kind: "patroni" as const };

beforeEach(() => vi.clearAllMocks());

describe("CatalogNodes", () => {
  it("узлы с ролями и ленивая загрузка каталога", async () => {
    const user = userEvent.setup();
    renderWithClient(<CatalogNodes connection={conn} view={topologyView()} />);
    await user.click(screen.getByRole("button", { name: /^Узлы/ }));
    expect(screen.getByRole("button", { name: "★ pg-1" })).toBeInTheDocument();
    expect(screen.getByText("Leader")).toBeInTheDocument();
    expect(api.listDatabases).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: /Базы данных \(с узла pg-1\)/ }));
    await user.click(await screen.findByRole("button", { name: /^orders/ }));
    await user.click(await screen.findByRole("button", { name: /^shop/ }));
    expect(screen.queryByRole("button", { name: /Мат\. представления/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^Таблицы \(3\)/ }));
    expect(await screen.findByText("customers")).toBeInTheDocument();
    expect(api.listSchemas).toHaveBeenCalledWith("c", "orders", false);
    expect(api.listObjects).toHaveBeenCalledWith("c", "orders", "shop", "tables", null, 0, 500);

    await user.click(screen.getByRole("button", { name: "Развернуть или свернуть партиции events" }));
    expect(await screen.findByText("events_2026_10")).toBeInTheDocument();
  });

  it("группа «Диагностика» — после «Ролей»; у сервера без инструментов Patroni, у Patroni с ними", async () => {
    const user = userEvent.setup();
    const { unmount } = renderWithClient(<CatalogNodes connection={conn} view={topologyView()} />);
    const toggles = screen.getAllByRole("button").map((b) => b.textContent?.replace(/^[▸▾]/, ""));
    expect(toggles.slice(-3)).toEqual(["Базы данных (с узла pg-1)", "Роли", "Диагностика"]);
    await user.click(screen.getByRole("button", { name: "Диагностика" }));
    expect(screen.getByRole("button", { name: "Матрица health" })).toBeInTheDocument();
    unmount();

    renderWithClient(<CatalogNodes connection={{ ...conn, kind: "server" }} view={topologyView({ topology: { ...topologyView().topology!, kind: "server" } })} />);
    await user.click(screen.getByRole("button", { name: "Диагностика" }));
    expect(screen.getByRole("button", { name: "Слоты репликации" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Матрица health" })).not.toBeInTheDocument();
  });

  it("фильтр объектов уходит в запрос", async () => {
    const user = userEvent.setup();
    // Поле фильтра появляется у групп больше 20 объектов.
    vi.mocked(api.listSchemas).mockResolvedValueOnce([
      { name: "shop", system: false, counts: { tables: 30, views: 0, matviews: 0, sequences: 0, functions: 0 } },
    ]);
    renderWithClient(<CatalogNodes connection={{ ...conn, kind: "server" }} view={topologyView({ topology: { ...topologyView().topology!, kind: "server" } })} />);
    expect(screen.queryByRole("button", { name: /^Узлы/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Базы данных/ }));
    await user.click(await screen.findByRole("button", { name: /^orders/ }));
    await user.click(await screen.findByRole("button", { name: /^shop/ }));
    await user.click(screen.getByRole("button", { name: /^Таблицы/ }));
    const input = await screen.findByLabelText("Фильтр: Таблицы (orders.shop)");
    const filtered = () => vi.mocked(api.listObjects).mock.calls.filter((call) => call[4] !== null);
    expect(filtered()).toHaveLength(0);

    // Задержка фильтра — 300 мс от последнего нажатия: каждое новое откладывает запрос заново.
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      fireEvent.change(input, { target: { value: "c" } });
      await act(() => vi.advanceTimersByTimeAsync(200));
      fireEvent.change(input, { target: { value: "cust" } });
      await act(() => vi.advanceTimersByTimeAsync(299));
      expect(filtered()).toHaveLength(0);
      await act(() => vi.advanceTimersByTimeAsync(1));
      expect(filtered()).toEqual([["c", "orders", "shop", "tables", "cust", 0, 500]]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("уведомление о возврате к узлу по умолчанию", () => {
    renderWithClient(<CatalogNodes connection={conn} view={topologyView({ catalogFallback: true })} />);
    expect(screen.getByText(/Выбранный узел недоступен/)).toBeInTheDocument();
  });
});
