import { screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { SlotsData } from "../../api/types";
import { formatTimestamp } from "../../lib/format";
import { diagReport, slot, subscription } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";
import { SlotsView } from "./SlotsView";

const data = (patch: Partial<SlotsData> = {}): SlotsData & { kind: "slots" } => ({ kind: "slots", slots: [], subscriptions: [], maxSlotWalKeepSizeMb: null, subscriptionErrorsGrowth: null, ...patch });

describe("SlotsView", () => {
  it("слоты с удерживаемым WAL и отметкой Patroni, подписки", () => {
    renderWithClient(<SlotsView connectionId="c" report={diagReport({ kind: "slots", slots: [slot({ name: "patroni2", active: true, patroni: "member" }), slot({ name: "dead", active: false, retainedBytes: 2 * 1024 ** 3, walStatus: "extended" })], subscriptions: [subscription({ name: "lab_sub", pid: 77, applyErrorCount: 0 })], maxSlotWalKeepSizeMb: -1, subscriptionErrorsGrowth: null })} />);
    expect(screen.getByRole("row", { name: /patroni2/ })).toHaveTextContent("участник Patroni");
    expect(screen.getByRole("row", { name: /dead/ })).toHaveTextContent("2,0 GB");
    expect(screen.getByText(/max_slot_wal_keep_size: без ограничения/)).toBeInTheDocument();
    expect(screen.getByRole("table", { name: "Подписки" })).toHaveTextContent("lab_sub");
  });

  it("постоянный слот Patroni; слот без отметки её не получает", () => {
    renderWithClient(<SlotsView connectionId="c" report={diagReport(data({ slots: [slot({ name: "perm", patroni: "permanent" }), slot({ name: "manual", patroni: null })] }))} />);
    expect(screen.getByRole("row", { name: /perm/ })).toHaveTextContent("постоянный слот Patroni");
    expect(screen.getByRole("row", { name: /manual/ })).not.toHaveTextContent("Patroni");
  });

  it("столбца Patroni нет, когда отметок нет ни у одного слота (подключение не Patroni)", () => {
    renderWithClient(<SlotsView connectionId="c" report={diagReport(data({ slots: [slot({ name: "a" })] }))} />);
    expect(screen.queryByRole("columnheader", { name: "Patroni" })).not.toBeInTheDocument();
  });

  it("логический слот: тип, плагин, база, активность и pid, возраст xmin и catalog_xmin", () => {
    renderWithClient(
      <SlotsView
        connectionId="c"
        report={diagReport(data({ slots: [slot({ name: "lab_sub", slotType: "logical", plugin: "pgoutput", database: "lab", active: true, activePid: 4242, xminAge: 1500, catalogXminAge: 2500 })] }))}
      />,
    );
    const row = screen.getByRole("row", { name: /lab_sub/ });
    for (const text of ["logical", "pgoutput", "lab", "активен", "4242", "1 500", "2 500"]) expect(row).toHaveTextContent(new RegExp(text.replace(" ", "\\s")));
  });

  it("неактивный слот PostgreSQL 17+: с какого момента; причина инвалидации; потерянный WAL выделен словом", () => {
    const at = Date.parse("2026-10-24T10:00:00Z");
    renderWithClient(
      <SlotsView
        connectionId="c"
        report={diagReport(data({ slots: [slot({ name: "old", active: false, inactiveSinceMs: at, walStatus: "lost", invalidationReason: "wal_removed", safeWalSize: null })] }))}
      />,
    );
    const row = screen.getByRole("row", { name: /old/ });
    expect(row).toHaveTextContent("неактивен");
    expect(row).toHaveTextContent(formatTimestamp(at));
    expect(row).toHaveTextContent("lost");
    expect(row).toHaveTextContent("инвалидирован: wal_removed");
  });

  it("запас WAL до потери слота, отметки failover, synced (копия с primary), временный", () => {
    renderWithClient(
      <SlotsView
        connectionId="c"
        report={diagReport(data({ slots: [slot({ name: "copy", safeWalSize: 3 * 1024 ** 2, failover: true, synced: true, temporary: true, twoPhase: true, conflicting: true }), slot({ name: "plain", failover: false, synced: false, temporary: false })] }))}
      />,
    );
    const row = screen.getByRole("row", { name: /copy/ });
    for (const text of ["3,0 MB", "failover", "синхронизирован с primary", "временный", "two-phase", "конфликтует"]) expect(row).toHaveTextContent(text);
    const plain = screen.getByRole("row", { name: /plain/ });
    for (const text of ["failover", "синхронизирован", "временный", "two-phase", "конфликтует"]) expect(plain).not.toHaveTextContent(text);
  });

  it("max_slot_wal_keep_size в мегабайтах показывается в байтовых единицах; неизвестная — строки нет", () => {
    const { rerender } = renderWithClient(<SlotsView connectionId="c" report={diagReport(data({ slots: [slot()], maxSlotWalKeepSizeMb: 2048 }))} />);
    expect(screen.getByText("max_slot_wal_keep_size: 2,0 GB")).toBeInTheDocument();
    rerender(<SlotsView connectionId="c" report={diagReport(data({ slots: [slot()], maxSlotWalKeepSizeMb: null }))} />);
    expect(screen.queryByText(/max_slot_wal_keep_size/)).not.toBeInTheDocument();
  });

  it("подписки: процесс применения, LSN, давность сообщения, счётчики ошибок и их рост", () => {
    renderWithClient(
      <SlotsView
        connectionId="c"
        report={diagReport(
          data({
            subscriptions: [
              subscription({ name: "ok_sub", pid: 77, receivedLsn: "0/3000060", lastMsgAgeS: 5, applyErrorCount: 0, syncErrorCount: 0 }),
              subscription({ name: "dead_sub", pid: null, applyErrorCount: 4, syncErrorCount: 1 }),
            ],
            subscriptionErrorsGrowth: 5,
          }),
        )}
      />,
    );
    const table = screen.getByRole("table", { name: "Подписки" });
    const ok = within(table).getByRole("row", { name: /ok_sub/ });
    for (const text of ["77", "0/3000060", "5 с"]) expect(ok).toHaveTextContent(text);
    const dead = within(table).getByRole("row", { name: /dead_sub/ });
    expect(dead).toHaveTextContent("не работает");
    expect(dead).toHaveTextContent("4");
    expect(screen.getByText("Ошибок с прошлого обновления: +5")).toBeInTheDocument();
  });

  it("без подписок таблицы подписок нет; без слотов — сообщение", () => {
    renderWithClient(<SlotsView connectionId="c" report={diagReport(data())} />);
    expect(screen.queryByRole("table", { name: "Подписки" })).not.toBeInTheDocument();
    expect(screen.getByText("Слотов репликации нет")).toBeInTheDocument();
  });

  it("чужой вид отчёта не рисуется", () => {
    const { container } = renderWithClient(<SlotsView connectionId="c" report={diagReport({ kind: "health", endpoints: [], members: [] })} />);
    expect(container).toBeEmptyDOMElement();
  });
});
