import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppError, Candidate, ConnectionProfile, PatroniAction, PatroniActionPreview, PatroniActionResult, WatchEvent } from "../../api/types";
import { formatScheduled } from "../../lib/format";
import { toRfc3339 } from "../../lib/schedule";
import { renderWithClient } from "../../test/render";
import { newProfile } from "../connections/profile";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { previewPatroniAction: vi.fn(), executePatroniAction: vi.fn(), stopSession: vi.fn() } };
});

import { api } from "../../api/client";
import { PatroniActionDialog } from "./PatroniActionDialog";

const dev: ConnectionProfile = { ...newProfile(), id: "c", name: "billing-dev", environment: "dev", kind: "patroni" };
const prod: ConnectionProfile = { ...dev, name: "billing-prod", environment: "prod" };
const appError = (patch: Partial<AppError>): AppError => ({ code: "INVALID_INPUT", message: "x", retryable: false, ...patch });

const ready = (member: string, patch: Partial<Candidate> = {}): Candidate => ({ member, ready: true, reasons: [], lagBytes: 0, pendingRestart: false, ...patch });
const notReady = (member: string): Candidate => ({ member, ready: false, reasons: ["lagTooHigh"], lagBytes: 5 * 1024 ** 2, pendingRestart: false });
const preview = (patch: Partial<PatroniActionPreview> = {}): PatroniActionPreview => ({
  leader: "pg-1",
  leaderState: "running",
  leaderReachable: true,
  paused: false,
  candidates: [ready("pg-2"), notReady("pg-3")],
  leaderClientSessions: 12,
  member: null,
  scheduledSwitchover: null,
  canSend: true,
  ...patch,
});
const result = (patch: Partial<PatroniActionResult> = {}): PatroniActionResult => ({
  response: "Successfully switched over to pg-2",
  watch: null,
  journalWritten: true,
  outcomeUnknown: false,
  error: null,
  ...patch,
});
const ev = (step: WatchEvent["step"], status: WatchEvent["status"] = "running"): WatchEvent => ({ step, status, atMs: 1, detail: null });

const switchover: PatroniAction = { kind: "switchover", candidate: null, scheduledAt: null };
const failover: PatroniAction = { kind: "failover", candidate: "" };
const restart = (member: string, onlyPending = false): PatroniAction => ({ kind: "restart", member, onlyPending, scheduledAt: null });
const noConfirm = { confirm: null, typedName: null };

const dialog = (action: PatroniAction, profile = dev, onClose: () => void = () => {}) => (
  <PatroniActionDialog connectionId="c" profile={profile} action={action} onClose={onClose} />
);

