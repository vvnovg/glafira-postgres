import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppError, ConnectionProfile, SlotDropPreview } from "../../api/types";
import { formatTimestamp } from "../../lib/format";
import { slot } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";
import { newProfile } from "../connections/profile";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { previewDropSlot: vi.fn(), dropSlot: vi.fn() } };
});

import { api } from "../../api/client";
import { DropSlotDialog } from "./DropSlotDialog";

const dev: ConnectionProfile = { ...newProfile(), id: "c", name: "billing-dev", environment: "dev" };
const prod: ConnectionProfile = { ...dev, name: "billing-prod", environment: "prod" };
const dead = slot({ name: "it_dead_slot", slotType: "physical", active: false, walStatus: "extended", retainedBytes: 2 * 1024 ** 3 });
const preview = (patch: Partial<typeof dead> = {}): SlotDropPreview => ({ member: "pg-1", slot: { ...dead, ...patch } });
const appError = (patch: Partial<AppError>): AppError => ({ code: "INVALID_INPUT", message: "x", retryable: false, ...patch });

/** Диалог для слота `it_dead_slot` на узле pg-1. */
const dialog = (profile = dev, onClose: () => void = () => {}, member = "pg-1") => (
  <DropSlotDialog connectionId="c" profile={profile} member={member} slot="it_dead_slot" scope="tab" onClose={onClose} />
);

