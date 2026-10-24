import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { DdlQueueItem, LockNode, LockSession, LocksData, WaitingLock } from "../../api/types";
import { diagReport } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";
import { LocksView } from "./LocksView";

const s = (pid: number, query: string): LockSession => ({ pid, backendStartUs: pid, user: "app", application: "api", database: "orders", state: "active", query, xactAgeS: 100, queryAgeS: 10 });
const lock = (mode: string, waitS: number | null, approx = false, patch: Partial<WaitingLock> = {}): WaitingLock => ({
  locktype: "relation",
  mode,
  relation: "shop.orders",
  relationOid: 1,
  relationDatabase: "orders",
  waitS,
  approx,
  ...patch,
});
const node = (session: LockSession, patch: Partial<LockNode> = {}): LockNode => ({ session, lock: null, alsoBlockedBy: [], victims: 0, children: [], ...patch });
const data = (patch: Partial<LocksData> = {}): LocksData & { kind: "locks" } => ({ kind: "locks", roots: [], cycles: [], ddlQueue: [], ...patch });
const ddl = (patch: Partial<DdlQueueItem> = {}): DdlQueueItem => ({
  session: s(2, "alter table shop.orders add column x int"),
  waitsFor: 1,
  waitS: 40,
  queued: 1,
  relation: "shop.orders",
  relationOid: 1,
  relationDatabase: "orders",
  mode: "AccessExclusiveLock",
  ...patch,
});

