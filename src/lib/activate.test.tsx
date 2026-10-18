import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { activateProps } from "./activate";

describe("activateProps", () => {
  it("делает элемент кнопкой с tabIndex 0", () => {
    render(<div {...activateProps(() => {})}>x</div>);
    const el = screen.getByRole("button");
    expect(el.tabIndex).toBe(0);
  });

  it("клик вызывает обработчик", async () => {
    const fn = vi.fn();
    render(<div {...activateProps(fn)}>x</div>);
    await userEvent.setup().click(screen.getByRole("button"));
    expect(fn).toHaveBeenCalledTimes(1);
  });

  it("Enter и Space вызывают обработчик, другая клавиша нет", async () => {
    const fn = vi.fn();
    const user = userEvent.setup();
    render(<div {...activateProps(fn)}>x</div>);
    await user.tab();
    await user.keyboard("{Enter}");
    expect(fn).toHaveBeenCalledTimes(1);
    await user.keyboard(" ");
    expect(fn).toHaveBeenCalledTimes(2);
    await user.keyboard("a");
    expect(fn).toHaveBeenCalledTimes(2);
  });
});
