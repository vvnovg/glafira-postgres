import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ConnectionProfile } from "../../api/types";
import { newProfile } from "../connections/profile";
import { ConfirmDialog, needsConfirmation } from "./ConfirmDialog";

const dev: ConnectionProfile = { ...newProfile(), id: "d", name: "dev-1", environment: "dev" };
const prod: ConnectionProfile = { ...newProfile(), id: "p", name: "prod-1", environment: "prod" };

describe("ConfirmDialog", () => {
  it("требует ввести имя объекта", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(
      <ConfirmDialog connection={dev} title="Удалить слот?" objectName="it_dead_slot" objectLabel="имя слота" confirmLabel="Удалить" danger onConfirm={onConfirm} onCancel={() => {}} />,
    );
    const button = screen.getByRole("button", { name: "Удалить" });
    expect(button).toBeDisabled();
    expect(screen.queryByLabelText("Имя подключения")).not.toBeInTheDocument();
    await user.type(screen.getByLabelText("Подтверждение: имя слота"), "it_dead");
    expect(button).toBeDisabled();
    await user.type(screen.getByLabelText("Подтверждение: имя слота"), "_slot");
    await user.click(button);
    expect(onConfirm).toHaveBeenCalledWith({ confirm: null, typedName: "it_dead_slot" });
  });

  it("имя объекта с пробелом на конце подтверждается (trim с обеих сторон)", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(
      <ConfirmDialog connection={dev} title="Удалить слот?" objectName="slot_a " objectLabel="имя слота" confirmLabel="Удалить" danger onConfirm={onConfirm} onCancel={() => {}} />,
    );
    await user.type(screen.getByLabelText("Подтверждение: имя слота"), "slot_a ");
    await user.click(screen.getByRole("button", { name: "Удалить" }));
    expect(onConfirm).toHaveBeenCalledWith({ confirm: null, typedName: "slot_a" });
  });

  it("для prod требует имя подключения", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(<ConfirmDialog connection={prod} title="Завершить сессию?" confirmLabel="Завершить" onConfirm={onConfirm} onCancel={() => {}} />);
    const button = screen.getByRole("button", { name: "Завершить" });
    expect(button).toBeDisabled();
    await user.type(screen.getByLabelText("Имя подключения"), "prod-1");
    await user.click(button);
    expect(onConfirm).toHaveBeenCalledWith({ confirm: "prod-1", typedName: null });
  });

  it("prod + имя объекта: нужны оба поля, в onConfirm — оба значения", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(
      <ConfirmDialog connection={prod} title="Удалить слот?" objectName="it_dead_slot" objectLabel="имя слота" confirmLabel="Удалить" danger onConfirm={onConfirm} onCancel={() => {}} />,
    );
    const button = screen.getByRole("button", { name: "Удалить" });
    await user.type(screen.getByLabelText("Подтверждение: имя слота"), "it_dead_slot");
    expect(button).toBeDisabled();
    await user.type(screen.getByLabelText("Имя подключения"), "prod-");
    expect(button).toBeDisabled();
    await user.type(screen.getByLabelText("Имя подключения"), "1");
    expect(button).toBeEnabled();
    await user.clear(screen.getByLabelText("Подтверждение: имя слота"));
    expect(button).toBeDisabled();
    await user.type(screen.getByLabelText("Подтверждение: имя слота"), "it_dead_slot");
    await user.click(button);
    expect(onConfirm).toHaveBeenCalledWith({ confirm: "prod-1", typedName: "it_dead_slot" });
  });

  it("отмена", async () => {
    const user = userEvent.setup();
    const onCancel = vi.fn();
    render(<ConfirmDialog connection={prod} title="x" confirmLabel="Да" onConfirm={() => {}} onCancel={onCancel} />);
    await user.click(screen.getByRole("button", { name: "Отмена" }));
    expect(onCancel).toHaveBeenCalled();
  });

  it("Escape отменяет, Enter подтверждает, фокус в первом поле", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    render(<ConfirmDialog connection={dev} title="t" objectName="x" confirmLabel="Да" onConfirm={onConfirm} onCancel={onCancel} />);
    const input = screen.getByLabelText("Подтверждение: имя");
    expect(input).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(onConfirm).not.toHaveBeenCalled();
    await user.type(input, "x{Enter}");
    expect(onConfirm).toHaveBeenCalledWith({ confirm: null, typedName: "x" });
    await user.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("при busy поля и отмена заблокированы, Escape и Enter не действуют", () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    const { unmount } = render(<ConfirmDialog connection={dev} title="t" objectName="x" confirmLabel="Да" busy onConfirm={() => {}} onCancel={() => {}} />);
    expect(screen.getByLabelText("Подтверждение: имя")).toBeDisabled();
    unmount();
    render(<ConfirmDialog connection={dev} title="t" confirmLabel="Да" busy onConfirm={onConfirm} onCancel={onCancel} />);
    expect(screen.getByRole("button", { name: "Отмена" })).toBeDisabled();
    const dialog = screen.getByRole("group", { name: "t" });
    fireEvent.keyDown(dialog, { key: "Escape" });
    fireEvent.keyDown(dialog, { key: "Enter" });
    expect(onCancel).not.toHaveBeenCalled();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("needsConfirmation", () => {
    expect(needsConfirmation(dev)).toBe(false);
    expect(needsConfirmation(dev, "it_dead_slot")).toBe(true);
    expect(needsConfirmation(prod)).toBe(true);
  });
});
