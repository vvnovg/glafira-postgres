import { QueryClientProvider } from "@tanstack/react-query";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionFolder, ImportPreview } from "../../../api/types";
import { renderWithClient } from "../../../test/render";

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return {
    ...orig,
    api: {
      getLicenseStatus: vi.fn(() => new Promise(() => {})),
      listFolders: vi.fn(),
      previewImport: vi.fn(),
      applyImport: vi.fn(),
    },
  };
});

import { api } from "../../../api/client";
import { ImportDialog } from "./ImportDialog";

const FOLDERS: ConnectionFolder[] = [
  { id: "a", name: "Проект А", parentId: null, order: 0 },
  { id: "b", name: "ИФТ", parentId: "a", order: 0 },
];

const PREVIEW: ImportPreview = {
  added: [
    { kind: "folder", id: "f1", name: "Новая", path: [] },
    { kind: "connection", id: "c1", name: "kafka-new", path: ["Новая"] },
  ],
  updated: [{ kind: "connection", id: "c2", name: "kafka-upd", path: ["Проект А"] }],
  unchanged: [],
  skipped: [
    {
      name: "broken",
      reason: { key: "errors.import.unsupportedConnection", params: { error: "oops" }, text: "Unsupported: oops" },
    },
  ],
  warnings: [{ key: "warnings.import.prodReadOnly", params: { count: "2" }, text: "2 prod connections will be read-only" }],
  needsPasswords: 3,
};

const PATH = "/tmp/in.glafira-postgres-connections.json";

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.listFolders).mockResolvedValue(FOLDERS);
  vi.mocked(api.previewImport).mockResolvedValue(PREVIEW);
  vi.mocked(api.applyImport).mockResolvedValue(PREVIEW);
});

function renderDialog(targetId: string | null = null) {
  const onClose = vi.fn();
  const onImported = vi.fn();
  const r = renderWithClient(<ImportDialog path={PATH} initialTargetId={targetId} onClose={onClose} onImported={onImported} />);
  return { ...r, onClose, onImported };
}

