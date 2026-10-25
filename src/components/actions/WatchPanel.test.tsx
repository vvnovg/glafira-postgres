import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { WatchEvent } from "../../api/types";

vi.mock("../../api/client", () => ({ api: { stopSession: vi.fn() } }));

import { api } from "../../api/client";
import { WATCH_STEPS, WatchPanel } from "./WatchPanel";

const ev = (step: WatchEvent["step"], status: WatchEvent["status"] = "running", detail: WatchEvent["detail"] = null): WatchEvent => ({ step, status, atMs: 1, detail });

describe("WatchPanel", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.stopSession).mockResolvedValue(true);
  });

  it("шаги switchover: пройденные, текущий, оставшиеся", () => {
    render(<WatchPanel action="switchover" events={[ev("accepted"), ev("newLeaderRunning")]} watchId="w" onClose={() => {}} />);
    expect(WATCH_STEPS("switchover", false)).toEqual(["accepted", "leaderStepped", "newLeaderRunning", "timelineIncreased", "replicasStreaming", "done"]);
    const items = screen.getAllByRole("listitem");
    expect(items).toHaveLength(6);
    expect(items[0]).toHaveTextContent("✓");
    expect(items[2]).toHaveTextContent("Новый лидер работает");
    expect(items[2]).toHaveTextContent("✓");
    expect(items[3]).not.toHaveTextContent("✓");
    // Следующий невыполненный шаг помечен как текущий — для скринридера, который не видит значков.
    expect(items[3]).toHaveAttribute("aria-current", "step");
    expect(items[4]).not.toHaveAttribute("aria-current");
  });

  it("шаги по видам действий — как WatchPlan::steps в Rust", () => {
    expect(WATCH_STEPS("failover", false)).toEqual(WATCH_STEPS("switchover", false));
    expect(WATCH_STEPS("restart", false)).toEqual(["accepted", "restarted", "memberRunning", "done"]);
    expect(WATCH_STEPS("restart", true)).toEqual(["accepted", "restarted", "memberRunning", "pendingCleared", "done"]);
    expect(WATCH_STEPS("pause", false)).toEqual(["accepted", "pauseApplied", "done"]);
  });

  it("restart «только если нужен»: шаг снятия отметки в списке", () => {
    render(<WatchPanel action="restart" onlyPending events={[ev("accepted")]} watchId="w" onClose={() => {}} />);
    expect(screen.getAllByRole("listitem")).toHaveLength(5);
    expect(screen.getByText(/Отметка «нужен перезапуск» снята/)).toBeInTheDocument();
  });

  it("событие done: все шаги пройдены, текущего нет", () => {
    render(<WatchPanel action="pause" events={[ev("accepted"), ev("pauseApplied"), ev("done", "done")]} watchId="w" onClose={() => {}} />);
    for (const item of screen.getAllByRole("listitem")) {
      expect(item).toHaveTextContent("✓");
      expect(item).not.toHaveAttribute("aria-current");
    }
  });

  it("срок вышел — сказано; закрытие останавливает наблюдение", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<WatchPanel action="pause" events={[ev("accepted", "timedOut")]} watchId="w" onClose={onClose} />);
    expect(screen.getByText(/Срок наблюдения вышел/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Закрыть" }));
    expect(api.stopSession).toHaveBeenCalledWith("w");
    expect(onClose).toHaveBeenCalled();
  });

  it("наблюдение не удалось: показана переведённая причина, а не объект", () => {
    render(
      <WatchPanel
        action="switchover"
        events={[ev("accepted", "failed", { code: "NODE_UNAVAILABLE", key: "errors.topology.notConnected", message: "The connection is not open", retryable: false })]}
        watchId="w"
        onClose={() => {}}
      />,
    );
    expect(screen.getByText("Подключение не открыто")).toBeInTheDocument();
    expect(document.body).not.toHaveTextContent("[object Object]");
  });

  it("наблюдение не удалось без причины: общий текст", () => {
    render(<WatchPanel action="switchover" events={[ev("accepted", "failed")]} watchId="w" onClose={() => {}} />);
    expect(screen.getByText("Действие не удалось")).toBeInTheDocument();
  });

  it("пока всё идёт, ни срока, ни ошибки не показано", () => {
    render(<WatchPanel action="switchover" events={[ev("accepted")]} watchId="w" onClose={() => {}} />);
    expect(screen.queryByText(/Срок наблюдения вышел/)).toBeNull();
    expect(screen.queryByText("Действие не удалось")).toBeNull();
  });

  it("событий ещё нет: список без отметок", () => {
    render(<WatchPanel action="pause" events={[]} watchId="w" onClose={() => {}} />);
    for (const item of screen.getAllByRole("listitem")) expect(item).not.toHaveTextContent("✓");
  });

  it("закрытие вкладки (размонтирование) останавливает наблюдение", async () => {
    const { unmount } = render(<WatchPanel action="pause" events={[ev("accepted")]} watchId="w" onClose={() => {}} />);
    expect(api.stopSession).not.toHaveBeenCalled();
    unmount();
    await vi.waitFor(() => expect(api.stopSession).toHaveBeenCalledWith("w"));
  });

  it("«Закрыть», а затем размонтирование останавливают наблюдение один раз", async () => {
    const user = userEvent.setup();
    const { unmount } = render(<WatchPanel action="pause" events={[ev("accepted")]} watchId="w" onClose={() => {}} />);
    await user.click(screen.getByRole("button", { name: "Закрыть" }));
    unmount();
    // Размонтирование останавливает уже после микрозадачи: даём ей выполниться.
    await Promise.resolve();
    expect(api.stopSession).toHaveBeenCalledTimes(1);
  });

  it("двойной монтаж StrictMode наблюдение не останавливает", async () => {
    render(
      <StrictMode>
        <WatchPanel action="pause" events={[ev("accepted")]} watchId="w" onClose={() => {}} />
      </StrictMode>,
    );
    await Promise.resolve();
    await Promise.resolve();
    expect(api.stopSession).not.toHaveBeenCalled();
  });

  it("другое наблюдение той же панели: каждое останавливается по разу", async () => {
    const { rerender, unmount } = render(<WatchPanel action="pause" events={[ev("accepted")]} watchId="w1" onClose={() => {}} />);
    rerender(<WatchPanel action="pause" events={[ev("accepted")]} watchId="w2" onClose={() => {}} />);
    await vi.waitFor(() => expect(api.stopSession).toHaveBeenCalledWith("w1"));
    unmount();
    await vi.waitFor(() => expect(api.stopSession).toHaveBeenCalledWith("w2"));
    expect(api.stopSession).toHaveBeenCalledTimes(2);
  });

  it("сбой остановки наблюдения не ломает закрытие", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    vi.mocked(api.stopSession).mockRejectedValue(new Error("gone"));
    render(<WatchPanel action="pause" events={[ev("accepted")]} watchId="w" onClose={onClose} />);
    await user.click(screen.getByRole("button", { name: "Закрыть" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
