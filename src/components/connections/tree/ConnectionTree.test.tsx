import { createEvent, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionFolder, ConnectionProfile } from "../../../api/types";
import { renderWithClient } from "../../../test/render";
import { newProfile } from "../profile";

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return {
    ...orig,
    api: {
      getLicenseStatus: vi.fn(() => new Promise(() => {})),
      // Топология подключений не нужна дереву папок: запрос остаётся в ожидании.
      getTopology: vi.fn(() => new Promise(() => {})),
      listFolders: vi.fn(),
      listConnections: vi.fn(),
      createFolder: vi.fn(),
      renameFolder: vi.fn(),
      deleteFolder: vi.fn(),
      moveItems: vi.fn(),
      exportConnections: vi.fn(),
      previewImport: vi.fn(),
      applyImport: vi.fn(),
    },
  };
});
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn() }));

import { open as openDialog, save } from "@tauri-apps/plugin-dialog";
import { api } from "../../../api/client";
import { useWorkspace } from "../../../state/workspace";
import { ConnectionTree } from "./ConnectionTree";

const FOLDERS: ConnectionFolder[] = [
  { id: "a", name: "Проект А", parentId: null, order: 0 },
  { id: "b", name: "ИФТ", parentId: "a", order: 0 },
];
const CONNS: ConnectionProfile[] = [
  { ...newProfile(), id: "x", name: "kafka-ift", folderId: "b", order: 0 },
  { ...newProfile(), id: "y", name: "local", folderId: null, order: 0 },
];

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  vi.mocked(api.listFolders).mockResolvedValue(FOLDERS);
  vi.mocked(api.listConnections).mockResolvedValue(CONNS);
  useWorkspace.setState({ tabs: [], activeId: null });
});

/** Строка папки (с кнопкой «⋯» и целью перетаскивания) по имени. */
const folderRow = (name: string) => screen.getByRole("button", { name: `Развернуть или свернуть ${name}` }).closest(".tree-folder") as HTMLElement;
const connRow = (name: string) => screen.getByText(name).closest(".tree-conn") as HTMLElement;

function stubDataTransfer() {
  const data = new Map<string, string>();
  return {
    data,
    dropEffect: "none",
    effectAllowed: "all",
    setData: (type: string, value: string) => data.set(type, value),
    getData: (type: string) => data.get(type) ?? "",
    types: [] as string[],
  };
}

/** dragover/drop с clientY: в jsdom нет DragEvent, координату задаём на событии сами. */
function dragAt(type: "dragOver" | "drop", el: HTMLElement, dataTransfer: object, clientY: number) {
  const event = createEvent[type](el, { dataTransfer });
  Object.defineProperty(event, "clientY", { value: clientY });
  return fireEvent(el, event);
}

async function openFolderMenu(name: string) {
  const user = userEvent.setup();
  await user.click(within(folderRow(name)).getByRole("button", { name: "Ещё действия" }));
  return { user, menu: screen.getByRole("menu") };
}

