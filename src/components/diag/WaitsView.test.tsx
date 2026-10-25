import { act, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppError, SamplerEvent } from "../../api/types";
import { diagReport, waitsData } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { startSampler: vi.fn(), stopSession: vi.fn() } };
});

import { api } from "../../api/client";
import { WaitsView } from "./WaitsView";

const authFailed: AppError = { code: "AUTH_FAILED", message: "password authentication failed", retryable: false };

/** Канал подписки: событие, которое сэмплер «присылает» вкладке. */
let emit: (e: SamplerEvent) => void;

describe("WaitsView", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    emit = () => {};
    vi.mocked(api.startSampler).mockImplementation(async (_id, _m, onEvent) => {
      emit = onEvent;
      return "s-1";
    });
    vi.mocked(api.stopSession).mockResolvedValue(true);
  });

  it("топ ожиданий и запросов, «CPU*» с пояснением, сэмплер не работает — подсказка", () => {
    const data = waitsData({ samplerRunning: false, totalSamples: 10,
      topEvents: [{ class: "cpu", event: null, samples: 6, share: 0.6 }, { class: "lock", event: "transactionid", samples: 4, share: 0.4 }],
      topQueries: [{ key: "id:8", queryId: "8", text: "update t", samples: 4, share: 0.4, topEvent: "Lock:transactionid" }],
      byApplication: [{ name: "api", samples: 10, share: 1 }] });
    renderWithClient(<WaitsView connectionId="c" report={diagReport(data)} />);
    const events = screen.getByRole("table", { name: "Топ ожиданий" });
    expect(within(events).getByRole("row", { name: /CPU\*/ })).toHaveTextContent("60 %");
    expect(screen.getByText(/на процессоре или в ожидании без инструментирования/)).toBeInTheDocument();
    expect(within(screen.getByRole("table", { name: "Топ запросов" })).getByRole("row", { name: /update t/ })).toHaveTextContent("Lock:transactionid");
    expect(screen.getByText("Сэмплер не работает: история не пополняется")).toBeInTheDocument();
  });

  it("подписывается на узел отчёта; «живой» индикатор по тику, а устаревшее «не работает» из отчёта при этом не показывается", async () => {
    const report = diagReport(waitsData({ samplerRunning: false }), { member: "pg-2" });
    renderWithClient(<WaitsView connectionId="c" report={report} />);
    await waitFor(() => expect(api.startSampler).toHaveBeenCalledWith("c", "pg-2", expect.any(Function)));
    // Отчёт получен раньше подписки: до первого тика говорить «не работает» верно, после — уже нет.
    expect(screen.getByText("Сэмплер не работает: история не пополняется")).toBeInTheDocument();
    act(() => emit({ kind: "tick", atMs: 1, active: 3, waiting: 1 }));
    expect(screen.getByText("Сэмплер: активных 3, ждут блокировку 1")).toBeInTheDocument();
    expect(screen.queryByText("Сэмплер не работает: история не пополняется")).toBeNull();
  });

  it("при закрытии вида сэмплер отписывается", async () => {
    const { unmount } = renderWithClient(<WaitsView connectionId="c" report={diagReport(waitsData())} />);
    await waitFor(() => expect(api.startSampler).toHaveBeenCalled());
    unmount();
    await waitFor(() => expect(api.stopSession).toHaveBeenCalledWith("s-1"));
  });

  it("ошибка сэмплера — плашка ошибки", async () => {
    renderWithClient(<WaitsView connectionId="c" report={diagReport(waitsData())} />);
    await waitFor(() => expect(api.startSampler).toHaveBeenCalled());
    act(() => emit({ kind: "error", error: authFailed }));
    expect(await screen.findByRole("alert")).toHaveTextContent("password authentication failed");
  });

  it("история короче окна: с какого времени работает сэмплер — по первому интервалу с сэмплами", () => {
    const T = new Date(2026, 9, 25, 12, 0, 0).getTime();
    const data = waitsData({ truncated: true, bucketS: 10, buckets: [
      { startMs: T, sampledS: 0, aas: {} },
      { startMs: T + 10_000, sampledS: 10, aas: { cpu: 1 } },
    ] });
    renderWithClient(<WaitsView connectionId="c" report={diagReport(data)} />);
    expect(screen.getByText(/История короче окна: сэмплер работает с 2026-10-25 12:00:10/)).toBeInTheDocument();
  });

  it("окно полное — строки об усечении нет; сэмплов нет — сказано словами", () => {
    renderWithClient(<WaitsView connectionId="c" report={diagReport(waitsData())} />);
    expect(screen.queryByText(/История короче окна/)).toBeNull();
    expect(screen.getByText("За выбранное окно сэмплов нет")).toBeInTheDocument();
    expect(screen.queryByRole("table", { name: "Топ ожиданий" })).toBeNull();
  });

  it("группы — по приложениям, ролям, базам и клиентам; пустое имя — «(не задано)»; пустые группы не показываются", () => {
    const data = waitsData({ totalSamples: 5,
      topEvents: [{ class: "io", event: "DataFileRead", samples: 5, share: 1 }],
      byApplication: [{ name: "", samples: 3, share: 0.6 }, { name: "api", samples: 2, share: 0.4 }],
      byUser: [{ name: "app", samples: 5, share: 1 }],
      byClient: [{ name: "10.0.0.5", samples: 5, share: 1 }] });
    renderWithClient(<WaitsView connectionId="c" report={diagReport(data)} />);
    const apps = screen.getByRole("table", { name: "По приложениям" });
    expect(within(apps).getAllByRole("row")[1]).toHaveTextContent("(не задано)");
    expect(within(apps).getAllByRole("row")[1]).toHaveTextContent("60 %");
    expect(screen.getByRole("table", { name: "По ролям" })).toBeInTheDocument();
    expect(screen.getByRole("table", { name: "По клиентам" })).toBeInTheDocument();
    expect(screen.queryByRole("table", { name: "По базам данных" })).toBeNull();
  });

  it("запрос: query_id и главное ожидание; без query_id — «—»; длинный текст — целиком в подсказке", () => {
    const long = "select " + "x".repeat(300);
    const data = waitsData({ totalSamples: 5, topQueries: [
      { key: "id:42", queryId: "42", text: "select 1", samples: 3, share: 0.6, topEvent: "IO:DataFileRead" },
      { key: "text:ab", queryId: null, text: long, samples: 2, share: 0.4, topEvent: "CPU*" },
    ] });
    renderWithClient(<WaitsView connectionId="c" report={diagReport(data)} />);
    const rows = within(screen.getByRole("table", { name: "Топ запросов" })).getAllByRole("row");
    expect(rows[1]).toHaveTextContent("42");
    expect(rows[1]).toHaveTextContent("IO:DataFileRead");
    expect(rows[2].querySelector("code")).toHaveAttribute("title", long);
    expect(within(rows[2]).getAllByRole("cell")[1]).toHaveTextContent("—");
  });
});
