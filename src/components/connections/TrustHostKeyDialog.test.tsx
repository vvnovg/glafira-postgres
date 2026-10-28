import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppError } from "../../api/types";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { trustSshHostKey: vi.fn() } };
});

import { api } from "../../api/client";
import { TrustHostKeyDialog } from "./TrustHostKeyDialog";

const unknown: AppError = {
  code: "SSH_ERROR",
  key: "errors.ssh.hostKeyUnknown",
  message: "unknown host key",
  retryable: false,
  params: { kind: "hostKeyUnknown", target: "bastion", host: "bastion (10.0.0.5)", fingerprint: "SHA256:abc" },
};

beforeEach(() => vi.resetAllMocks());

describe("TrustHostKeyDialog", () => {
  it("показывает хост и отпечаток; фокус — на «Отмена», чтобы случайный Enter ничему не доверял", () => {
    render(<TrustHostKeyDialog error={unknown} onTrusted={vi.fn()} onCancel={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "Неизвестный ключ хоста" });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(dialog).toHaveTextContent("bastion (10.0.0.5)");
    expect(dialog).toHaveTextContent("SHA256:abc");
    expect(screen.getByRole("button", { name: "Отмена" })).toHaveFocus();
  });

  it("подпись кнопки зависит от действия: без пропа — «Доверять и подключиться», для проверки — «Доверять и проверить»", () => {
    const { unmount } = render(<TrustHostKeyDialog error={unknown} onTrusted={vi.fn()} onCancel={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Доверять и подключиться" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Доверять и проверить" })).not.toBeInTheDocument();
    unmount();
    render(<TrustHostKeyDialog error={unknown} action="check" onTrusted={vi.fn()} onCancel={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Доверять и проверить" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Доверять и подключиться" })).not.toBeInTheDocument();
  });

  it("«Доверять и проверить» делает то же, что и «Доверять и подключиться»: доверяет и сообщает родителю", async () => {
    vi.mocked(api.trustSshHostKey).mockResolvedValue(undefined);
    const onTrusted = vi.fn();
    const user = userEvent.setup();
    render(<TrustHostKeyDialog error={unknown} action="check" onTrusted={onTrusted} onCancel={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: "Доверять и проверить" }));
    expect(api.trustSshHostKey).toHaveBeenCalledWith("bastion", "SHA256:abc");
    expect(onTrusted).toHaveBeenCalledTimes(1);
  });

  it("«Доверять и подключиться» передаёт цель и отпечаток, затем сообщает родителю", async () => {
    vi.mocked(api.trustSshHostKey).mockResolvedValue(undefined);
    const onTrusted = vi.fn();
    const user = userEvent.setup();
    render(<TrustHostKeyDialog error={unknown} onTrusted={onTrusted} onCancel={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: "Доверять и подключиться" }));
    expect(api.trustSshHostKey).toHaveBeenCalledWith("bastion", "SHA256:abc");
    expect(onTrusted).toHaveBeenCalledTimes(1);
  });

  it("пока команда выполняется, обе кнопки заблокированы: повторный клик доверие не дублирует", async () => {
    let finish: () => void = () => {};
    vi.mocked(api.trustSshHostKey).mockReturnValue(new Promise<void>((resolve) => (finish = resolve)));
    const user = userEvent.setup();
    render(<TrustHostKeyDialog error={unknown} onTrusted={vi.fn()} onCancel={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: "Доверять и подключиться" }));
    expect(screen.getByRole("button", { name: "Доверять и подключиться" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Отмена" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Доверять и подключиться" }));
    expect(api.trustSshHostKey).toHaveBeenCalledTimes(1);
    finish();
  });

  it("Escape и «Отмена» закрывают диалог без доверия", async () => {
    const onCancel = vi.fn();
    const user = userEvent.setup();
    render(<TrustHostKeyDialog error={unknown} onTrusted={vi.fn()} onCancel={onCancel} />);
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Отмена" }));
    expect(onCancel).toHaveBeenCalledTimes(2);
    expect(api.trustSshHostKey).not.toHaveBeenCalled();
  });

  it("Tab ходит по кругу внутри диалога", async () => {
    const user = userEvent.setup();
    render(<TrustHostKeyDialog error={unknown} onTrusted={vi.fn()} onCancel={vi.fn()} />);
    const cancel = screen.getByRole("button", { name: "Отмена" });
    const trust = screen.getByRole("button", { name: "Доверять и подключиться" });
    expect(cancel).toHaveFocus();
    await user.tab();
    expect(trust).toHaveFocus();
    await user.tab();
    expect(cancel).toHaveFocus();
    await user.tab({ shift: true });
    expect(trust).toHaveFocus();
  });

  it("после отказа команды ошибка в диалоге, а родитель не уведомлён", async () => {
    vi.mocked(api.trustSshHostKey).mockRejectedValue({ code: "INVALID_INPUT", message: "Fingerprint is empty", retryable: false });
    const onTrusted = vi.fn();
    const user = userEvent.setup();
    render(<TrustHostKeyDialog error={unknown} onTrusted={onTrusted} onCancel={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: "Доверять и подключиться" }));
    expect(await screen.findByText("Fingerprint is empty")).toBeInTheDocument();
    expect(onTrusted).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Доверять и подключиться" })).toBeEnabled();
  });

  it("ошибка, которой доверять нельзя (смена ключа, пустой отпечаток), диалога не открывает", () => {
    const changed: AppError = { ...unknown, key: "errors.ssh.hostKeyChanged", params: { ...unknown.params, kind: "hostKeyChanged" } };
    const noFingerprint: AppError = { ...unknown, params: { ...unknown.params, fingerprint: "" } };
    const { container, rerender } = render(<TrustHostKeyDialog error={changed} onTrusted={vi.fn()} onCancel={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<TrustHostKeyDialog error={noFingerprint} onTrusted={vi.fn()} onCancel={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });
});
