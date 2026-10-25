import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppError, ConnectionProfile, PreviewRow, SessionActionPreview } from "../../api/types";
import { renderWithClient } from "../../test/render";
import { newProfile } from "../connections/profile";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { previewSessionAction: vi.fn(), executeSessionAction: vi.fn() } };
});

import { api } from "../../api/client";
import { SessionActionDialog } from "./SessionActionDialog";

const dev: ConnectionProfile = { ...newProfile(), id: "c", name: "billing-dev", environment: "dev" };
const prod: ConnectionProfile = { ...dev, name: "billing-prod", environment: "prod" };
const row = (pid: number, allowed = true, application = "api"): PreviewRow => ({
  session: {
    pid,
    backendStartUs: pid * 10,
    user: "app",
    database: "orders",
    application,
    client: null,
    state: "idle in transaction",
    backendType: "client backend",
    xactAgeS: 600,
    stateAgeS: 600,
    query: "UPDATE t",
  },
  allowed,
  reason: allowed ? null : "superuserSession",
});
const preview: SessionActionPreview = { member: "pg-1", rows: [row(1), row(2), row(3, false)] };
const target = { kind: "idleInTransaction" as const, olderThanS: 300 };
const one = { kind: "selected" as const, sessions: [{ pid: 1, backendStartUs: 10 }] };
const appError = (patch: Partial<AppError>): AppError => ({ code: "INVALID_INPUT", message: "x", retryable: false, ...patch });

