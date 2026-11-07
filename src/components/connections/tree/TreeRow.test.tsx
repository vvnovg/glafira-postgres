import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { TreeRow } from "./TreeRow";

describe("TreeRow", () => {
  it("лист — кнопка со значком, именем, дополнительным текстом и aria-current у активного", async () => {
    const onActivate = vi.fn();
    render(<TreeRow icon="list" name="Топики" extra="8" current onActivate={onActivate} />);
    const row = screen.getByRole("button", { name: /Топики/ });
    expect(row.querySelector(".tree-extra")).toHaveTextContent("8");
    expect(row).toHaveAttribute("aria-current", "page");
    expect(row).toHaveClass("tree-row", "current");
    expect(row.querySelector('svg[data-icon="list"]')).not.toBeNull();
    // Колонка шеврона есть и у листа: имена одного уровня выровнены.
    expect(row.querySelector(".tree-chevron")).not.toBeNull();
    await userEvent.click(row);
    expect(onActivate).toHaveBeenCalledTimes(1);
  });

  it("узел с шевроном — кнопка раскрытия с aria-expanded, хвост строки снаружи кнопки", async () => {
    const onToggle = vi.fn();
    render(
      <TreeRow chevron={{ expanded: false, onToggle, label: "Развернуть Prod" }} icon="folder" name="Prod" bold>
        <button type="button">меню</button>
      </TreeRow>,
    );
    const toggle = screen.getByRole("button", { name: "Развернуть Prod" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle.querySelector(".tree-chevron")).not.toHaveClass("open");
    await userEvent.click(toggle);
    expect(onToggle).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "меню" }).closest(".tree-row")).toHaveClass("bold");
  });

  it("раскрытый узел поворачивает шеврон; без колонки шеврона — chevron=\"none\"", () => {
    const { container, rerender } = render(<TreeRow chevron={{ expanded: true, onToggle: () => {}, label: "x" }} name="x" />);
    expect(container.querySelector(".tree-chevron")).toHaveClass("open");
    rerender(<TreeRow chevron="none" name="Настройки" onActivate={() => {}} />);
    expect(container.querySelector(".tree-chevron")).toBeNull();
  });

  it("строка без действия — div с атрибутами rowProps и своим значком", () => {
    render(
      <TreeRow icon={<span className="dot" />} name="Local" title="Local" rowProps={{ className: "drop-before", draggable: true }}>
        <span>DEV</span>
      </TreeRow>,
    );
    const row = screen.getByText("Local").closest(".tree-row")!;
    expect(row.tagName).toBe("DIV");
    expect(row).toHaveClass("drop-before");
    expect(row).toHaveAttribute("draggable", "true");
    expect(row.querySelector(".tree-icon .dot")).not.toBeNull();
    expect(screen.getByText("Local")).toHaveAttribute("title", "Local");
  });

  it("раздельный режим (PostgreSQL): шеврон и строка — две кнопки, aria-current у строки", async () => {
    const onToggle = vi.fn();
    const onActivate = vi.fn();
    render(
      <TreeRow chevron={{ expanded: false, onToggle, label: "Партиции events" }} icon="table" name="events" extra="24" current onActivate={onActivate} activateLabel="Открыть events">
        <button type="button">меню</button>
      </TreeRow>,
    );
    const toggle = screen.getByRole("button", { name: "Партиции events" });
    const main = screen.getByRole("button", { name: "Открыть events" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(main).toHaveAttribute("aria-current", "page");
    expect(main).not.toHaveAttribute("aria-expanded");
    expect(main.querySelector('svg[data-icon="table"]')).not.toBeNull();
    expect(main.querySelector(".tree-extra")).toHaveTextContent("24");
    expect(toggle.closest(".tree-row")).toHaveClass("split", "current");
    await userEvent.click(toggle);
    expect(onToggle).toHaveBeenCalledTimes(1);
    expect(onActivate).not.toHaveBeenCalled();
    await userEvent.click(main);
    expect(onActivate).toHaveBeenCalledTimes(1);
    expect(onToggle).toHaveBeenCalledTimes(1);
  });
});
