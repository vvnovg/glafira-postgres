import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../i18n";
import { renderWithClient } from "../../test/render";
import { loadGuide } from "../../guide/chapters";
import { HelpPanel } from "./HelpPanel";

const openUrl = vi.fn();
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: (url: string) => openUrl(url) }));

vi.mock("../../guide/chapters", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../guide/chapters")>();
  return {
    ...actual,
    loadGuide: vi.fn().mockResolvedValue({
      "getting-started":
        "# Начало работы\n\nОткройте [[ui:sidebar.settings]].\n\n## Первое подключение {#first-connection}\n\nСм. [пароли](getting-started.md#passwords), [сайт](https://example.com) и [файл](../admin.md), [плохая](javascript:alert(1)).\n\nЗначение: [[ui:test.tricky]].\n\n![картинка](https://example.com/a.png)\n\n## Пароли {#passwords}\n\nПароль хранится в системном хранилище.\n\n## Без якоря\n\nТекст без якоря.\n",
    }),
  };
});

const EMPTY = {};
i18n.addResource("ru", "translation", "test.tricky", "<code>x</code> [a](b) *z*");

describe("HelpPanel", () => {
  beforeEach(() => {
    openUrl.mockReset();
    Element.prototype.scrollIntoView = vi.fn();
  });

  it("shows the contents and the first chapter", async () => {
    renderWithClient(<HelpPanel spec={EMPTY} />);
    expect(await screen.findByRole("heading", { level: 1, name: "Начало работы" })).toBeInTheDocument();
    const toc = screen.getByRole("navigation", { name: "Содержание" });
    expect(toc).toHaveTextContent("Начало работы");
    expect(toc).toHaveTextContent("Первое подключение");
    expect(toc).toHaveTextContent("Пароли");
    expect(screen.getByRole("heading", { level: 2, name: "Первое подключение" })).toHaveAttribute("id", "first-connection");
    expect(screen.getByText("Настройки").tagName).toBe("STRONG");
  });

  it("opens the chapter and scrolls to the anchor from the spec", async () => {
    renderWithClient(<HelpPanel spec={{ chapter: "getting-started", anchor: "passwords" }} />);
    expect(await screen.findByRole("heading", { level: 1, name: "Начало работы" })).toBeInTheDocument();
    await waitFor(() => expect(Element.prototype.scrollIntoView).toHaveBeenCalled());
  });

  it("follows an internal link to the chapter and anchor", async () => {
    const { container } = renderWithClient(<HelpPanel spec={EMPTY} />);
    fireEvent.click(await screen.findByRole("link", { name: "пароли" }));
    expect(await screen.findByRole("heading", { level: 1, name: "Начало работы" })).toBeInTheDocument();
    const target = container.querySelector('[id="passwords"]');
    expect(target).not.toBeNull();
    await waitFor(() => expect(Element.prototype.scrollIntoView).toHaveBeenCalled());
    expect(openUrl).not.toHaveBeenCalled();
  });

  it("opens https links in the system browser and leaves other links inactive", async () => {
    renderWithClient(<HelpPanel spec={EMPTY} />);
    fireEvent.click(await screen.findByRole("link", { name: "сайт" }));
    expect(openUrl).toHaveBeenCalledWith("https://example.com");
    expect(screen.queryByRole("link", { name: "файл" })).toBeNull();
    expect(screen.getByText("файл")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "плохая" })).toBeNull();
    expect(screen.getByText("плохая")).toBeInTheDocument();
  });

  it("renders label values literally and never loads images", async () => {
    const { container } = renderWithClient(<HelpPanel spec={EMPTY} />);
    expect(await screen.findByText("<code>x</code> [a](b) *z*", { selector: "strong" })).toBeInTheDocument();
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("code")).toBeNull();
  });

  it("renders a heading without an anchor", async () => {
    renderWithClient(<HelpPanel spec={EMPTY} />);
    expect(await screen.findByRole("heading", { level: 2, name: "Без якоря" })).not.toHaveAttribute("id");
  });

  it("shows an error when the guide fails to load", async () => {
    vi.mocked(loadGuide).mockRejectedValueOnce(new Error("boom"));
    renderWithClient(<HelpPanel spec={EMPTY} />);
    expect(await screen.findByText("Не удалось загрузить руководство")).toBeInTheDocument();
  });

  it("searches and jumps to a result", async () => {
    renderWithClient(<HelpPanel spec={EMPTY} />);
    await screen.findByRole("heading", { level: 1, name: "Начало работы" });
    const input = screen.getByRole("searchbox", { name: "Поиск по руководству" });
    fireEvent.change(input, { target: { value: "ХРАНИЛИЩЕ" } });
    const hit = screen.getByRole("button", { name: /Начало работы › Пароли/ });
    fireEvent.click(hit);
    await waitFor(() => expect(Element.prototype.scrollIntoView).toHaveBeenCalled());
    fireEvent.change(input, { target: { value: "zzzz" } });
    expect(screen.getByText("Ничего не найдено")).toBeInTheDocument();
  });

  it("re-renders labels when the language changes", async () => {
    renderWithClient(<HelpPanel spec={EMPTY} />);
    expect(await screen.findByText("Настройки")).toBeInTheDocument();
    await act(async () => {
      await i18n.changeLanguage("en");
    });
    expect(await screen.findByText("Settings")).toBeInTheDocument();
    expect(loadGuide).toHaveBeenCalledWith("en");
    await act(async () => {
      await i18n.changeLanguage("ru");
    });
    expect(await screen.findByText("Настройки")).toBeInTheDocument();
  });
});