describe("LocksView", () => {
  it("DDL в очереди карточкой сверху, дерево от корня с числом жертв, ожидание ≈ на PG 13", () => {
    const s = (pid: number, query: string) => ({ pid, backendStartUs: pid, user: "app", application: "api", database: "orders", state: "active", query, xactAgeS: 100, queryAgeS: 10 });
    const lock = (mode: string, waitS: number, approx = false) => ({ locktype: "relation", mode, relation: "shop.orders", relationOid: 1, relationDatabase: "orders", waitS, approx });
    const data = {
      kind: "locks" as const,
      roots: [{ session: s(1, "update shop.orders"), lock: null, alsoBlockedBy: [], victims: 2, children: [
        { session: s(2, "alter table shop.orders add column x int"), lock: lock("AccessExclusiveLock", 40), alsoBlockedBy: [], victims: 1, children: [
          { session: s(3, "select count(*) from shop.orders"), lock: lock("AccessShareLock", 30, true), alsoBlockedBy: [], victims: 0, children: [] },
        ] },
      ] }],
      cycles: [],
      ddlQueue: [{ session: s(2, "alter table shop.orders add column x int"), waitsFor: 1, waitS: 40, queued: 1, relation: "shop.orders", relationOid: 1, relationDatabase: "orders", mode: "AccessExclusiveLock" }],
    };
    renderWithClient(<LocksView report={diagReport(data)} />);
    const card = screen.getByRole("region", { name: "DDL в очереди" });
    expect(card).toHaveTextContent("alter table shop.orders");
    expect(card).toHaveTextContent("ждёт сессию 1");
    const tree = screen.getByRole("tree", { name: "Дерево блокировок" });
    expect(within(tree).getByRole("treeitem", { name: /сессия 1/i })).toHaveTextContent("жертв: 2");
    expect(within(tree).getByText("≈ 30 с")).toBeInTheDocument();
  });

  it("карточка DDL: режим, отношение, время ожидания и сколько сессий за ней; без очереди карточек нет", () => {
    const { unmount } = renderWithClient(<LocksView report={diagReport(data({ roots: [node(s(1, "update"), { victims: 1, children: [node(s(2, "x"), { lock: lock("AccessShareLock", 5) })] })], ddlQueue: [ddl({ queued: 3 })] }))} />);
    const card = screen.getByRole("region", { name: "DDL в очереди" });
    expect(card).toHaveTextContent("AccessExclusiveLock на shop.orders");
    expect(card).toHaveTextContent("ждёт сессию 1 (40 с)");
    expect(card).toHaveTextContent("за ней сессий: 3");
    expect(card).toHaveTextContent("Сессия 2");
    unmount();
    renderWithClient(<LocksView report={diagReport(data({ roots: [node(s(1, "update"))] }))} />);
    expect(screen.queryByRole("region", { name: "DDL в очереди" })).not.toBeInTheDocument();
  });

  it("DDL ждёт подготовленную транзакцию или завершившуюся сессию: waitsFor пуст", () => {
    renderWithClient(<LocksView report={diagReport(data({ roots: [node(s(2, "alter"), { lock: lock("AccessExclusiveLock", 12), alsoBlockedBy: [0] })], ddlQueue: [ddl({ waitsFor: null, waitS: 12 })] }))} />);
    const card = screen.getByRole("region", { name: "DDL в очереди" });
    expect(card).toHaveTextContent("ждёт prepared transaction или завершившуюся сессию (12 с)");
    expect(card).not.toHaveTextContent("сессию 0");
  });

  it("время ожидания DDL неизвестно: фраза без скобок, в обоих вариантах", () => {
    const { unmount } = renderWithClient(<LocksView report={diagReport(data({ ddlQueue: [ddl({ waitS: null })] }))} />);
    const timeless = screen.getByRole("region", { name: "DDL в очереди" });
    expect(timeless).toHaveTextContent("ждёт сессию 1");
    expect(timeless).not.toHaveTextContent("(");
    unmount();
    renderWithClient(<LocksView report={diagReport(data({ ddlQueue: [ddl({ waitsFor: null, waitS: null })] }))} />);
    const card = screen.getByRole("region", { name: "DDL в очереди" });
    expect(card).toHaveTextContent("ждёт prepared transaction или завершившуюся сессию");
    expect(card).not.toHaveTextContent("(");
  });

  it("отношение другой БД — «oid N в БД X» в карточке и в узле; без имени БД — «oid N»", () => {
    const other = { relation: null, relationOid: 16450, relationDatabase: "billing" };
    renderWithClient(
      <LocksView
        report={diagReport(
          data({
            roots: [node(s(1, "update"), { victims: 2, children: [node(s(2, "alter"), { lock: lock("AccessExclusiveLock", 9, false, other) }), node(s(3, "select"), { lock: lock("AccessShareLock", 3, false, { relation: null, relationOid: 77, relationDatabase: null }) })] })],
            ddlQueue: [ddl({ ...other })],
          }),
        )}
      />,
    );
    expect(screen.getByRole("region", { name: "DDL в очереди" })).toHaveTextContent("AccessExclusiveLock на oid 16450 в БД billing");
    const tree = screen.getByRole("tree", { name: "Дерево блокировок" });
    expect(within(tree).getByRole("treeitem", { name: /сессия 2/i })).toHaveTextContent("AccessExclusiveLock на oid 16450 в БД billing");
    expect(within(tree).getByRole("treeitem", { name: /сессия 3/i })).toHaveTextContent("AccessShareLock на oid 77");
  });

  it("блокировка не отношения: режим и тип блокировки", () => {
    renderWithClient(<LocksView report={diagReport(data({ roots: [node(s(1, "update"), { victims: 1, children: [node(s(2, "update"), { lock: lock("ShareLock", 4, false, { locktype: "transactionid", relation: null, relationOid: null, relationDatabase: null }) })] })] }))} />);
    expect(screen.getByRole("treeitem", { name: /сессия 2/i })).toHaveTextContent("ShareLock (transactionid)");
  });

  it("ещё ждёт: pid остальных блокирующих, 0 — отдельной подписью, а не «сессия 0»; ждущий корень без жертв", () => {
    renderWithClient(<LocksView report={diagReport(data({ roots: [node(s(5, "update"), { lock: lock("ShareLock", 7), alsoBlockedBy: [0, 9], victims: 0 })] }))} />);
    const item = screen.getByRole("treeitem", { name: /сессия 5/i });
    expect(item).toHaveTextContent("ещё ждёт: prepared transaction или завершившаяся сессия, 9");
    expect(item).not.toHaveTextContent("сессия 0");
    expect(item).not.toHaveTextContent("жертв");
    expect(item).toHaveTextContent("7 с");
  });

  it("корни и все ветки раскрыты; кнопка сворачивает поддерево, aria-expanded только у узлов с потомками", async () => {
    const user = userEvent.setup();
    const tree = node(s(1, "update"), { victims: 2, children: [node(s(2, "select a"), { lock: lock("AccessShareLock", 3), victims: 1, children: [node(s(3, "select b"), { lock: lock("AccessShareLock", 2) })] })] });
    renderWithClient(<LocksView report={diagReport(data({ roots: [tree] }))} />);
    const item = (pid: number) => screen.getByRole("treeitem", { name: new RegExp(`сессия ${pid}$`, "i") });
    expect(item(1)).toHaveAttribute("aria-expanded", "true");
    expect(item(2)).toHaveAttribute("aria-expanded", "true");
    expect(item(3)).not.toHaveAttribute("aria-expanded");
    await user.click(within(item(2)).getByRole("button", { name: "Свернуть поддерево сессии 2" }));
    expect(item(2)).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("treeitem", { name: /сессия 3$/i })).not.toBeInTheDocument();
    await user.click(within(item(2)).getByRole("button", { name: "Развернуть поддерево сессии 2" }));
    expect(item(3)).toBeInTheDocument();
  });

  it("запрос в дереве — одной строкой, обрезан; ожидание без оценки — точное значение", () => {
    const q = `select\n  ${"y".repeat(300)}`;
    renderWithClient(<LocksView report={diagReport(data({ roots: [node(s(1, "update"), { victims: 1, children: [node(s(2, q), { lock: lock("AccessShareLock", 42) })] })] }))} />);
    const item = screen.getByRole("treeitem", { name: /сессия 2/i });
    expect(within(item).getByText("42 с")).toBeInTheDocument();
    expect(within(item).queryByText(/≈/)).not.toBeInTheDocument();
    const code = within(item).getByText(/^select y+…$/);
    expect(code.textContent!.length).toBeLessThanOrEqual(121);
  });

  it("пусто: «Никто не ждёт блокировок»; цикл ожидания — группой pid; подпись про лог сервера есть всегда", () => {
    const logNote = "Подробности взаимных блокировок есть только в логе сервера";
    const { unmount } = renderWithClient(<LocksView report={diagReport(data())} />);
    expect(screen.getByText("Никто не ждёт блокировок")).toBeInTheDocument();
    expect(screen.queryByRole("tree")).not.toBeInTheDocument();
    expect(screen.queryByText("Сессии ждут друг друга", { exact: false })).not.toBeInTheDocument();
    expect(screen.getByText(logNote)).toBeInTheDocument();
    unmount();
    const withTree = renderWithClient(<LocksView report={diagReport(data({ roots: [node(s(1, "update"))] }))} />);
    expect(screen.getByText(logNote)).toBeInTheDocument();
    withTree.unmount();
    renderWithClient(<LocksView report={diagReport(data({ cycles: [[4, 8]] }))} />);
    expect(screen.queryByText("Никто не ждёт блокировок")).not.toBeInTheDocument();
    expect(screen.getByText("Сессии ждут друг друга: 4, 8")).toBeInTheDocument();
    expect(screen.getByText(logNote)).toBeInTheDocument();
  });
});