describe("ImportDialog", () => {
  it("показывает сводку и группы из предпросмотра", async () => {
    renderDialog();
    const dialog = screen.getByRole("dialog", { name: "Импорт подключений" });
    expect(await within(dialog).findByText("Добавится: 2 · Обновится: 1 · Без изменений: 0 · Пропущено: 1")).toBeInTheDocument();
    expect(api.previewImport).toHaveBeenCalledWith(PATH, null, true);
    expect(within(dialog).getByText("Новые (2)")).toBeInTheDocument();
    expect(within(dialog).getByText("Обновятся (1)")).toBeInTheDocument();
    expect(within(dialog).getByText("Пропущены (1)")).toBeInTheDocument();
    expect(within(dialog).queryByText(/^Без изменений \(/)).not.toBeInTheDocument();
    expect(within(dialog).getByText("Новая / kafka-new")).toBeInTheDocument();
    expect(within(dialog).getByText("Проект А / kafka-upd")).toBeInTheDocument();
    expect(within(dialog).getByText(/broken/)).toHaveTextContent("oops");
  });

  it("смена цели и флажка повторяет предпросмотр с новыми аргументами", async () => {
    const user = userEvent.setup();
    renderDialog();
    await screen.findByText(/^Добавится:/);
    const select = screen.getByRole("combobox", { name: "Куда импортировать" });
    expect(within(select).getByRole("option", { name: "Проект А / ИФТ" })).toBeInTheDocument();
    await user.selectOptions(select, "b");
    await waitFor(() => expect(api.previewImport).toHaveBeenLastCalledWith(PATH, "b", true));
    await user.click(screen.getByRole("checkbox", { name: "Обновлять существующие подключения" }));
    await waitFor(() => expect(api.previewImport).toHaveBeenLastCalledWith(PATH, "b", false));
  });

  it("при «Импортировать сюда» выбрана папка", async () => {
    renderDialog("a");
    await waitFor(() => expect(api.previewImport).toHaveBeenCalledWith(PATH, "a", true));
    expect(await screen.findByRole("combobox", { name: "Куда импортировать" })).toHaveValue("a");
  });

  it("показывает предупреждения и строку о паролях", async () => {
    renderDialog();
    expect(await screen.findByText(/2 prod-подключения будут только для чтения/)).toBeInTheDocument();
    expect(screen.getByText("После импорта введите пароли для 3 подключений")).toBeInTheDocument();
  });

  it("«Импортировать» применяет импорт с теми же аргументами и закрывает диалог", async () => {
    const user = userEvent.setup();
    const { onImported, client } = renderDialog();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    await screen.findByText(/^Добавится:/);
    await user.click(screen.getByRole("checkbox", { name: "Обновлять существующие подключения" }));
    await waitFor(() => expect(api.previewImport).toHaveBeenLastCalledWith(PATH, null, false));
    await user.click(screen.getByRole("button", { name: "Импортировать" }));
    expect(api.applyImport).toHaveBeenCalledWith(PATH, null, false);
    await waitFor(() => expect(onImported).toHaveBeenCalledWith(PREVIEW));
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["connections"] });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["folders"] });
  });

  it("«Отмена» закрывает диалог без импорта", async () => {
    const user = userEvent.setup();
    const { onClose } = renderDialog();
    await screen.findByText(/^Добавится:/);
    await user.click(screen.getByRole("button", { name: "Отмена" }));
    expect(onClose).toHaveBeenCalled();
    expect(api.applyImport).not.toHaveBeenCalled();
  });

  it("ошибка разбора файла показывается баннером, импорт недоступен", async () => {
    vi.mocked(api.previewImport).mockRejectedValue({
      code: "INVALID_INPUT",
      key: "errors.import.incompatibleVersion",
      message: "The file was created by an incompatible Glafira version",
      retryable: false,
    });
    renderDialog();
    expect(await screen.findByRole("alert")).toHaveTextContent("Файл создан в несовместимой версии Glafira — обновите приложение");
    expect(screen.getByRole("button", { name: "Импортировать" })).toBeDisabled();
  });

  it("Tab не уводит фокус из диалога", async () => {
    const user = userEvent.setup();
    renderWithClient(
      <>
        <button type="button">снаружи</button>
        <ImportDialog path={PATH} initialTargetId={null} onClose={vi.fn()} onImported={vi.fn()} />
      </>,
    );
    await screen.findByText(/^Добавится:/);
    const firstHint = screen.getByRole("button", { name: "Подсказка: Куда импортировать" });
    const cancel = screen.getByRole("button", { name: "Отмена" });
    expect(screen.getByRole("combobox", { name: "Куда импортировать" })).toHaveFocus();
    await user.tab({ shift: true });
    expect(firstHint).toHaveFocus();
    await user.tab({ shift: true });
    expect(cancel).toHaveFocus();
    await user.tab();
    expect(firstHint).toHaveFocus();
    for (const shift of [false, true]) {
      for (let i = 0; i < 20; i++) {
        await user.tab({ shift });
        expect(screen.getByRole("dialog")).toContainElement(document.activeElement as HTMLElement);
      }
    }
  });

  it("после закрытия фокус возвращается туда, где был до открытия", async () => {
    const outside = (
      <button type="button" key="b">
        меню
      </button>
    );
    const dialog = <ImportDialog key="d" path={PATH} initialTargetId={null} onClose={vi.fn()} onImported={vi.fn()} />;
    const { rerender, client } = renderWithClient(<>{outside}</>);
    screen.getByRole("button", { name: "меню" }).focus();
    const wrap = (children: React.ReactNode) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
    rerender(wrap([outside, dialog]));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Куда импортировать" })).toHaveFocus());
    rerender(wrap([outside]));
    expect(screen.getByRole("button", { name: "меню" })).toHaveFocus();
  });

  it("Esc при открытой подсказке закрывает только подсказку", async () => {
    const user = userEvent.setup();
    const { onClose } = renderDialog();
    await screen.findByText(/^Добавится:/);
    await user.click(screen.getByRole("button", { name: "Подсказка: Куда импортировать" }));
    expect(await screen.findByRole("tooltip")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("tooltip")).not.toBeInTheDocument());
    expect(onClose).not.toHaveBeenCalled();
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
  });
});
