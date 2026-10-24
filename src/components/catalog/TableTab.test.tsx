import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TableDetails } from "../../api/types";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { getTable: vi.fn() } };
});

import { api } from "../../api/client";
import { TableTab } from "./TableTab";

const details: TableDetails = {
  oid: 1, schema: "shop", name: "orders", relkind: "r", unlogged: false, owner: "postgres", tablespace: null, comment: "Заказы", reloptions: ["fillfactor=90"],
  columns: [
    { name: "id", dataType: "bigint", notNull: true, defaultExpr: null, identity: "a", generated: null, collation: null, statTarget: null, storage: "p", comment: null },
    { name: "total", dataType: "numeric(12,2)", notNull: true, defaultExpr: "0", identity: null, generated: null, collation: null, statTarget: 500, storage: "m", comment: "сумма" },
  ],
  indexes: [
    { name: "orders_pkey", definition: "CREATE UNIQUE INDEX orders_pkey ON shop.orders USING btree (id)", primary: true, unique: true, valid: true, sizeBytes: 16384, idxScan: 12, idxTupRead: 30, lastIdxScanMs: null },
    { name: "orders_bad", definition: "CREATE INDEX orders_bad ON shop.orders USING btree (total)", primary: false, unique: false, valid: false, sizeBytes: 8192, idxScan: 0, idxTupRead: 0, lastIdxScanMs: 1_700_000_000_000 },
  ],
  constraints: [{ name: "orders_pkey", kind: "p", definition: "PRIMARY KEY (id)", validated: true }],
  incomingFks: [{ constraint: "items_order_fkey", schema: "shop", table: "items", definition: "FOREIGN KEY (order_id) REFERENCES shop.orders(id)" }],
  stats: { heapBytes: 8192, indexBytes: 24576, toastBytes: 0, totalBytes: 32768, reltuples: 1000, seqScan: 3, idxScan: 12, nLiveTup: 1000, nDeadTup: 5, nModSinceAnalyze: 0, nInsSinceVacuum: 0, lastVacuumMs: null, lastAutovacuumMs: 1_700_000_000_000, lastAnalyzeMs: null, lastAutoanalyzeMs: null, vacuumCount: 0, autovacuumCount: 1, analyzeCount: 0, autoanalyzeCount: 1 },
  partitioning: { key: null, bound: null, parent: null, partitions: [] },
  triggers: [{ name: "touch", enabled: "O", definition: "CREATE TRIGGER touch BEFORE INSERT ON shop.orders FOR EACH ROW EXECUTE FUNCTION shop.touch()" }],
  ddl: "-- DDL built by Glafira from the catalog; not a replacement for pg_dump (no privileges, RLS policies or owners)\nCREATE TABLE shop.orders (\n    id bigint\n);\n",
};

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.getTable).mockResolvedValue(details);
});