describe("SessionActionDialog", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.previewSessionAction).mockResolvedValue(preview);
    vi.mocked(api.executeSessionAction).mockResolvedValue({
      member: "pg-1",
      results: [
        { pid: 1, backendStartUs: 10, result: "terminated" },
        { pid: 2, backendStartUs: 20, result: "noLongerMatches" },
      ],
      outcome: "ok",
      journalWritten: true,
    });
  });

  it("предпросмотр: разрешённые сессии уходят на выполнение, запрещённые — с причиной", async () => {
    const user = userEvent.setup();
    renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={target} onClose={() => {}} />);
    const table = await screen.findByRole("table", { name: "Сессии для завершения" });
    expect(within(table).getAllByRole("row")).toHaveLength(4);
    expect(within(table).getByText("Сессия суперпользователя: нужна роль суперпользователя")).toBeInTheDocument();
    expect(api.previewSessionAction).toHaveBeenCalledWith("c", { member: "pg-1", kind: "terminate", target });
    // Узел назван: на кластере из нескольких узлов важно, где именно выполняется действие.
    expect(screen.getByText("Узел: pg-1")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Завершить 2 сессии" }));
    expect(api.executeSessionAction).toHaveBeenCalledWith(
      "c",
      { member: "pg-1", kind: "terminate", target, sessions: [{ pid: 1, backendStartUs: 10 }, { pid: 2, backendStartUs: 20 }] },
      { confirm: null, typedName: null },
    );
    expect(await screen.findByText("Завершена")).toBeInTheDocument();
    expect(screen.getByText("Уже не подходит под условие")).toBeInTheDocument();
  });

  it("критерий пакета назван: idle in transaction дольше порога", async () => {
    renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={target} onClose={() => {}} />);
    await screen.findByRole("table", { name: "Сессии для завершения" });
    expect(screen.getByText("Сессии в состоянии idle in transaction дольше 5 мин")).toBeInTheDocument();
  });

  it("пакет на prod: имя подключения и число сессий", async () => {
    const user = userEvent.setup();
    renderWithClient(<SessionActionDialog connectionId="c" profile={prod} member="pg-1" kind="terminate" target={target} onClose={() => {}} />);
    const button = await screen.findByRole("button", { name: "Завершить 2 сессии" });
    expect(button).toBeDisabled();
    await user.type(screen.getByLabelText("Имя подключения"), "billing-prod");
    await user.type(screen.getByLabelText("Подтверждение: число сессий"), "2");
    await user.click(button);
    expect(vi.mocked(api.executeSessionAction).mock.calls[0][2]).toEqual({ confirm: "billing-prod", typedName: "2" });
  });

  it("одна сессия на prod: только имя подключения, числа нет", async () => {
    const user = userEvent.setup();
    vi.mocked(api.previewSessionAction).mockResolvedValue({ member: "pg-1", rows: [row(1)] });
    renderWithClient(<SessionActionDialog connectionId="c" profile={prod} member="pg-1" kind="terminate" target={one} onClose={() => {}} />);
    const button = await screen.findByRole("button", { name: "Завершить сессию" });
    expect(screen.queryByLabelText("Подтверждение: число сессий")).toBeNull();
    await user.type(screen.getByLabelText("Имя подключения"), "billing-prod");
    await user.click(button);
    expect(vi.mocked(api.executeSessionAction).mock.calls[0][2]).toEqual({ confirm: "billing-prod", typedName: null });
  });

  it("критерий «idle in transaction» на prod — пакет, даже если подходит одна сессия: число 1", async () => {
    const user = userEvent.setup();
    vi.mocked(api.previewSessionAction).mockResolvedValue({ member: "pg-1", rows: [row(1)] });
    renderWithClient(<SessionActionDialog connectionId="c" profile={prod} member="pg-1" kind="terminate" target={target} onClose={() => {}} />);
    const button = await screen.findByRole("button", { name: "Завершить сессию" });
    await user.type(screen.getByLabelText("Имя подключения"), "billing-prod");
    expect(button).toBeDisabled();
    await user.type(screen.getByLabelText("Подтверждение: число сессий"), "1");
    await user.click(button);
    expect(vi.mocked(api.executeSessionAction).mock.calls[0][2]).toEqual({ confirm: "billing-prod", typedName: "1" });
  });

  it("соединение самой Glafira помечено, но разрешённое остаётся в выполнении", async () => {
    const user = userEvent.setup();
    vi.mocked(api.previewSessionAction).mockResolvedValue({ member: "pg-1", rows: [row(1), row(2, true, "glafira-pg")] });
    renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={target} onClose={() => {}} />);
    const table = await screen.findByRole("table", { name: "Сессии для завершения" });
    const rows = within(table).getAllByRole("row");
    expect(rows[1]).not.toHaveTextContent("Соединение Glafira");
    expect(rows[2]).toHaveTextContent("Соединение Glafira");
    await user.click(screen.getByRole("button", { name: "Завершить 2 сессии" }));
    expect(vi.mocked(api.executeSessionAction).mock.calls[0][1].sessions).toEqual([{ pid: 1, backendStartUs: 10 }, { pid: 2, backendStartUs: 20 }]);
  });

  it("отмена запроса: свои заголовок и кнопка, не опасная; завершение — опасная", async () => {
    const { unmount } = renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="cancel" target={target} onClose={() => {}} />);
    expect(await screen.findByRole("table", { name: "Запросы для отмены" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Отменить запрос?" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Отменить 2 запроса" })).not.toHaveClass("danger");
    unmount();
    renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={target} onClose={() => {}} />);
    expect(await screen.findByRole("button", { name: "Завершить 2 сессии" })).toHaveClass("danger");
  });

  it("журнал не записан — об этом сказано", async () => {
    const user = userEvent.setup();
    // Одна сессия в предпросмотре: кнопка в единственном числе («Отменить запрос»).
    vi.mocked(api.previewSessionAction).mockResolvedValue({ member: "pg-1", rows: [row(1)] });
    vi.mocked(api.executeSessionAction).mockResolvedValue({ member: "pg-1", results: [], outcome: "ok", journalWritten: false });
    renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="cancel" target={one} onClose={() => {}} />);
    await user.click(await screen.findByRole("button", { name: "Отменить запрос" }));
    expect(await screen.findByText("Журнал не записан")).toBeInTheDocument();
  });

  it("ни одна сессия не разрешена: выполнять нечего, закрыть можно", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    vi.mocked(api.previewSessionAction).mockResolvedValue({ member: "pg-1", rows: [row(3, false)] });
    renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={one} onClose={onClose} />);
    expect(await screen.findByText("Ни одну сессию нельзя отменить или завершить")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Завершить/ })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Закрыть" }));
    expect(onClose).toHaveBeenCalled();
    expect(api.executeSessionAction).not.toHaveBeenCalled();
  });

  it("предпросмотр не удался: ошибка переведена, закрыть можно", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    vi.mocked(api.previewSessionAction).mockRejectedValue(appError({ code: "NODE_UNAVAILABLE", key: "errors.actions.nodeUnavailable", params: { member: "pg-9" } }));
    renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-9" kind="terminate" target={one} onClose={onClose} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Узел pg-9 недоступен");
    await user.click(screen.getByRole("button", { name: "Закрыть" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("выполнение отклонено до начала: ошибка с переводом, подтверждение остаётся для повтора", async () => {
    const user = userEvent.setup();
    vi.mocked(api.executeSessionAction).mockRejectedValue(appError({ key: "errors.actions.batchTooLarge", params: { max: "500" } }));
    renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={target} onClose={() => {}} />);
    await user.click(await screen.findByRole("button", { name: "Завершить 2 сессии" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Не больше 500 сессий за одно действие");
    expect(screen.getByRole("button", { name: "Завершить 2 сессии" })).toBeEnabled();
  });

  it("защита Rust: отказ guard на prod показан переведённым текстом", async () => {
    const user = userEvent.setup();
    vi.mocked(api.previewSessionAction).mockResolvedValue({ member: "pg-1", rows: [row(1)] });
    vi.mocked(api.executeSessionAction).mockRejectedValue(
      appError({ code: "CONFIRMATION_REQUIRED", key: "errors.guard.prodConfirm", params: { name: "billing-prod" } }),
    );
    renderWithClient(<SessionActionDialog connectionId="c" profile={prod} member="pg-1" kind="terminate" target={one} onClose={() => {}} />);
    const button = await screen.findByRole("button", { name: "Завершить сессию" });
    await user.type(screen.getByLabelText("Имя подключения"), "billing-prod");
    await user.click(button);
    expect(await screen.findByRole("alert")).toHaveTextContent("Чтобы изменить prod-подключение, введите его имя «billing-prod»");
  });

  it("итог сессии «ошибка» показывает переведённую причину; итог — таблицей PID и результата", async () => {
    const user = userEvent.setup();
    vi.mocked(api.executeSessionAction).mockResolvedValue({
      member: "pg-1",
      results: [
        { pid: 1, backendStartUs: 10, result: "signalSent" },
        { pid: 2, backendStartUs: 20, result: "failed", error: appError({ code: "NODE_UNAVAILABLE", key: "errors.actions.nodeUnavailable", params: { member: "pg-1" } }) },
        { pid: 3, backendStartUs: 30, result: "gone" },
        { pid: 4, backendStartUs: 40, result: "notAllowed" },
      ],
      outcome: "partial",
      journalWritten: true,
    });
    renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={target} onClose={() => {}} />);
    await user.click(await screen.findByRole("button", { name: "Завершить 2 сессии" }));
    const results = await screen.findByRole("table", { name: "Результат" });
    const rows = within(results).getAllByRole("row");
    expect(rows).toHaveLength(5);
    expect(rows[1]).toHaveTextContent("1");
    expect(rows[1]).toHaveTextContent("Сигнал отправлен");
    expect(rows[2]).toHaveTextContent("Ошибка");
    expect(rows[2]).toHaveTextContent("Узел pg-1 недоступен");
    expect(rows[3]).toHaveTextContent("Сессии уже нет");
    expect(rows[4]).toHaveTextContent("Нет прав");
    expect(screen.queryByText("Журнал не записан")).toBeNull();
    // Предпросмотр после выполнения не остаётся: повторить действие тем же списком нельзя.
    expect(screen.queryByRole("table", { name: "Сессии для завершения" })).toBeNull();
  });

  it("после выполнения устаревают отчёты диагностики и журнал подключения — и только его", async () => {
    const user = userEvent.setup();
    const { client } = renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={target} onClose={() => {}} />);
    const spy = vi.spyOn(client, "invalidateQueries");
    await user.click(await screen.findByRole("button", { name: "Завершить 2 сессии" }));
    await screen.findByRole("table", { name: "Результат" });
    await waitFor(() => expect(spy).toHaveBeenCalledWith({ queryKey: ["diag", "c"] }));
    expect(spy).toHaveBeenCalledWith({ queryKey: ["actions", "c"] });
  });

  it("закрытие после результата вызывает onClose", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={target} onClose={onClose} />);
    await user.click(await screen.findByRole("button", { name: "Завершить 2 сессии" }));
    await user.click(await screen.findByRole("button", { name: "Закрыть" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("предпросмотр пуст: «Нет подходящих сессий», а не «нельзя отменить или завершить»; таблицы нет, закрыть можно", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    vi.mocked(api.previewSessionAction).mockResolvedValue({ member: "pg-1", rows: [] });
    renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={target} onClose={onClose} />);
    expect(await screen.findByText("Нет подходящих сессий")).toBeInTheDocument();
    expect(screen.queryByText("Ни одну сессию нельзя отменить или завершить")).toBeNull();
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.queryByRole("button", { name: /Завершить/ })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Закрыть" }));
    expect(onClose).toHaveBeenCalled();
    expect(api.executeSessionAction).not.toHaveBeenCalled();
  });

  describe("клавиатура и фокус", () => {
    it("Escape закрывает диалог на не-prod подключении: фокус на секции, вне группы подтверждения", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={target} onClose={onClose} />);
      await screen.findByRole("button", { name: "Завершить 2 сессии" });
      // Фокус — на секции диалога, а не в ConfirmDialog (на dev у него нет поля ввода).
      expect(screen.getByRole("region", { name: "Завершение сессий" })).toHaveFocus();
      await user.keyboard("{Escape}");
      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it("Escape из поля имени на prod закрывает один раз (ConfirmDialog и секция не дублируют вызов)", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      renderWithClient(<SessionActionDialog connectionId="c" profile={prod} member="pg-1" kind="terminate" target={target} onClose={onClose} />);
      await user.click(await screen.findByLabelText("Имя подключения"));
      await user.keyboard("{Escape}");
      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it("Escape закрывает и экран результата, и сообщение об ошибке предпросмотра", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      const { unmount } = renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={target} onClose={onClose} />);
      await user.click(await screen.findByRole("button", { name: "Завершить 2 сессии" }));
      await screen.findByRole("table", { name: "Результат" });
      await user.click(screen.getByRole("button", { name: "Закрыть" }));
      onClose.mockClear();
      await user.keyboard("{Escape}");
      expect(onClose).toHaveBeenCalledTimes(1);
      unmount();
      vi.mocked(api.previewSessionAction).mockRejectedValue(appError({ code: "NODE_UNAVAILABLE", key: "errors.actions.nodeUnavailable", params: { member: "pg-9" } }));
      renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-9" kind="terminate" target={one} onClose={onClose} />);
      await screen.findByRole("alert");
      onClose.mockClear();
      await user.keyboard("{Escape}");
      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it("пока действие выполняется, Escape диалог не закрывает", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      vi.mocked(api.executeSessionAction).mockReturnValue(new Promise(() => {}));
      renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={target} onClose={onClose} />);
      await user.click(await screen.findByRole("button", { name: "Завершить 2 сессии" }));
      await waitFor(() => expect(api.executeSessionAction).toHaveBeenCalled());
      await user.keyboard("{Escape}");
      expect(onClose).not.toHaveBeenCalled();
    });

    it("у секции и группы подтверждения разные имена; после закрытия фокус возвращается на кнопку, открывшую диалог", async () => {
      const user = userEvent.setup();
      function Host() {
        const [open, setOpen] = useState(false);
        return (
          <>
            <button type="button" onClick={() => setOpen(true)}>
              Открыть
            </button>
            {open && <SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={target} onClose={() => setOpen(false)} />}
          </>
        );
      }
      renderWithClient(<Host />);
      const opener = screen.getByRole("button", { name: "Открыть" });
      await user.click(opener);
      await screen.findByRole("group", { name: "Завершить сессии?" });
      expect(screen.getByRole("region", { name: "Завершение сессий" })).toBeInTheDocument();
      // Имя «Завершить сессии?» одно — у группы подтверждения.
      expect(screen.getAllByLabelText("Завершить сессии?")).toHaveLength(1);
      await user.keyboard("{Escape}");
      expect(screen.queryByRole("region", { name: "Завершение сессий" })).toBeNull();
      expect(opener).toHaveFocus();
    });
  });

  it("выполняется узел из предпросмотра, а не из пропса", async () => {
    const user = userEvent.setup();
    vi.mocked(api.previewSessionAction).mockResolvedValue({ member: "pg-2", rows: [row(1)] });
    renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={one} onClose={() => {}} />);
    await user.click(await screen.findByRole("button", { name: "Завершить сессию" }));
    expect(vi.mocked(api.executeSessionAction).mock.calls[0][1].member).toBe("pg-2");
  });
});