describe("PatroniActionDialog", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.previewPatroniAction).mockResolvedValue(preview());
    vi.mocked(api.executePatroniAction).mockResolvedValue(result());
    vi.mocked(api.stopSession).mockResolvedValue(true);
  });

  describe("switchover", () => {
    it("предпросмотр: Patroni выбирает сам по умолчанию, неготовый кандидат недоступен с причиной, последствия названы", async () => {
      const user = userEvent.setup();
      renderWithClient(dialog(switchover));
      const run = await screen.findByRole("button", { name: "Выполнить" });
      expect(api.previewPatroniAction).toHaveBeenCalledWith("c", switchover);
      expect(screen.getByRole("radio", { name: "Patroni выберет сам" })).toBeChecked();
      expect(screen.getByRole("radio", { name: /pg-2/ })).toBeEnabled();
      const bad = screen.getByRole("radio", { name: /pg-3/ });
      expect(bad).toBeDisabled();
      expect(bad).toHaveAccessibleName(/лаг больше maximum_lag_on_failover/);
      expect(screen.getByText("Лидер: pg-1")).toHaveTextContent("Лидер: pg-1 (running)");
      expect(screen.getByText("Оборвётся клиентских сессий лидера: 12")).toBeInTheDocument();
      expect(screen.getByText("Запись будет недоступна несколько секунд")).toBeInTheDocument();
      expect(screen.getByText("Старый лидер станет репликой")).toBeInTheDocument();
      expect(run).toHaveClass("primary");
      await user.click(run);
      expect(api.executePatroniAction).toHaveBeenCalledWith("c", { kind: "switchover", candidate: null, scheduledAt: null }, noConfirm, expect.any(Function));
    });

    it("выбранный готовый кандидат уходит в действие", async () => {
      const user = userEvent.setup();
      renderWithClient(dialog(switchover));
      await user.click(await screen.findByRole("radio", { name: /pg-2/ }));
      await user.click(screen.getByRole("button", { name: "Выполнить" }));
      expect(vi.mocked(api.executePatroniAction).mock.calls[0][1]).toEqual({ kind: "switchover", candidate: "pg-2", scheduledAt: null });
    });

    it("«По расписанию»: без времени выполнять нельзя и сказано почему; со временем оно уходит в scheduledAt (RFC 3339)", async () => {
      const user = userEvent.setup();
      renderWithClient(dialog(switchover));
      const run = await screen.findByRole("button", { name: "Выполнить" });
      expect(screen.queryByLabelText("Дата и время")).toBeNull();
      await user.click(screen.getByRole("radio", { name: "По расписанию" }));
      expect(run).toBeDisabled();
      expect(screen.getByText("Укажите дату и время, чтобы запланировать действие")).toBeInTheDocument();
      fireEvent.change(screen.getByLabelText("Дата и время"), { target: { value: "2030-12-01T10:30" } });
      expect(run).toBeEnabled();
      expect(screen.queryByText("Укажите дату и время, чтобы запланировать действие")).toBeNull();
      await user.click(run);
      expect(vi.mocked(api.executePatroniAction).mock.calls[0][1]).toEqual({ kind: "switchover", candidate: null, scheduledAt: toRfc3339("2030-12-01T10:30") });
    });

    it("время стёрли после ввода: кнопка снова неактивна, исключения нет; «Сейчас» отправляет без расписания", async () => {
      const user = userEvent.setup();
      renderWithClient(dialog(switchover));
      const run = await screen.findByRole("button", { name: "Выполнить" });
      await user.click(screen.getByRole("radio", { name: "По расписанию" }));
      const input = screen.getByLabelText("Дата и время");
      fireEvent.change(input, { target: { value: "2030-12-01T10:30" } });
      expect(run).toBeEnabled();
      fireEvent.change(input, { target: { value: "" } });
      expect(run).toBeDisabled();
      await user.click(screen.getByRole("radio", { name: "Сейчас" }));
      expect(run).toBeEnabled();
      await user.click(run);
      expect(vi.mocked(api.executePatroniAction).mock.calls[0][1]).toMatchObject({ scheduledAt: null });
    });

    it("после ответа с наблюдением показана панель хода действия, события из канала её обновляют", async () => {
      const user = userEvent.setup();
      vi.mocked(api.executePatroniAction).mockImplementation(async (_id, _a, _c, onEvent) => {
        onEvent(ev("accepted"));
        return result({ watch: "w-1" });
      });
      renderWithClient(dialog(switchover));
      await user.click(await screen.findByRole("button", { name: "Выполнить" }));
      const panel = await screen.findByRole("region", { name: "Ход действия" });
      expect(within(panel).getAllByRole("listitem")[0]).toHaveTextContent("✓");
      // Диалог остаётся открытым, а подтверждения в нём уже нет: повторить то же действие нельзя.
      expect(screen.queryByRole("button", { name: "Выполнить" })).toBeNull();
    });

    it("лидер не отвечает — предупреждение; число сессий не прочиталось — так и сказано, а не 0", async () => {
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ leaderReachable: false, leaderState: "stopped", leaderClientSessions: null }));
      renderWithClient(dialog(switchover));
      await screen.findByRole("button", { name: "Выполнить" });
      expect(screen.getByText(/Лидер не отвечает/)).toBeInTheDocument();
      expect(screen.getByText("Лидер: pg-1")).toHaveTextContent("(stopped)");
      expect(screen.getByText("Оборвётся клиентских сессий лидера: не удалось прочитать")).toBeInTheDocument();
      expect(screen.queryByText(/сессий лидера: 0/)).toBeNull();
    });

    it("лидер отвечает — предупреждения нет", async () => {
      renderWithClient(dialog(switchover));
      await screen.findByRole("button", { name: "Выполнить" });
      expect(screen.queryByText(/Лидер не отвечает/)).toBeNull();
    });
  });

  describe("failover", () => {
    it("по умолчанию выбран первый готовый кандидат; кнопка ждёт ввода его имени; действие опасное", async () => {
      const user = userEvent.setup();
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ candidates: [notReady("pg-3"), ready("pg-2"), ready("pg-4")] }));
      renderWithClient(dialog(failover));
      const run = await screen.findByRole("button", { name: "Выполнить" });
      expect(screen.getByRole("radio", { name: /pg-2/ })).toBeChecked();
      expect(screen.queryByRole("radio", { name: "Patroni выберет сам" })).toBeNull();
      expect(screen.getByRole("radio", { name: /pg-3/ })).toBeDisabled();
      expect(run).toHaveClass("danger");
      expect(run).toBeDisabled();
      const field = screen.getByLabelText("Подтверждение: имя кандидата");
      await user.type(field, "pg-");
      expect(run).toBeDisabled();
      await user.type(field, "2");
      expect(run).toBeEnabled();
      await user.click(run);
      expect(api.executePatroniAction).toHaveBeenCalledWith("c", { kind: "failover", candidate: "pg-2" }, { confirm: null, typedName: "pg-2" }, expect.any(Function));
    });

    it("смена кандидата требует ввести имя нового", async () => {
      const user = userEvent.setup();
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ candidates: [ready("pg-2"), ready("pg-4")] }));
      renderWithClient(dialog(failover));
      const run = await screen.findByRole("button", { name: "Выполнить" });
      await user.type(screen.getByLabelText("Подтверждение: имя кандидата"), "pg-2");
      expect(run).toBeEnabled();
      await user.click(screen.getByRole("radio", { name: /pg-4/ }));
      expect(run).toBeDisabled();
    });

    it("нет готового кандидата: причины названы, подтверждения нет, закрыть можно", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ candidates: [notReady("pg-3")] }));
      renderWithClient(dialog(failover, dev, onClose));
      expect(await screen.findByText("Нет реплики, готовой стать лидером")).toBeInTheDocument();
      expect(screen.getByText(/лаг больше maximum_lag_on_failover/)).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Выполнить" })).toBeNull();
      expect(screen.queryByLabelText("Подтверждение: имя кандидата")).toBeNull();
      await user.click(screen.getByRole("button", { name: "Закрыть" }));
      expect(onClose).toHaveBeenCalledTimes(1);
      expect(api.executePatroniAction).not.toHaveBeenCalled();
    });

    it("на prod кроме имени кандидата нужно имя подключения", async () => {
      const user = userEvent.setup();
      renderWithClient(dialog(failover, prod));
      const run = await screen.findByRole("button", { name: "Выполнить" });
      await user.type(screen.getByLabelText("Подтверждение: имя кандидата"), "pg-2");
      expect(run).toBeDisabled();
      await user.type(screen.getByLabelText("Имя подключения"), "billing-prod");
      await user.click(run);
      expect(vi.mocked(api.executePatroniAction).mock.calls[0][2]).toEqual({ confirm: "billing-prod", typedName: "pg-2" });
    });
  });

  describe("canSend", () => {
    it("нет логина API: сказано, выполнения и подтверждения нет, закрыть можно", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ canSend: false }));
      renderWithClient(dialog(switchover, dev, onClose));
      expect(await screen.findByText(/Укажите в подключении логин API Patroni/)).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Выполнить" })).toBeNull();
      expect(screen.queryByRole("group")).toBeNull();
      await user.click(screen.getByRole("button", { name: "Закрыть" }));
      expect(onClose).toHaveBeenCalledTimes(1);
      expect(api.executePatroniAction).not.toHaveBeenCalled();
    });
  });

  describe("restart", () => {
    const member = (patch = {}) => ({ name: "pg-1", role: "primary" as const, state: "running", pendingRestart: true, scheduledRestart: null, ...patch });

    it("лидер: запись недоступна до конца перезапуска; «только если нужен» включён при pending restart; действие опасное", async () => {
      const user = userEvent.setup();
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ member: member(), candidates: [], leaderClientSessions: null }));
      renderWithClient(dialog(restart("pg-1")));
      const run = await screen.findByRole("button", { name: "Выполнить" });
      expect(screen.getByText("Перезапуск лидера — запись недоступна до конца перезапуска")).toBeInTheDocument();
      expect(screen.getByRole("checkbox", { name: "Только если нужен перезапуск" })).toBeChecked();
      expect(screen.getByText("Узел: pg-1")).toBeInTheDocument();
      expect(run).toHaveClass("danger");
      await user.click(run);
      expect(api.executePatroniAction).toHaveBeenCalledWith("c", { kind: "restart", member: "pg-1", onlyPending: true, scheduledAt: null }, noConfirm, expect.any(Function));
    });

    it("флажок можно снять: onlyPending уходит false; без pending restart он выключен", async () => {
      const user = userEvent.setup();
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ member: member({ name: "pg-2", role: "replica", pendingRestart: true }) }));
      renderWithClient(dialog(restart("pg-2", true)));
      const box = await screen.findByRole("checkbox", { name: "Только если нужен перезапуск" });
      expect(box).toBeChecked();
      await user.click(box);
      await user.click(screen.getByRole("button", { name: "Выполнить" }));
      expect(vi.mocked(api.executePatroniAction).mock.calls[0][1]).toEqual({ kind: "restart", member: "pg-2", onlyPending: false, scheduledAt: null });
    });

    it("реплика без pending restart: флажок выключен, предупреждения про лидера нет, кнопка не опасная", async () => {
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ member: member({ name: "pg-2", role: "replica", pendingRestart: false }) }));
      renderWithClient(dialog(restart("pg-2")));
      const run = await screen.findByRole("button", { name: "Выполнить" });
      expect(screen.getByRole("checkbox", { name: "Только если нужен перезапуск" })).not.toBeChecked();
      expect(screen.queryByText(/Перезапуск лидера/)).toBeNull();
      expect(run).toHaveClass("primary");
    });

    it("по расписанию: время уходит в scheduledAt", async () => {
      const user = userEvent.setup();
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ member: member({ name: "pg-2", role: "replica" }) }));
      renderWithClient(dialog(restart("pg-2")));
      await user.click(await screen.findByRole("radio", { name: "По расписанию" }));
      fireEvent.change(screen.getByLabelText("Дата и время"), { target: { value: "2030-12-01T03:00" } });
      await user.click(screen.getByRole("button", { name: "Выполнить" }));
      expect(vi.mocked(api.executePatroniAction).mock.calls[0][1]).toMatchObject({ kind: "restart", member: "pg-2", scheduledAt: toRfc3339("2030-12-01T03:00") });
    });

    it("узла нет в топологии: так и сказано, выполнения нет", async () => {
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ member: null }));
      renderWithClient(dialog(restart("pg-9")));
      expect(await screen.findByText("Узел pg-9 недоступен")).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Выполнить" })).toBeNull();
      expect(screen.getByRole("button", { name: "Закрыть" })).toBeInTheDocument();
    });
  });

  describe("отмены", () => {
    const at = "2030-12-01T10:30:00+03:00";

    it("отмена switchover: что запланировано, и действие без параметров", async () => {
      const user = userEvent.setup();
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ candidates: [], scheduledSwitchover: { at, from: "pg-1", to: "pg-2" } }));
      renderWithClient(dialog({ kind: "cancelSwitchover" }));
      await screen.findByRole("button", { name: "Выполнить" });
      expect(screen.getByText(`Запланирован switchover pg-1 → pg-2 на ${formatScheduled(at)}`)).toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: "Выполнить" }));
      expect(api.executePatroniAction).toHaveBeenCalledWith("c", { kind: "cancelSwitchover" }, noConfirm, expect.any(Function));
      // Ответ без наблюдения: текст Patroni.
      expect(await screen.findByText("Ответ Patroni: Successfully switched over to pg-2")).toBeInTheDocument();
    });

    it("ничего не запланировано: сказано, выполнения нет", async () => {
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ scheduledSwitchover: null }));
      renderWithClient(dialog({ kind: "cancelSwitchover" }));
      expect(await screen.findByText("Ничего не запланировано")).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Выполнить" })).toBeNull();
    });

    it("отмена restart: время и узел, действие с узлом", async () => {
      const user = userEvent.setup();
      vi.mocked(api.previewPatroniAction).mockResolvedValue(
        preview({ candidates: [], member: { name: "pg-2", role: "replica", state: "streaming", pendingRestart: false, scheduledRestart: { at, postgresVersion: null } } }),
      );
      renderWithClient(dialog({ kind: "cancelRestart", member: "pg-2" }));
      await screen.findByRole("button", { name: "Выполнить" });
      expect(screen.getByText("Узел: pg-2")).toBeInTheDocument();
      expect(screen.getByText(`Запланирован рестарт на ${formatScheduled(at)}`)).toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: "Выполнить" }));
      expect(vi.mocked(api.executePatroniAction).mock.calls[0][1]).toEqual({ kind: "cancelRestart", member: "pg-2" });
    });

    it("отмена restart без расписания у узла: сказано, выполнения нет", async () => {
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ member: { name: "pg-2", role: "replica", state: "streaming", pendingRestart: false, scheduledRestart: null } }));
      renderWithClient(dialog({ kind: "cancelRestart", member: "pg-2" }));
      expect(await screen.findByText("Ничего не запланировано")).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Выполнить" })).toBeNull();
    });
  });

  describe("пауза", () => {
    it("пауза: Patroni перестанет управлять кластером; действие pause с paused: true; ответ без наблюдения не открывает панель", async () => {
      const user = userEvent.setup();
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ candidates: [], paused: false }));
      renderWithClient(dialog({ kind: "pause", paused: true }));
      await screen.findByRole("button", { name: "Выполнить" });
      expect(screen.getByText("Patroni перестанет управлять кластером: ни failover, ни перезапусков, пока пауза не снята")).toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: "Выполнить" }));
      expect(api.executePatroniAction).toHaveBeenCalledWith("c", { kind: "pause", paused: true }, noConfirm, expect.any(Function));
    });

    it("снятие паузы: действие pause с paused: false и своё пояснение", async () => {
      const user = userEvent.setup();
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ candidates: [], paused: true }));
      renderWithClient(dialog({ kind: "pause", paused: false }));
      await screen.findByRole("button", { name: "Выполнить" });
      expect(screen.getByText("Patroni снова начнёт управлять кластером")).toBeInTheDocument();
      expect(screen.queryByText(/ни failover/)).toBeNull();
      await user.click(screen.getByRole("button", { name: "Выполнить" }));
      expect(vi.mocked(api.executePatroniAction).mock.calls[0][1]).toEqual({ kind: "pause", paused: false });
    });

    it("кластер уже в этом состоянии: сказано, выполнения нет", async () => {
      vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ paused: true }));
      renderWithClient(dialog({ kind: "pause", paused: true }));
      expect(await screen.findByText("Кластер уже в этом состоянии")).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Выполнить" })).toBeNull();
    });
  });

  describe("итог выполнения", () => {
    it("ответ Patroni и «Журнал не записан» без наблюдения", async () => {
      const user = userEvent.setup();
      vi.mocked(api.executePatroniAction).mockResolvedValue(result({ response: "Switchover scheduled", journalWritten: false }));
      renderWithClient(dialog(switchover));
      await user.click(await screen.findByRole("button", { name: "Выполнить" }));
      expect(await screen.findByText("Ответ Patroni: Switchover scheduled")).toBeInTheDocument();
      expect(screen.getByText("Журнал не записан")).toBeInTheDocument();
      expect(screen.queryByRole("region", { name: "Ход действия" })).toBeNull();
    });

    it("пустой ответ Patroni: не «Ответ Patroni: » с пустотой", async () => {
      const user = userEvent.setup();
      vi.mocked(api.executePatroniAction).mockResolvedValue(result({ response: "" }));
      renderWithClient(dialog(switchover));
      await user.click(await screen.findByRole("button", { name: "Выполнить" }));
      expect(await screen.findByText("Patroni принял запрос")).toBeInTheDocument();
      expect(screen.queryByText(/Ответ Patroni/)).toBeNull();
      expect(screen.queryByText("Журнал не записан")).toBeNull();
    });

    it("исход неизвестен, наблюдение идёт: предупреждение, причина и панель", async () => {
      const user = userEvent.setup();
      vi.mocked(api.executePatroniAction).mockResolvedValue(
        result({
          response: "",
          watch: "w-9",
          outcomeUnknown: true,
          error: appError({ code: "PATRONI_ERROR", key: "errors.patroni.timeout", params: { url: "http://pg-1:8008", secs: "5" } }),
        }),
      );
      renderWithClient(dialog(switchover));
      await user.click(await screen.findByRole("button", { name: "Выполнить" }));
      expect(await screen.findByText("Patroni не ответил — выполнилось ли действие, покажет наблюдение")).toBeInTheDocument();
      expect(screen.getByText(/API Patroni http:\/\/pg-1:8008 не ответил за 5 с/)).toBeInTheDocument();
      expect(screen.getByRole("region", { name: "Ход действия" })).toBeInTheDocument();
      expect(screen.queryByText(/Ответ Patroni/)).toBeNull();
    });

    it("исход неизвестен, наблюдения нет: «проверьте обзор»", async () => {
      const user = userEvent.setup();
      vi.mocked(api.executePatroniAction).mockResolvedValue(
        result({ response: "", watch: null, outcomeUnknown: true, error: appError({ code: "PATRONI_ERROR", key: "errors.patroni.connectionLost", params: { url: "http://pg-1:8008" } }) }),
      );
      renderWithClient(dialog(switchover));
      await user.click(await screen.findByRole("button", { name: "Выполнить" }));
      expect(await screen.findByText("Patroni не ответил — выполнилось ли действие, проверьте обзор")).toBeInTheDocument();
      expect(screen.queryByRole("region", { name: "Ход действия" })).toBeNull();
      expect(screen.getByRole("button", { name: "Закрыть" })).toBeInTheDocument();
    });

    it("после выполнения устаревают топология и журнал подключения; событие done — ещё раз топология", async () => {
      const user = userEvent.setup();
      let emit: (e: WatchEvent) => void = () => {};
      vi.mocked(api.executePatroniAction).mockImplementation(async (_id, _a, _c, onEvent) => {
        emit = onEvent;
        return result({ watch: "w-1" });
      });
      const { client } = renderWithClient(dialog(switchover));
      const spy = vi.spyOn(client, "invalidateQueries");
      await user.click(await screen.findByRole("button", { name: "Выполнить" }));
      await screen.findByRole("region", { name: "Ход действия" });
      await waitFor(() => expect(spy).toHaveBeenCalledWith({ queryKey: ["actions", "c"] }));
      expect(spy).toHaveBeenCalledWith({ queryKey: ["topology", "c"] });
      const topologyCalls = () => spy.mock.calls.filter(([f]) => JSON.stringify(f?.queryKey) === JSON.stringify(["topology", "c"])).length;
      const before = topologyCalls();
      // Промежуточное событие топологию не трогает, конечные — перечитывают.
      act(() => emit(ev("leaderStepped")));
      expect(topologyCalls()).toBe(before);
      act(() => emit(ev("done", "done")));
      await waitFor(() => expect(topologyCalls()).toBe(before + 1));
      act(() => emit(ev("done", "timedOut")));
      await waitFor(() => expect(topologyCalls()).toBe(before + 2));
    });
  });

  describe("ошибки", () => {
    it("выполнение отклонено до начала: ошибка с переводом, подтверждение остаётся для повтора", async () => {
      const user = userEvent.setup();
      vi.mocked(api.executePatroniAction).mockRejectedValue(appError({ key: "errors.actions.noLeader" }));
      renderWithClient(dialog(switchover));
      await user.click(await screen.findByRole("button", { name: "Выполнить" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("Нет работающего лидера, с которого можно переключиться");
      expect(screen.getByRole("button", { name: "Выполнить" })).toBeEnabled();
      await user.click(screen.getByRole("button", { name: "Выполнить" }));
      expect(api.executePatroniAction).toHaveBeenCalledTimes(2);
    });

    it("после отказа Patroni журнал подключения устаревает (запись об отказе уже сделана)", async () => {
      const user = userEvent.setup();
      vi.mocked(api.executePatroniAction).mockRejectedValue(appError({ code: "PATRONI_ERROR", key: "errors.patroni.actionRejected", params: { status: "412" } }));
      const { client } = renderWithClient(dialog(switchover));
      const spy = vi.spyOn(client, "invalidateQueries");
      await user.click(await screen.findByRole("button", { name: "Выполнить" }));
      await screen.findByRole("alert");
      expect(spy).toHaveBeenCalledWith({ queryKey: ["actions", "c"] });
    });

    it("защита Rust: отказ guard на prod показан переведённым текстом", async () => {
      const user = userEvent.setup();
      vi.mocked(api.executePatroniAction).mockRejectedValue(appError({ code: "CONFIRMATION_REQUIRED", key: "errors.guard.prodConfirm", params: { name: "billing-prod" } }));
      renderWithClient(dialog(switchover, prod));
      await user.type(await screen.findByLabelText("Имя подключения"), "billing-prod");
      await user.click(screen.getByRole("button", { name: "Выполнить" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("Чтобы изменить prod-подключение, введите его имя «billing-prod»");
      expect(vi.mocked(api.executePatroniAction).mock.calls[0][2]).toEqual({ confirm: "billing-prod", typedName: null });
    });

    it("предпросмотр не удался: ошибка переведена, подтверждения нет, закрыть можно", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      vi.mocked(api.previewPatroniAction).mockRejectedValue(appError({ code: "NODE_UNAVAILABLE", key: "errors.topology.notConnected" }));
      renderWithClient(dialog(switchover, dev, onClose));
      expect(await screen.findByRole("alert")).toHaveTextContent("Подключение не открыто");
      expect(screen.queryByRole("button", { name: "Выполнить" })).toBeNull();
      await user.click(screen.getByRole("button", { name: "Закрыть" }));
      expect(onClose).toHaveBeenCalledTimes(1);
      expect(api.executePatroniAction).not.toHaveBeenCalled();
    });
  });

  it("повторное открытие читает предпросмотр заново, а не берёт кэш прошлого диалога", async () => {
    const user = userEvent.setup();
    function Host() {
      const [open, setOpen] = useState(true);
      return (
        <>
          <button type="button" onClick={() => setOpen((o) => !o)}>
            Переключить
          </button>
          {open && dialog(switchover, dev, () => setOpen(false))}
        </>
      );
    }
    renderWithClient(<Host />);
    await screen.findByText("Оборвётся клиентских сессий лидера: 12");
    await user.click(screen.getByRole("button", { name: "Переключить" }));
    vi.mocked(api.previewPatroniAction).mockResolvedValue(preview({ leaderClientSessions: 40 }));
    await user.click(screen.getByRole("button", { name: "Переключить" }));
    expect(await screen.findByText("Оборвётся клиентских сессий лидера: 40")).toBeInTheDocument();
    expect(api.previewPatroniAction).toHaveBeenCalledTimes(2);
  });

  describe("клавиатура и фокус", () => {
    it("Escape закрывает диалог, пока команда не выполняется", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      renderWithClient(dialog(switchover, dev, onClose));
      await screen.findByRole("button", { name: "Выполнить" });
      await user.keyboard("{Escape}");
      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it("Escape из поля имени закрывает один раз", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      renderWithClient(dialog(failover, dev, onClose));
      await user.click(await screen.findByLabelText("Подтверждение: имя кандидата"));
      await user.keyboard("{Escape}");
      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it("Escape закрывает сообщение об ошибке предпросмотра и экран результата", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      const { unmount } = renderWithClient(dialog(switchover, dev, onClose));
      await user.click(await screen.findByRole("button", { name: "Выполнить" }));
      await screen.findByText(/Ответ Patroni/);
      await user.keyboard("{Escape}");
      expect(onClose).toHaveBeenCalledTimes(1);
      unmount();
      onClose.mockClear();
      vi.mocked(api.previewPatroniAction).mockRejectedValue(appError({ key: "errors.topology.notConnected" }));
      renderWithClient(dialog(switchover, dev, onClose));
      await screen.findByRole("alert");
      await user.keyboard("{Escape}");
      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it("после итога фокус возвращается в диалог (кнопка подтверждения исчезла), чужой фокус не отбирается", async () => {
      const user = userEvent.setup();
      renderWithClient(
        <>
          <button type="button">Снаружи</button>
          {dialog(switchover)}
        </>,
      );
      await user.click(await screen.findByRole("button", { name: "Выполнить" }));
      await screen.findByText(/Ответ Patroni/);
      expect(screen.getByRole("region", { name: "Switchover" })).toHaveFocus();
    });

    it("фокус, который пользователь унёс из диалога, итог не отбирает", async () => {
      const user = userEvent.setup();
      let finish: (r: PatroniActionResult) => void = () => {};
      vi.mocked(api.executePatroniAction).mockReturnValue(new Promise((resolve) => (finish = resolve)));
      renderWithClient(
        <>
          <button type="button">Снаружи</button>
          {dialog(switchover)}
        </>,
      );
      await user.click(await screen.findByRole("button", { name: "Выполнить" }));
      const outside = screen.getByRole("button", { name: "Снаружи" });
      outside.focus();
      await act(async () => finish(result()));
      await screen.findByText(/Ответ Patroni/);
      expect(outside).toHaveFocus();
    });

    it("пока действие выполняется, Escape диалог не закрывает", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      vi.mocked(api.executePatroniAction).mockReturnValue(new Promise(() => {}));
      renderWithClient(dialog(switchover, dev, onClose));
      await user.click(await screen.findByRole("button", { name: "Выполнить" }));
      await waitFor(() => expect(api.executePatroniAction).toHaveBeenCalled());
      const section = screen.getByRole("region", { name: "Switchover" });
      section.focus();
      await user.keyboard("{Escape}");
      expect(onClose).not.toHaveBeenCalled();
      expect(screen.getByRole("button", { name: "Отмена" })).toBeDisabled();
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
            {open && dialog(switchover, dev, () => setOpen(false))}
          </>
        );
      }
      renderWithClient(<Host />);
      const opener = screen.getByRole("button", { name: "Открыть" });
      await user.click(opener);
      const group = await screen.findByRole("group", { name: "Patroni: Switchover" });
      expect(within(screen.getByRole("region", { name: "Switchover" })).getByRole("group", { name: "Patroni: Switchover" })).toBe(group);
      await user.keyboard("{Escape}");
      expect(screen.queryByRole("region", { name: "Switchover" })).toBeNull();
      expect(opener).toHaveFocus();
    });
  });
});