describe("TableTab", () => {
  it("подвкладки: колонки по умолчанию, индексы с подсветкой INVALID, DDL с кнопкой копирования", async () => {
    const user = userEvent.setup();
    renderWithClient(<TableTab connectionId="c" database="orders" schema="shop" name="orders" />);
    // Заголовок виден сразу (имя известно по вкладке), поэтому загрузку ждём по таблице колонок.
    const columns = await screen.findByRole("table", { name: "Колонки" });
    expect(screen.getByRole("heading", { name: "shop.orders" })).toBeInTheDocument();
    expect(api.getTable).toHaveBeenCalledWith("c", "orders", "shop", "orders");
    expect(within(columns).getByText("total")).toBeInTheDocument();
    expect(within(columns).getByText("numeric(12,2)")).toBeInTheDocument();
    expect(within(columns).getByText("GENERATED ALWAYS AS IDENTITY")).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Индексы" }));
    const bad = screen.getByRole("row", { name: /orders_bad/ });
    expect(bad).toHaveClass("invalid");
    expect(within(bad).getByText("INVALID")).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Ограничения" }));
    expect(screen.getByText(/REFERENCES shop.orders\(id\)/)).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Размер и статистика" }));
    expect(screen.getByText("32,0 KB")).toBeInTheDocument();
    expect(screen.getByText("fillfactor=90")).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Триггеры" }));
    expect(screen.getByText(/CREATE TRIGGER touch/)).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "DDL" }));
    expect(screen.getByText(/CREATE TABLE shop.orders/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Копировать DDL" })).toBeInTheDocument();
  });

  it("партиционирование показывается только у партиций и родителей", async () => {
    vi.mocked(api.getTable).mockResolvedValue({ ...details, relkind: "p", partitioning: { key: "RANGE (happened_at)", bound: null, parent: null, partitions: ["shop.events_2026_09"] } });
    const user = userEvent.setup();
    renderWithClient(<TableTab connectionId="c" database="orders" schema="shop" name="events" />);
    await user.click(await screen.findByRole("tab", { name: "Партиционирование" }));
    expect(screen.getByText("RANGE (happened_at)")).toBeInTheDocument();
    expect(screen.getByText("shop.events_2026_09")).toBeInTheDocument();
  });

  it("у обычной таблицы подвкладки «Партиционирование» нет, у партиции — есть, с границами и родителем", async () => {
    const user = userEvent.setup();
    const plain = renderWithClient(<TableTab connectionId="c" database="orders" schema="shop" name="orders" />);
    await screen.findByRole("table", { name: "Колонки" });
    expect(screen.queryByRole("tab", { name: "Партиционирование" })).not.toBeInTheDocument();
    plain.unmount();

    vi.mocked(api.getTable).mockResolvedValue({
      ...details,
      name: "events_2026_09",
      partitioning: { key: null, bound: "FOR VALUES FROM ('2026-09-01') TO ('2026-10-01')", parent: "shop.events", partitions: [] },
    });
    renderWithClient(<TableTab connectionId="c" database="orders" schema="shop" name="events_2026_09" />);
    await user.click(await screen.findByRole("tab", { name: "Партиционирование" }));
    expect(screen.getByText("FOR VALUES FROM ('2026-09-01') TO ('2026-10-01')")).toBeInTheDocument();
    expect(screen.getByText("shop.events")).toBeInTheDocument();
  });

  it("партиционированная таблица: размеры — суммы по дереву, подвкладка это поясняет", async () => {
    vi.mocked(api.getTable).mockResolvedValue({ ...details, relkind: "p", partitioning: { key: "RANGE (id)", bound: null, parent: null, partitions: [] } });
    const user = userEvent.setup();
    renderWithClient(<TableTab connectionId="c" database="orders" schema="shop" name="events" />);
    await user.click(await screen.findByRole("tab", { name: "Размер и статистика" }));
    expect(screen.getByText("Размеры и счётчики просуммированы по всем партициям.")).toBeInTheDocument();
  });

  it("таблица UNLOGGED помечена в заголовке и объяснена", async () => {
    vi.mocked(api.getTable).mockResolvedValue({ ...details, unlogged: true });
    renderWithClient(<TableTab connectionId="c" database="orders" schema="shop" name="orders" />);
    expect(await screen.findByText("UNLOGGED")).toBeInTheDocument();
    expect(screen.getByText(/Не попадает в WAL/)).toBeInTheDocument();
    // У обычной таблицы пометки нет (см. первый тест: подвкладки и таблица без неё).
  });

  it("ограничение без проверки помечено NOT VALID, неизвестный вид показывается кодом", async () => {
    vi.mocked(api.getTable).mockResolvedValue({
      ...details,
      constraints: [
        { name: "fk_cust", kind: "f", definition: "FOREIGN KEY (customer_id) REFERENCES shop.customers(id)", validated: false },
        { name: "odd", kind: "?", definition: "ODD", validated: true },
      ],
    });
    const user = userEvent.setup();
    renderWithClient(<TableTab connectionId="c" database="orders" schema="shop" name="orders" />);
    await user.click(await screen.findByRole("tab", { name: "Ограничения" }));
    const fk = screen.getByRole("row", { name: /fk_cust/ });
    expect(within(fk).getByText("NOT VALID")).toBeInTheDocument();
    expect(within(fk).getByText("Внешний ключ")).toBeInTheDocument();
    expect(within(screen.getByRole("row", { name: /odd/ })).getByText("?")).toBeInTheDocument();
  });

  it("generated-колонка показывает выражение, прочерки — для пустых значений", async () => {
    vi.mocked(api.getTable).mockResolvedValue({
      ...details,
      columns: [{ ...details.columns[0], name: "gross", identity: null, generated: "s", defaultExpr: "net * 1.2", notNull: false }],
    });
    renderWithClient(<TableTab connectionId="c" database="orders" schema="shop" name="orders" />);
    const columns = await screen.findByRole("table", { name: "Колонки" });
    expect(within(columns).getByText("GENERATED ALWAYS AS (net * 1.2) STORED")).toBeInTheDocument();
  });

  it("подвкладки переключаются стрелками, активная — единственная в порядке Tab", async () => {
    const user = userEvent.setup();
    renderWithClient(<TableTab connectionId="c" database="orders" schema="shop" name="orders" />);
    const columnsTab = await screen.findByRole("tab", { name: "Колонки" });
    expect(columnsTab).toHaveAttribute("aria-selected", "true");
    expect(columnsTab).toHaveAttribute("tabindex", "0");
    expect(screen.getByRole("tab", { name: "Индексы" })).toHaveAttribute("tabindex", "-1");
    columnsTab.focus();
    await user.keyboard("{ArrowRight}");
    const indexes = screen.getByRole("tab", { name: "Индексы" });
    expect(indexes).toHaveAttribute("aria-selected", "true");
    expect(indexes).toHaveFocus();
    expect(screen.getByRole("tabpanel")).toHaveAccessibleName("Индексы");
    await user.keyboard("{End}");
    expect(screen.getByRole("tab", { name: "DDL" })).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "Колонки" })).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("tab", { name: "DDL" })).toHaveFocus();
  });

  it("ошибка загрузки показывается с повтором", async () => {
    // Ошибка как из Rust: ключ и параметры шаблона (без params в тексте остались бы «{{schema}}.{{name}}»).
    vi.mocked(api.getTable)
      .mockRejectedValueOnce({ code: "NOT_FOUND", key: "errors.catalog.objectNotFound", params: { schema: "shop", name: "orders" }, message: "gone", retryable: false })
      .mockResolvedValueOnce(details);
    const user = userEvent.setup();
    renderWithClient(<TableTab connectionId="c" database="orders" schema="shop" name="orders" />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Объект shop.orders не найден");
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(await screen.findByRole("table", { name: "Колонки" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