describe("DropSlotDialog", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.previewDropSlot).mockResolvedValue(preview());
    vi.mocked(api.dropSlot).mockResolvedValue({ journalWritten: true });
  });

  it("предпросмотр показывает тип, базу, wal_status и удерживаемый WAL; кнопка ждёт ввода имени слота, затем слот удаляется", async () => {
    const user = userEvent.setup();
    vi.mocked(api.previewDropSlot).mockResolvedValue(preview({ database: null, inactiveSinceMs: Date.parse("2026-10-24T10:00:00Z") }));
    renderWithClient(dialog());
    await screen.findByRole("button", { name: "Удалить слот" });
    const facts = document.querySelector("dl.kv") as HTMLElement;
    expect(api.previewDropSlot).toHaveBeenCalledWith("c", { member: "pg-1", slot: "it_dead_slot" });
    const text = facts.textContent ?? "";
    for (const part of ["Тип", "physical", "База данных", "Состояние", "неактивен", "wal_status", "extended", "Удерживает WAL", "2,0 GB", "Неактивен с", formatTimestamp(Date.parse("2026-10-24T10:00:00Z"))]) {
      expect(text).toContain(part);
    }
    // Узел назван: на кластере из нескольких узлов важно, где именно выполняется действие.
    expect(screen.getByText("Узел: pg-1")).toBeInTheDocument();
    const button = screen.getByRole("button", { name: "Удалить слот" });
    expect(button).toBeDisabled();
    expect(button).toHaveClass("danger");
    await user.type(screen.getByLabelText("Подтверждение: имя слота"), "it_dead");
    expect(button).toBeDisabled();
    await user.type(screen.getByLabelText("Подтверждение: имя слота"), "_slot");
    await user.click(button);
    expect(api.dropSlot).toHaveBeenCalledWith("c", { member: "pg-1", slot: "it_dead_slot" }, { confirm: null, typedName: "it_dead_slot" });
    expect(await screen.findByText("Слот удалён")).toBeInTheDocument();
    // После удаления подтверждение не остаётся: повторить то же удаление нельзя.
    expect(screen.queryByRole("button", { name: "Удалить слот" })).toBeNull();
    expect(screen.queryByText("Журнал не записан")).toBeNull();
  });

  it("логический слот: плагин и база; момент неактивности показан только когда он известен", async () => {
    vi.mocked(api.previewDropSlot).mockResolvedValue(preview({ slotType: "logical", plugin: "pgoutput", database: "lab" }));
    renderWithClient(dialog());
    await screen.findByRole("button", { name: "Удалить слот" });
    const facts = document.querySelector("dl.kv") as HTMLElement;
    expect(facts).toHaveTextContent("logical · pgoutput");
    expect(facts).toHaveTextContent("lab");
    expect(facts).not.toHaveTextContent("Неактивен с");
  });

  it("слот стал активным, пока открывался диалог: предупреждение и только «Закрыть», подтверждения и удаления нет", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    vi.mocked(api.previewDropSlot).mockResolvedValue(preview({ active: true, activePid: 4242, patroni: "member" }));
    renderWithClient(dialog(dev, onClose));
    expect(await screen.findByRole("status")).toHaveTextContent("Слот активен — удалить можно только неактивный слот");
    // Факты видны: состояние названо с pid процесса, который слот держит.
    expect(document.querySelector("dl.kv")).toHaveTextContent("активен, pid 4242");
    expect(screen.queryByRole("group")).toBeNull();
    expect(screen.queryByLabelText("Подтверждение: имя слота")).toBeNull();
    expect(screen.queryByRole("button", { name: "Удалить слот" })).toBeNull();
    // Предупреждение Patroni относится к удалению, которого здесь нет.
    expect(screen.queryByText(/Patroni/)).toBeNull();
    await user.click(screen.getByRole("button", { name: "Закрыть" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(api.dropSlot).not.toHaveBeenCalled();
  });

  it("слот участника Patroni: предупреждение, кнопка доступна", async () => {
    vi.mocked(api.previewDropSlot).mockResolvedValue(preview({ patroni: "member" }));
    renderWithClient(dialog());
    expect(await screen.findByText("Patroni пересоздаст слот, пока участник есть в кластере; удалите участника")).toBeInTheDocument();
    expect(screen.queryByText(/patronictl edit-config/)).toBeNull();
    // Предупреждение не запрещает: имя введено — кнопка доступна.
    const user = userEvent.setup();
    await user.type(screen.getByLabelText("Подтверждение: имя слота"), "it_dead_slot");
    expect(screen.getByRole("button", { name: "Удалить слот" })).toBeEnabled();
  });

  it("постоянный слот Patroni: предупреждение про /config, кнопка доступна", async () => {
    vi.mocked(api.previewDropSlot).mockResolvedValue(preview({ patroni: "permanent" }));
    renderWithClient(dialog());
    expect(await screen.findByText("Уберите слот из /config (patronictl edit-config), иначе Patroni его пересоздаст")).toBeInTheDocument();
    expect(screen.queryByText(/Patroni пересоздаст слот/)).toBeNull();
    const user = userEvent.setup();
    await user.type(screen.getByLabelText("Подтверждение: имя слота"), "it_dead_slot");
    expect(screen.getByRole("button", { name: "Удалить слот" })).toBeEnabled();
  });

  it("слот без отметки Patroni: предупреждения нет", async () => {
    renderWithClient(dialog());
    await screen.findByRole("button", { name: "Удалить слот" });
    expect(screen.queryByText(/Patroni/)).toBeNull();
  });

  it("на prod кроме имени слота нужно имя подключения", async () => {
    const user = userEvent.setup();
    renderWithClient(dialog(prod));
    const button = await screen.findByRole("button", { name: "Удалить слот" });
    await user.type(screen.getByLabelText("Подтверждение: имя слота"), "it_dead_slot");
    expect(button).toBeDisabled();
    await user.type(screen.getByLabelText("Имя подключения"), "billing-prod");
    expect(button).toBeEnabled();
    await user.click(button);
    expect(api.dropSlot).toHaveBeenCalledWith("c", { member: "pg-1", slot: "it_dead_slot" }, { confirm: "billing-prod", typedName: "it_dead_slot" });
  });

  it("журнал не записан — об этом сказано", async () => {
    const user = userEvent.setup();
    vi.mocked(api.dropSlot).mockResolvedValue({ journalWritten: false });
    renderWithClient(dialog());
    await user.type(await screen.findByLabelText("Подтверждение: имя слота"), "it_dead_slot");
    await user.click(screen.getByRole("button", { name: "Удалить слот" }));
    expect(await screen.findByText("Слот удалён")).toBeInTheDocument();
    expect(screen.getByText("Журнал не записан")).toBeInTheDocument();
  });

  it("после удаления устаревают отчёты диагностики и журнал подключения — и только его", async () => {
    const user = userEvent.setup();
    const { client } = renderWithClient(dialog());
    const spy = vi.spyOn(client, "invalidateQueries");
    await user.type(await screen.findByLabelText("Подтверждение: имя слота"), "it_dead_slot");
    await user.click(screen.getByRole("button", { name: "Удалить слот" }));
    await screen.findByText("Слот удалён");
    await waitFor(() => expect(spy).toHaveBeenCalledWith({ queryKey: ["diag", "c"] }));
    expect(spy).toHaveBeenCalledWith({ queryKey: ["actions", "c"] });
  });

  it("слот стал активным: ошибка переведена, подтверждение остаётся", async () => {
    const user = userEvent.setup();
    vi.mocked(api.dropSlot).mockRejectedValue(appError({ key: "errors.actions.slotActiveOrGone" }));
    renderWithClient(dialog());
    await user.type(await screen.findByLabelText("Подтверждение: имя слота"), "it_dead_slot");
    await user.click(screen.getByRole("button", { name: "Удалить слот" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Слот стал активным или исчез");
    expect(screen.getByRole("button", { name: "Удалить слот" })).toBeEnabled();
    expect(screen.queryByText("Слот удалён")).toBeNull();
  });

  it("у роли нет REPLICATION: ошибка переведена", async () => {
    const user = userEvent.setup();
    vi.mocked(api.dropSlot).mockRejectedValue(appError({ code: "NOT_AUTHORIZED", key: "errors.actions.replicationRequired" }));
    renderWithClient(dialog());
    await user.type(await screen.findByLabelText("Подтверждение: имя слота"), "it_dead_slot");
    await user.click(screen.getByRole("button", { name: "Удалить слот" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Для удаления слота нужен атрибут REPLICATION или роль суперпользователя");
  });

  it("защита Rust: отказ guard на prod показан переведённым текстом", async () => {
    const user = userEvent.setup();
    vi.mocked(api.dropSlot).mockRejectedValue(appError({ code: "CONFIRMATION_REQUIRED", key: "errors.guard.prodConfirm", params: { name: "billing-prod" } }));
    renderWithClient(dialog(prod));
    await user.type(await screen.findByLabelText("Подтверждение: имя слота"), "it_dead_slot");
    await user.type(screen.getByLabelText("Имя подключения"), "billing-prod");
    await user.click(screen.getByRole("button", { name: "Удалить слот" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Чтобы изменить prod-подключение, введите его имя «billing-prod»");
  });

  it("предпросмотр не удался (слота уже нет): ошибка переведена, подтверждения нет, закрыть можно", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    vi.mocked(api.previewDropSlot).mockRejectedValue(appError({ code: "NOT_FOUND", key: "errors.actions.slotNotFound", params: { slot: "it_dead_slot", member: "pg-1" } }));
    renderWithClient(dialog(dev, onClose));
    expect(await screen.findByRole("alert")).toHaveTextContent("Слота it_dead_slot нет на узле pg-1");
    expect(screen.queryByLabelText("Подтверждение: имя слота")).toBeNull();
    expect(screen.queryByRole("button", { name: "Удалить слот" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Закрыть" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(api.dropSlot).not.toHaveBeenCalled();
  });

  it("«Отмена» и «Закрыть» после результата вызывают onClose", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    renderWithClient(dialog(dev, onClose));
    await user.click(await screen.findByRole("button", { name: "Отмена" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    await user.type(screen.getByLabelText("Подтверждение: имя слота"), "it_dead_slot");
    await user.click(screen.getByRole("button", { name: "Удалить слот" }));
    await user.click(await screen.findByRole("button", { name: "Закрыть" }));
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("выполняется узел из предпросмотра, а не из пропса", async () => {
    const user = userEvent.setup();
    vi.mocked(api.previewDropSlot).mockResolvedValue({ member: "pg-2", slot: dead });
    renderWithClient(dialog(dev, () => {}, "pg-1"));
    await user.type(await screen.findByLabelText("Подтверждение: имя слота"), "it_dead_slot");
    expect(screen.getByText("Узел: pg-2")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Удалить слот" }));
    expect(vi.mocked(api.dropSlot).mock.calls[0][1]).toEqual({ member: "pg-2", slot: "it_dead_slot" });
  });

  it("повторное открытие читает слот заново, а не берёт предпросмотр прошлого диалога", async () => {
    const user = userEvent.setup();
    function Host() {
      const [open, setOpen] = useState(true);
      return (
        <>
          <button type="button" onClick={() => setOpen((o) => !o)}>
            Переключить
          </button>
          {open && dialog(dev, () => setOpen(false))}
        </>
      );
    }
    renderWithClient(<Host />);
    await screen.findByRole("button", { name: "Удалить слот" });
    await user.click(screen.getByRole("button", { name: "Переключить" }));
    expect(screen.queryByRole("button", { name: "Удалить слот" })).toBeNull();
    vi.mocked(api.previewDropSlot).mockResolvedValue(preview({ retainedBytes: 3 * 1024 ** 3 }));
    await user.click(screen.getByRole("button", { name: "Переключить" }));
    expect(await screen.findByText("3,0 GB")).toBeInTheDocument();
    expect(api.previewDropSlot).toHaveBeenCalledTimes(2);
  });

  describe("клавиатура и фокус", () => {
    it("Escape закрывает диалог на не-prod подключении: фокус на секции, вне группы подтверждения", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      renderWithClient(dialog(dev, onClose));
      await screen.findByRole("button", { name: "Удалить слот" });
      await user.keyboard("{Escape}");
      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it("Escape из поля имени закрывает один раз (ConfirmDialog и секция не дублируют вызов)", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      renderWithClient(dialog(prod, onClose));
      await user.click(await screen.findByLabelText("Подтверждение: имя слота"));
      await user.keyboard("{Escape}");
      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it("Escape закрывает и экран результата, и сообщение об ошибке предпросмотра", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      const { unmount } = renderWithClient(dialog(dev, onClose));
      await user.type(await screen.findByLabelText("Подтверждение: имя слота"), "it_dead_slot");
      await user.click(screen.getByRole("button", { name: "Удалить слот" }));
      await user.click(await screen.findByRole("button", { name: "Закрыть" }));
      onClose.mockClear();
      await user.keyboard("{Escape}");
      expect(onClose).toHaveBeenCalledTimes(1);
      unmount();
      vi.mocked(api.previewDropSlot).mockRejectedValue(appError({ code: "NODE_UNAVAILABLE", key: "errors.actions.nodeUnavailable", params: { member: "pg-1" } }));
      renderWithClient(dialog(dev, onClose));
      await screen.findByRole("alert");
      onClose.mockClear();
      await user.keyboard("{Escape}");
      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it("пока слот удаляется, Escape диалог не закрывает", async () => {
      const user = userEvent.setup();
      const onClose = vi.fn();
      vi.mocked(api.dropSlot).mockReturnValue(new Promise(() => {}));
      renderWithClient(dialog(dev, onClose));
      await user.type(await screen.findByLabelText("Подтверждение: имя слота"), "it_dead_slot");
      await user.click(screen.getByRole("button", { name: "Удалить слот" }));
      await waitFor(() => expect(api.dropSlot).toHaveBeenCalled());
      // После щелчка кнопка неактивна и фокус уходит с неё на body, откуда нажатие до секции не доходит. Поэтому фокус
      // ставится на саму секцию: без защиты `!run.isPending` в обработчике секции Escape закрыл бы диалог посреди команды.
      const section = screen.getByRole("region", { name: "Удаление слота" });
      section.focus();
      expect(section).toHaveFocus();
      await user.keyboard("{Escape}");
      expect(onClose).not.toHaveBeenCalled();
      // «Отмена» тоже ждёт конца команды.
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
            {open && dialog(dev, () => setOpen(false))}
          </>
        );
      }
      renderWithClient(<Host />);
      const opener = screen.getByRole("button", { name: "Открыть" });
      await user.click(opener);
      const group = await screen.findByRole("group", { name: "Удалить слот it_dead_slot?" });
      expect(within(screen.getByRole("region", { name: "Удаление слота" })).getByRole("group")).toBe(group);
      // Имя «Удалить слот it_dead_slot?» одно — у группы подтверждения.
      expect(screen.getAllByLabelText("Удалить слот it_dead_slot?")).toHaveLength(1);
      await user.keyboard("{Escape}");
      expect(screen.queryByRole("region", { name: "Удаление слота" })).toBeNull();
      expect(opener).toHaveFocus();
    });
  });
});
