import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Comparison, QueryStat, SnapshotInfo } from "../../api/types";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { snapshotList: vi.fn(), snapshotTake: vi.fn(), snapshotDelete: vi.fn(), snapshotCompare: vi.fn() } };
});

import { api } from "../../api/client";
import { SnapshotsPanel } from "./SnapshotsPanel";

const a: SnapshotInfo = { file: "pg-1/2026-10-29T10-00-00.json", member: "pg-1", takenAtMs: 1_761_732_000_000, rows: 120, bytes: 40_000 };
const b: SnapshotInfo = { file: "pg-1/2026-10-29T11-00-00.json", member: "pg-1", takenAtMs: 1_761_735_600_000, rows: 130, bytes: 42_000 };
const stat = (id: string, mean: number, patch: Partial<QueryStat> = {}): QueryStat => ({
  key: { queryId: id, dbid: 1, userid: 1, toplevel: true }, datname: "perf", rolname: "app", calls: 100, totalExecMs: mean * 100, meanExecMs: mean,
  minExecMs: null, maxExecMs: null, stddevExecMs: null, rows: 0, rowsPerCall: 0, plans: 0, totalPlanMs: 0, sharedHit: 0, sharedRead: 0, sharedDirtied: 0,
  sharedWritten: 0, localHit: 0, localRead: 0, localDirtied: 0, localWritten: 0, tempRead: 0, tempWritten: 0, ioReadMs: 0, ioWriteMs: 0, tempIoMs: null,
  walRecords: 0, walFpi: 0, walBytes: 0, jitFunctions: null, jitMs: null, parallelPlanned: null, parallelLaunched: null, cpuUserMs: null, cpuSysMs: null, diskReadBytes: null, diskWriteBytes: null, sharePct: 0, hitRatio: null,
  text: "select x", textTruncated: false, own: false, ...patch,
});
const comparison: Comparison = {
  new: [stat("5", 1), stat("6", 1, { text: "/* glafira */ select 1", own: true })],
  gone: [],
  newMore: 0,
  goneMore: 0,
  degraded: [{ key: stat("1", 1).key, datname: "perf", rolname: "app", before: stat("1", 10), after: stat("1", 30), ratio: 3 }],
  improved: [{ key: stat("2", 1).key, datname: "perf", rolname: "app", before: stat("2", 25), after: stat("2", 10), ratio: 0.4 }],
  totalsFrom: { calls: 1, totalExecMs: 1, rows: 0 },
  totalsTo: { calls: 2, totalExecMs: 2, rows: 0 },
  reset: false,
};