describe("ConnectionTree", () => {
  it("показывает папки с подключениями и пункты подключения", async () => {
    renderWithClient(<ConnectionTree />);
    expect(await screen.findByText("kafka-ift")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Развернуть или свернуть Проект А" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: "Развернуть или свернуть ИФТ" })).toBeInTheDocument();
    expect(screen.getByText("local")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /Развернуть или свернуть подключение/ })).toHaveLength(2);
  });

  it("сворачивание папки сохраняется между перерисовками", async () => {
    const user = userEvent.setup();
    const first = renderWithClient(<ConnectionTree />);
    await screen.findByText("kafka-ift");
    await user.click(screen.getByRole("button", { name: "Развернуть или свернуть Проект А" }));
    expect(screen.queryByText("kafka-ift")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Развернуть или свернуть Проект А" })).toHaveAttribute("aria-expanded", "false");
    expect(JSON.parse(localStorage.getItem("glafira.tree.collapsed") ?? "[]")).toEqual(["a"]);
    first.unmount();

    renderWithClient(<ConnectionTree />);
    expect(await screen.findByText("local")).toBeInTheDocument();
    expect(screen.queryByText("kafka-ift")).not.toBeInTheDocument();
  });

  it("сворачивание пишет localStorage один раз (без побочных эффектов в updater)", async () => {
    const user = userEvent.setup();
    renderWithClient(
      <StrictMode>
        <ConnectionTree />
      </StrictMode>,
    );
    await screen.findByText("kafka-ift");
    const setItem = vi.spyOn(Storage.prototype, "setItem");
    await user.click(screen.getByRole("button", { name: "Развернуть или свернуть Проект А" }));
    expect(setItem.mock.calls.filter(([k]) => k === "glafira.tree.collapsed")).toEqual([["glafira.tree.collapsed", '["a"]']]);
    setItem.mockRestore();
  });

  it("подсказка о пустом списке не мелькает, пока папки загружаются", async () => {
    let resolveFolders: (f: ConnectionFolder[]) => void = () => {};
    vi.mocked(api.listFolders).mockReturnValue(new Promise((r) => (resolveFolders = r)));
    vi.mocked(api.listConnections).mockResolvedValue([]);
    renderWithClient(<ConnectionTree />);
    await waitFor(() => expect(api.listConnections).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.queryByText(/Добавьте первое подключение/)).not.toBeInTheDocument();
    resolveFolders([]);
    expect(await screen.findByText(/Добавьте первое подключение/)).toBeInTheDocument();
  });

  it("строка папки с открытым меню не перетаскивается", async () => {
    renderWithClient(<ConnectionTree />);
    await screen.findByText("kafka-ift");
    expect(folderRow("Проект А")).toHaveAttribute("draggable", "true");
    await openFolderMenu("Проект А");
    expect(folderRow("Проект А")).toHaveAttribute("draggable", "false");
    expect(folderRow("ИФТ")).toHaveAttribute("draggable", "true");
  });

  it("испорченное значение в localStorage не ломает дерево", async () => {
    localStorage.setItem("glafira.tree.collapsed", "{not json");
    renderWithClient(<ConnectionTree />);
    expect(await screen.findByText("kafka-ift")).toBeInTheDocument();
  });

  it("фильтр скрывает несовпавшие, разворачивает папки и сообщает, если ничего нет", async () => {
    localStorage.setItem("glafira.tree.collapsed", JSON.stringify(["a"]));
    const user = userEvent.setup();
    renderWithClient(<ConnectionTree />);
    await screen.findByText("local");
    expect(screen.queryByText("kafka-ift")).not.toBeInTheDocument();

    const filter = screen.getByRole("searchbox", { name: "Фильтр подключений" });
    await user.type(filter, "ift");
    expect(screen.getByText("kafka-ift")).toBeInTheDocument();
    expect(screen.queryByText("local")).not.toBeInTheDocument();
    expect(screen.queryByText("Ничего не найдено")).not.toBeInTheDocument();

    await user.clear(filter);
    await user.type(filter, "zzz");
    expect(screen.getByText("Ничего не найдено")).toBeInTheDocument();
    expect(screen.queryByText("kafka-ift")).not.toBeInTheDocument();

    await user.clear(filter);
    expect(screen.getByText("local")).toBeInTheDocument();
    expect(screen.queryByText("kafka-ift")).not.toBeInTheDocument();
  });

  it("подключения видны, даже если папки не загрузились", async () => {
    vi.mocked(api.listFolders).mockRejectedValue(new Error("boom"));
    renderWithClient(<ConnectionTree />);
    expect(await screen.findByText("kafka-ift")).toBeInTheDocument();
    expect(screen.getByText("local")).toBeInTheDocument();
  });

  it("«+ папка» создаёт папку в корне по Enter", async () => {
    vi.mocked(api.createFolder).mockResolvedValue({ id: "n", name: "ИФТ", parentId: null, order: 1 });
    const user = userEvent.setup();
    renderWithClient(<ConnectionTree />);
    await screen.findByText("local");
    await user.click(screen.getByRole("button", { name: "Новая папка" }));
    const input = screen.getByRole("textbox", { name: "Имя папки" });
    expect(input).toHaveFocus();
    await user.type(input, "ИФТ{Enter}");
    expect(api.createFolder).toHaveBeenCalledWith("ИФТ", null);
    expect(await screen.findByText("local")).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "Имя папки" })).not.toBeInTheDocument();
  });

  it("Esc отменяет ввод имени", async () => {
    const user = userEvent.setup();
    renderWithClient(<ConnectionTree />);
    await screen.findByText("local");
    await user.click(screen.getByRole("button", { name: "Новая папка" }));
    await user.type(screen.getByRole("textbox", { name: "Имя папки" }), "x{Escape}");
    expect(screen.queryByRole("textbox", { name: "Имя папки" })).not.toBeInTheDocument();
    expect(api.createFolder).not.toHaveBeenCalled();
  });

  it("ошибка из Rust показывается под полем имени", async () => {
    vi.mocked(api.createFolder).mockRejectedValue({
      code: "INVALID_INPUT",
      key: "errors.folders.nameTaken",
      params: { name: "ИФТ" },
      message: "Folder “ИФТ” already exists here",
      retryable: false,
    });
    renderWithClient(<ConnectionTree />);
    await screen.findByText("kafka-ift");
    const { user } = await openFolderMenu("Проект А");
    await user.click(screen.getByRole("menuitem", { name: "Новая папка" }));
    const input = screen.getByRole("textbox", { name: "Имя папки" });
    await user.type(input, "ИФТ{Enter}");
    expect(api.createFolder).toHaveBeenCalledWith("ИФТ", "a");
    expect(await screen.findByText("Папка «ИФТ» здесь уже есть")).toBeInTheDocument();
    expect(input).toBeInTheDocument();
  });

  it("меню папки открывается правым кликом и закрывается по Esc", async () => {
    const user = userEvent.setup();
    renderWithClient(<ConnectionTree />);
    await screen.findByText("kafka-ift");
    fireEvent.contextMenu(folderRow("ИФТ"));
    const menu = screen.getByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual([
      "Новая папка",
      "Новое подключение здесь",
      "Переименовать",
      "Удалить",
      // «Экспортировать папку…» и «Импортировать сюда…» вернёт Task 19 (флаг EXCHANGE в ConnectionTree).
    ]);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("меню закрывается кликом вне его", async () => {
    const user = userEvent.setup();
    renderWithClient(<ConnectionTree />);
    await screen.findByText("kafka-ift");
    await openFolderMenu("ИФТ");
    await user.click(screen.getByText("local"));
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("переименование через меню", async () => {
    vi.mocked(api.renameFolder).mockResolvedValue({ id: "a", name: "Проект Б", parentId: null, order: 0 });
    renderWithClient(<ConnectionTree />);
    await screen.findByText("kafka-ift");
    const { user } = await openFolderMenu("Проект А");
    await user.click(screen.getByRole("menuitem", { name: "Переименовать" }));
    const input = screen.getByRole("textbox", { name: "Имя папки" });
    expect(input).toHaveValue("Проект А");
    await user.clear(input);
    await user.type(input, "Проект Б{Enter}");
    expect(api.renameFolder).toHaveBeenCalledWith("a", "Проект Б");
  });

  it("удаление с подтверждением внутри страницы и ошибка «не пуста»", async () => {
    vi.mocked(api.deleteFolder).mockRejectedValueOnce({
      code: "INVALID_INPUT",
      key: "errors.folders.notEmpty",
      message: "Folder is not empty",
      retryable: false,
    });
    renderWithClient(<ConnectionTree />);
    await screen.findByText("kafka-ift");
    const { user } = await openFolderMenu("Проект А");
    await user.click(screen.getByRole("menuitem", { name: "Удалить" }));
    expect(api.deleteFolder).not.toHaveBeenCalled();
    const group = screen.getByRole("group", { name: "Удалить папку «Проект А»?" });
    await user.click(within(group).getByRole("button", { name: "Удалить" }));
    expect(api.deleteFolder).toHaveBeenCalledWith("a");
    expect(await within(group).findByText(/Папка не пуста/)).toBeInTheDocument();

    await user.click(within(group).getByRole("button", { name: "Отмена" }));
    expect(screen.queryByRole("group", { name: "Удалить папку «Проект А»?" })).not.toBeInTheDocument();
  });

  it("удаление пустой папки", async () => {
    vi.mocked(api.deleteFolder).mockResolvedValue(undefined);
    renderWithClient(<ConnectionTree />);
    await screen.findByText("kafka-ift");
    const { user } = await openFolderMenu("ИФТ");
    await user.click(screen.getByRole("menuitem", { name: "Удалить" }));
    await user.click(within(screen.getByRole("group", { name: "Удалить папку «ИФТ»?" })).getByRole("button", { name: "Удалить" }));
    expect(api.deleteFolder).toHaveBeenCalledWith("b");
    await vi.waitFor(() => expect(screen.queryByRole("group")).not.toBeInTheDocument());
  });

  it("«Новое подключение здесь» открывает форму с папкой", async () => {
    renderWithClient(<ConnectionTree />);
    await screen.findByText("kafka-ift");
    const { user } = await openFolderMenu("ИФТ");
    await user.click(screen.getByRole("menuitem", { name: "Новое подключение здесь" }));
    expect(useWorkspace.getState().tabs).toEqual([
      expect.objectContaining({ kind: "connection-form", connectionId: null, folderId: "b" }),
    ]);
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("перетаскивание подключения на папку переносит его внутрь", async () => {
    vi.mocked(api.moveItems).mockResolvedValue(undefined);
    renderWithClient(<ConnectionTree />);
    await screen.findByText("kafka-ift");
    const dataTransfer = stubDataTransfer();
    fireEvent.dragStart(connRow("local"), { dataTransfer });
    const notPrevented = fireEvent.dragOver(folderRow("ИФТ"), { dataTransfer });
    expect(notPrevented).toBe(false);
    expect(folderRow("ИФТ")).toHaveClass("drop-into");
    fireEvent.drop(folderRow("ИФТ"), { dataTransfer });
    expect(api.moveItems).toHaveBeenCalledWith([{ kind: "connection", id: "y" }], "b", null);
  });

  it("перетаскивание на подключение ставит перед ним в его папку", async () => {
    vi.mocked(api.moveItems).mockResolvedValue(undefined);
    renderWithClient(<ConnectionTree />);
    await screen.findByText("kafka-ift");
    const dataTransfer = stubDataTransfer();
    fireEvent.dragStart(connRow("local"), { dataTransfer });
    fireEvent.dragOver(connRow("kafka-ift"), { dataTransfer });
    fireEvent.drop(connRow("kafka-ift"), { dataTransfer });
    expect(api.moveItems).toHaveBeenCalledWith([{ kind: "connection", id: "y" }], "b", { kind: "connection", id: "x" });
  });

  it("папку нельзя бросить в её потомка", async () => {
    renderWithClient(<ConnectionTree />);
    await screen.findByText("kafka-ift");
    const dataTransfer = stubDataTransfer();
    fireEvent.dragStart(folderRow("Проект А"), { dataTransfer });
    const notPrevented = fireEvent.dragOver(folderRow("ИФТ"), { dataTransfer });
    expect(notPrevented).toBe(true);
    expect(folderRow("ИФТ")).not.toHaveClass("drop-into");
    fireEvent.drop(folderRow("ИФТ"), { dataTransfer });
    expect(api.moveItems).not.toHaveBeenCalled();
  });

  it("перетаскивание на заголовок панели переносит в корень", async () => {
    vi.mocked(api.moveItems).mockResolvedValue(undefined);
    renderWithClient(<ConnectionTree />);
    await screen.findByText("kafka-ift");
    const dataTransfer = stubDataTransfer();
    const header = screen.getByText("Подключения").closest(".sidebar-header") as HTMLElement;
    fireEvent.dragStart(folderRow("ИФТ"), { dataTransfer });
    expect(fireEvent.dragOver(header, { dataTransfer })).toBe(false);
    fireEvent.drop(header, { dataTransfer });
    expect(api.moveItems).toHaveBeenCalledWith([{ kind: "folder", id: "b" }], null, null);
  });

  describe("порядок папок и цели перетаскивания", () => {
    const SIBLINGS: ConnectionFolder[] = [
      { id: "A", name: "Альфа", parentId: null, order: 0 },
      { id: "B", name: "Бета", parentId: null, order: 1 },
      { id: "C", name: "Вложенная", parentId: "A", order: 0 },
    ];

    /** Строка 20px сверху на 100px: верхние ~30% — «перед», остальное — «внутрь». */
    function stubRect(el: HTMLElement) {
      el.getBoundingClientRect = () => ({ top: 100, bottom: 120, height: 20, left: 0, right: 200, width: 200, x: 0, y: 100, toJSON: () => ({}) });
    }

    beforeEach(() => {
      vi.mocked(api.listFolders).mockResolvedValue(SIBLINGS);
      vi.mocked(api.moveItems).mockResolvedValue(undefined);
    });

    it("папка, брошенная на верх строки соседней папки, встаёт перед ней", async () => {
      renderWithClient(<ConnectionTree />);
      await screen.findByText("local");
      const target = folderRow("Альфа");
      stubRect(target);
      const dataTransfer = stubDataTransfer();
      fireEvent.dragStart(folderRow("Бета"), { dataTransfer });
      expect(dragAt("dragOver", target, dataTransfer, 102)).toBe(false);
      expect(target).toHaveClass("drop-before");
      expect(target).not.toHaveClass("drop-into");
      dragAt("drop", target, dataTransfer, 102);
      expect(api.moveItems).toHaveBeenCalledWith([{ kind: "folder", id: "B" }], null, { kind: "folder", id: "A" });
    });

    it("ниже верхней зоны строки папки — внутрь", async () => {
      renderWithClient(<ConnectionTree />);
      await screen.findByText("local");
      const target = folderRow("Альфа");
      stubRect(target);
      const dataTransfer = stubDataTransfer();
      fireEvent.dragStart(folderRow("Бета"), { dataTransfer });
      dragAt("dragOver", target, dataTransfer, 112);
      expect(target).toHaveClass("drop-into");
      dragAt("drop", target, dataTransfer, 112);
      expect(api.moveItems).toHaveBeenCalledWith([{ kind: "folder", id: "B" }], "A", null);
    });

    it("подключение на верх строки папки кладётся внутрь (зона «перед» — только для папок)", async () => {
      renderWithClient(<ConnectionTree />);
      await screen.findByText("local");
      const target = folderRow("Альфа");
      stubRect(target);
      const dataTransfer = stubDataTransfer();
      fireEvent.dragStart(connRow("local"), { dataTransfer });
      dragAt("dragOver", target, dataTransfer, 101);
      expect(target).toHaveClass("drop-into");
      dragAt("drop", target, dataTransfer, 101);
      expect(api.moveItems).toHaveBeenCalledWith([{ kind: "connection", id: "y" }], "A", null);
    });

    it("папка на подключение — в конец папок этого уровня, подсвечивается последняя папка", async () => {
      renderWithClient(<ConnectionTree />);
      await screen.findByText("local");
      const dataTransfer = stubDataTransfer();
      fireEvent.dragStart(folderRow("Вложенная"), { dataTransfer });
      expect(fireEvent.dragOver(connRow("local"), { dataTransfer })).toBe(false);
      expect(connRow("local")).not.toHaveClass("drop-before");
      expect(folderRow("Бета")).toHaveClass("drop-after");
      fireEvent.drop(connRow("local"), { dataTransfer });
      expect(api.moveItems).toHaveBeenCalledWith([{ kind: "folder", id: "C" }], null, null);
    });

    it("папку нельзя поставить перед ней самой", async () => {
      renderWithClient(<ConnectionTree />);
      await screen.findByText("local");
      const target = folderRow("Бета");
      stubRect(target);
      const dataTransfer = stubDataTransfer();
      fireEvent.dragStart(target, { dataTransfer });
      expect(dragAt("dragOver", target, dataTransfer, 101)).toBe(true);
      dragAt("drop", target, dataTransfer, 101);
      expect(api.moveItems).not.toHaveBeenCalled();
    });
  });

  describe("доступность меню", () => {
    it("триггер: aria-haspopup и aria-expanded; Esc возвращает фокус и не уходит в document", async () => {
      const user = userEvent.setup();
      renderWithClient(<ConnectionTree />);
      await screen.findByText("kafka-ift");
      const trigger = within(folderRow("ИФТ")).getByRole("button", { name: "Ещё действия" });
      expect(trigger).toHaveAttribute("aria-haspopup", "menu");
      expect(trigger).toHaveAttribute("aria-expanded", "false");
      await user.click(trigger);
      expect(trigger).toHaveAttribute("aria-expanded", "true");
      expect(screen.getAllByRole("menuitem")[0]).toHaveFocus();
      const docKey = vi.fn();
      document.addEventListener("keydown", docKey);
      try {
        await user.keyboard("{Escape}");
      } finally {
        document.removeEventListener("keydown", docKey);
      }
      expect(screen.queryByRole("menu")).not.toBeInTheDocument();
      expect(docKey).not.toHaveBeenCalled();
      expect(trigger).toHaveFocus();
      expect(trigger).toHaveAttribute("aria-expanded", "false");
    });

    it("Tab закрывает меню", async () => {
      const user = userEvent.setup();
      renderWithClient(<ConnectionTree />);
      await screen.findByText("kafka-ift");
      const header = screen.getByText("Подключения").closest(".sidebar-header") as HTMLElement;
      await user.click(within(header).getByRole("button", { name: "Ещё действия" }));
      expect(screen.getByRole("menu")).toBeInTheDocument();
      await user.tab();
      expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    });

    it("перетаскивание не начинается из меню", async () => {
      renderWithClient(<ConnectionTree />);
      await screen.findByText("kafka-ift");
      await openFolderMenu("ИФТ");
      const dataTransfer = stubDataTransfer();
      const notPrevented = fireEvent.dragStart(screen.getAllByRole("menuitem")[0], { dataTransfer });
      expect(notPrevented).toBe(false);
      fireEvent.dragOver(folderRow("Проект А"), { dataTransfer });
      fireEvent.drop(folderRow("Проект А"), { dataTransfer });
      expect(api.moveItems).not.toHaveBeenCalled();
    });
  });

  it("пустое поле имени отменяется при потере фокуса, непустое остаётся", async () => {
    const user = userEvent.setup();
    renderWithClient(<ConnectionTree />);
    await screen.findByText("local");
    await user.click(screen.getByRole("button", { name: "Новая папка" }));
    fireEvent.blur(screen.getByRole("textbox", { name: "Имя папки" }));
    expect(screen.queryByRole("textbox", { name: "Имя папки" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Новая папка" }));
    await user.type(screen.getByRole("textbox", { name: "Имя папки" }), "x");
    fireEvent.blur(screen.getByRole("textbox", { name: "Имя папки" }));
    expect(screen.getByRole("textbox", { name: "Имя папки" })).toHaveValue("x");
  });
  // Команды экспорта и импорта вернёт Task 19: пока пункты меню скрыты флагом EXCHANGE, тесты пропущены.
  describe("экспорт и импорт", () => {
    const EMPTY_PREVIEW = { added: [], updated: [], unchanged: [], skipped: [], warnings: [], needsPasswords: 0 };

    async function openHeaderMenu() {
      const user = userEvent.setup();
      await screen.findByText("local");
      const header = screen.getByText("Подключения").closest(".sidebar-header") as HTMLElement;
      await user.click(within(header).getByRole("button", { name: "Ещё действия" }));
      return { user, menu: screen.getByRole("menu") };
    }

    it.skip("«Экспортировать папку…» сохраняет папку и сообщает итог", async () => {
      vi.mocked(save).mockResolvedValue("/tmp/a.json");
      vi.mocked(api.exportConnections).mockResolvedValue({ connections: 1, folders: 2 });
      renderWithClient(<ConnectionTree />);
      await screen.findByText("kafka-ift");
      const { user, menu } = await openFolderMenu("Проект А");
      await user.click(within(menu).getByRole("menuitem", { name: "Экспортировать папку…" }));
      expect(save).toHaveBeenCalledWith({
        defaultPath: "Проект А.glafira-connections.json",
        filters: [{ name: "Glafira", extensions: ["json"] }],
      });
      expect(api.exportConnections).toHaveBeenCalledWith("/tmp/a.json", "a");
      expect(await screen.findByRole("status")).toHaveTextContent("Сохранено: подключений 1, папок 2");
    });

    it.skip("«Экспортировать всё…» передаёт null и имя файла с датой", async () => {
      vi.mocked(save).mockResolvedValue("/tmp/all.json");
      vi.mocked(api.exportConnections).mockResolvedValue({ connections: 2, folders: 2 });
      renderWithClient(<ConnectionTree />);
      const { user, menu } = await openHeaderMenu();
      await user.click(within(menu).getByRole("menuitem", { name: "Экспортировать всё…" }));
      const arg = vi.mocked(save).mock.calls[0][0];
      expect(arg?.defaultPath).toMatch(/^glafira-connections-\d{4}-\d{2}-\d{2}\.glafira-connections\.json$/);
      expect(api.exportConnections).toHaveBeenCalledWith("/tmp/all.json", null);
    });

    it.skip("отмена диалога сохранения ничего не вызывает", async () => {
      vi.mocked(save).mockResolvedValue(null);
      renderWithClient(<ConnectionTree />);
      await screen.findByText("kafka-ift");
      const { user, menu } = await openFolderMenu("ИФТ");
      await user.click(within(menu).getByRole("menuitem", { name: "Экспортировать папку…" }));
      await waitFor(() => expect(save).toHaveBeenCalled());
      expect(api.exportConnections).not.toHaveBeenCalled();
      expect(screen.queryByRole("status")).not.toBeInTheDocument();
    });

    it.skip("ошибка экспорта показывается баннером", async () => {
      vi.mocked(save).mockResolvedValue("/tmp/a.json");
      vi.mocked(api.exportConnections).mockRejectedValue({ code: "INTERNAL", message: "disk full", retryable: false });
      renderWithClient(<ConnectionTree />);
      const { user, menu } = await openHeaderMenu();
      await user.click(within(menu).getByRole("menuitem", { name: "Экспортировать всё…" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("disk full");
    });

    it.skip("«Импортировать сюда…» открывает диалог с этой папкой и сообщает итог", async () => {
      vi.mocked(openDialog).mockResolvedValue("/tmp/in.json");
      vi.mocked(api.previewImport).mockResolvedValue(EMPTY_PREVIEW);
      vi.mocked(api.applyImport).mockResolvedValue({
        ...EMPTY_PREVIEW,
        added: [{ kind: "connection", id: "n", name: "n", path: [] }],
        needsPasswords: 1,
      });
      renderWithClient(<ConnectionTree />);
      await screen.findByText("kafka-ift");
      const { user, menu } = await openFolderMenu("ИФТ");
      await user.click(within(menu).getByRole("menuitem", { name: "Импортировать сюда…" }));
      expect(openDialog).toHaveBeenCalledWith({ multiple: false, directory: false, filters: [{ name: "Glafira", extensions: ["json"] }] });
      const dialog = await screen.findByRole("dialog", { name: "Импорт подключений" });
      await waitFor(() => expect(api.previewImport).toHaveBeenCalledWith("/tmp/in.json", "b", true));
      await user.click(within(dialog).getByRole("button", { name: "Импортировать" }));
      expect(api.applyImport).toHaveBeenCalledWith("/tmp/in.json", "b", true);
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
      const status = screen.getByRole("status");
      expect(status).toHaveTextContent("Импортировано: добавлено 1, обновлено 0");
      expect(status).toHaveTextContent("Введите пароли для 1 подключения");
    });

    it.skip("«Импортировать…» в заголовке: отмена выбора файла не открывает диалог", async () => {
      vi.mocked(openDialog).mockResolvedValue(null);
      renderWithClient(<ConnectionTree />);
      const { user, menu } = await openHeaderMenu();
      await user.click(within(menu).getByRole("menuitem", { name: "Импортировать…" }));
      await waitFor(() => expect(openDialog).toHaveBeenCalled());
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      expect(api.previewImport).not.toHaveBeenCalled();
    });
  });
});