describe("SnapshotsPanel", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.snapshotList).mockResolvedValue([b, a]);
    vi.mocked(api.snapshotTake).mockResolvedValue({ ...b, file: "pg-1/2026-10-29T12-00-00.json" });
    vi.mocked(api.snapshotDelete).mockResolvedValue(true);
    vi.mocked(api.snapshotCompare).mockResolvedValue(comparison);
  });

  it("список новых первыми, снятие добавляет строку, удаление спрашивает подтверждение", async () => {
    const onDeleted = vi.fn();
    const user = userEvent.setup();
    renderWithClient(<SnapshotsPanel connectionId="c" member="pg-1" onBetween={() => {}} onDeleted={onDeleted} />);
    const rows = await screen.findAllByRole("row");
    expect(rows[1]).toHaveTextContent("130");
    await user.click(screen.getByRole("button", { name: "Сделать снимок" }));
    await waitFor(() => expect(api.snapshotTake).toHaveBeenCalledWith("c", "pg-1"));
    await waitFor(() => expect(api.snapshotList).toHaveBeenCalledTimes(2));
    await user.click(within(rows[1]).getByRole("button", { name: "Удалить" }));
    expect(api.snapshotDelete).not.toHaveBeenCalled();
    // Подтверждение в фокусе: Escape закрывает его сразу.
    expect(screen.getByRole("button", { name: "Удалить снимок" })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("group", { name: "Удаление снимка" })).not.toBeInTheDocument();
    await user.click(within(rows[1]).getByRole("button", { name: "Удалить" }));
    await user.click(screen.getByRole("button", { name: "Удалить снимок" }));
    await waitFor(() => expect(api.snapshotDelete).toHaveBeenCalledWith("c", b.file));
    expect(onDeleted).toHaveBeenCalledWith(b.file);
    await waitFor(() => expect(screen.queryByRole("group", { name: "Удаление снимка" })).not.toBeInTheDocument());
  });

  it("без узла снимок не снимается", async () => {
    renderWithClient(<SnapshotsPanel connectionId="c" member={null} onBetween={() => {}} />);
    await screen.findAllByRole("row");
    expect(screen.getByRole("button", { name: "Сделать снимок" })).toBeDisabled();
  });

  it("ошибка снятия — в баннере ошибки", async () => {
    vi.mocked(api.snapshotTake).mockRejectedValue({ code: "INVALID_INPUT", message: "exists", key: "errors.snapshots.exists", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<SnapshotsPanel connectionId="c" member="pg-1" onBetween={() => {}} />);
    await screen.findAllByRole("row");
    await user.click(screen.getByRole("button", { name: "Сделать снимок" }));
    expect(await screen.findByText(/уже сделан в эту секунду/)).toBeInTheDocument();
  });

  it("два выбранных снимка сравниваются; «Между снимками» передаёт пару старший → младший", async () => {
    const onBetween = vi.fn();
    const user = userEvent.setup();
    renderWithClient(<SnapshotsPanel connectionId="c" member="pg-1" onBetween={onBetween} />);
    const rows = await screen.findAllByRole("row");
    expect(screen.getByRole("button", { name: "Сравнить" })).toBeDisabled();
    await user.click(within(rows[1]).getByRole("checkbox"));
    await user.click(within(rows[2]).getByRole("checkbox"));
    await user.click(screen.getByRole("button", { name: "Сравнить" }));
    await waitFor(() => expect(api.snapshotCompare).toHaveBeenCalledWith("c", a.file, b.file));
    expect(await screen.findByRole("heading", { name: /Деградировали \(1\)/ })).toBeInTheDocument();
    // Направление словами: замедление — «в 3 раза медленнее», ускорение — обратным коэффициентом, а не «×0,4».
    expect(within(screen.getByRole("table", { name: "Деградировали" })).getByText("в 3 раза медленнее")).toBeInTheDocument();
    expect(within(screen.getByRole("table", { name: "Ускорились" })).getByText("в 2,5 раза быстрее")).toBeInTheDocument();
    expect(screen.queryByText(/×/)).toBeNull();
    expect(screen.getByRole("heading", { name: /Новые \(2\)/ })).toBeInTheDocument();
    // Запрос самой Глафиры помечен.
    const fresh = screen.getByRole("table", { name: "Новые" });
    expect(within(fresh).getByText("Глафира")).toBeInTheDocument();
    // Снимки без pg_stat_kcache — колонки CPU нет.
    expect(within(fresh).queryByRole("columnheader", { name: "CPU" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Топ между снимками" }));
    expect(onBetween).toHaveBeenCalledWith(a, b);
  });

  it("новых и исчезнувших сверх предела — числом под списком, заголовок — со всеми", async () => {
    vi.mocked(api.snapshotCompare).mockResolvedValue({ ...comparison, gone: [stat("7", 1)], newMore: 3, goneMore: 21 });
    const user = userEvent.setup();
    renderWithClient(<SnapshotsPanel connectionId="c" member="pg-1" onBetween={vi.fn()} />);
    const rows = await screen.findAllByRole("row");
    await user.click(within(rows[1]).getByRole("checkbox"));
    await user.click(within(rows[2]).getByRole("checkbox"));
    await user.click(screen.getByRole("button", { name: "Сравнить" }));
    expect(await screen.findByRole("heading", { name: "Новые (5)" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Исчезли (22)" })).toBeInTheDocument();
    expect(screen.getByText("… ещё 3 запроса — показаны самые долгие")).toBeInTheDocument();
    expect(screen.getByText("… ещё 21 запрос — показаны самые долгие")).toBeInTheDocument();
    expect(screen.queryAllByText(/показаны самые долгие/)).toHaveLength(2);
  });

  it("снимки с pg_stat_kcache: у новых и исчезнувших — колонка CPU", async () => {
    vi.mocked(api.snapshotCompare).mockResolvedValue({ ...comparison, new: [stat("5", 1, { cpuUserMs: 40, cpuSysMs: 2 }), stat("6", 1)] });
    const user = userEvent.setup();
    renderWithClient(<SnapshotsPanel connectionId="c" member="pg-1" onBetween={vi.fn()} />);
    const rows = await screen.findAllByRole("row");
    await user.click(within(rows[1]).getByRole("checkbox"));
    await user.click(within(rows[2]).getByRole("checkbox"));
    await user.click(screen.getByRole("button", { name: "Сравнить" }));
    const fresh = await screen.findByRole("table", { name: "Новые" });
    expect(within(fresh).getByRole("columnheader", { name: "CPU" })).toBeInTheDocument();
    const cells = within(fresh).getAllByRole("row").slice(1).map((r) => { const c = within(r).getAllByRole("cell"); return c[c.length - 1].textContent; });
    expect(cells[0]).not.toBe("—");
    expect(cells[1]).toBe("—");
  });

  it("нечитаемый снимок нельзя выбрать, но можно удалить; снимки разных узлов не сравниваются", async () => {
    const broken: SnapshotInfo = { file: "pg-1/2026-10-29T09-00-00.json", member: "pg-1", takenAtMs: 1_761_728_400_000, rows: null, bytes: 200_000_000, unreadable: "tooLarge" };
    const other: SnapshotInfo = { file: "pg-2/2026-10-29T10-30-00.json", member: "pg-2", takenAtMs: 1_761_733_800_000, rows: 10, bytes: 1_000 };
    vi.mocked(api.snapshotList).mockResolvedValue([b, other, a, broken]);
    const user = userEvent.setup();
    renderWithClient(<SnapshotsPanel connectionId="c" member="pg-1" onBetween={() => {}} />);
    const rows = await screen.findAllByRole("row");
    expect(rows[4]).toHaveTextContent("Слишком большой");
    expect((rows[4] as HTMLTableRowElement).cells[3]).toHaveTextContent("—");
    expect(within(rows[4]).getByRole("checkbox")).toBeDisabled();
    await user.click(within(rows[1]).getByRole("checkbox"));
    await user.click(within(rows[2]).getByRole("checkbox"));
    expect(screen.getByText(/Снимки с разных узлов/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Сравнить" })).toBeDisabled();
    // Снять pg-2 и выбрать a: оба снимка с pg-1.
    await user.click(within(rows[2]).getByRole("checkbox"));
    await user.click(within(rows[3]).getByRole("checkbox"));
    expect(screen.queryByText(/Снимки с разных узлов/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Сравнить" })).toBeEnabled();
    await user.click(within(rows[4]).getByRole("button", { name: "Удалить" }));
    await user.click(screen.getByRole("button", { name: "Удалить снимок" }));
    await waitFor(() => expect(api.snapshotDelete).toHaveBeenCalledWith("c", broken.file));
  });

  it("отказ сравнения (снимки разных узлов, одна и та же пара) — в баннере ошибки", async () => {
    vi.mocked(api.snapshotCompare)
      .mockRejectedValueOnce({ code: "INVALID_INPUT", message: "m", key: "errors.snapshots.differentMembers", retryable: false })
      .mockRejectedValueOnce({ code: "INVALID_INPUT", message: "m", key: "errors.snapshots.samePair", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<SnapshotsPanel connectionId="c" member="pg-1" onBetween={() => {}} />);
    const rows = await screen.findAllByRole("row");
    await user.click(within(rows[1]).getByRole("checkbox"));
    await user.click(within(rows[2]).getByRole("checkbox"));
    await user.click(screen.getByRole("button", { name: "Сравнить" }));
    expect(await screen.findByText("Снимки разных узлов сравнить нельзя")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Сравнить" }));
    expect(await screen.findByText("Выберите два разных снимка")).toBeInTheDocument();
  });

  it("строк нет (null) — «—» и у читаемого по виду файла", async () => {
    vi.mocked(api.snapshotList).mockResolvedValue([{ ...a, rows: null }]);
    renderWithClient(<SnapshotsPanel connectionId="c" member="pg-1" onBetween={() => {}} />);
    const rows = await screen.findAllByRole("row");
    expect((rows[1] as HTMLTableRowElement).cells[3]).toHaveTextContent("—");
    expect(within(rows[1]).getByRole("checkbox")).toBeEnabled();
  });
});
