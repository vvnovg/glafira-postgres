import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ICON_NAMES, Icon } from "./Icon";

describe("Icon", () => {
  it("у каждого имени есть рисунок; значок скрыт от экранного диктора и не получает фокус", () => {
    for (const name of ICON_NAMES) {
      const { container, unmount } = render(<Icon name={name} />);
      const svg = container.querySelector("svg")!;
      expect(svg).toHaveAttribute("aria-hidden", "true");
      expect(svg).toHaveAttribute("focusable", "false");
      expect(svg).toHaveAttribute("data-icon", name);
      expect(svg.childElementCount).toBeGreaterThan(0);
      unmount();
    }
  });

  it("размер и класс", () => {
    const { container } = render(<Icon name="close" size={14} className="x" />);
    const svg = container.querySelector("svg")!;
    expect(svg).toHaveAttribute("width", "14");
    expect(svg).toHaveAttribute("height", "14");
    expect(svg).toHaveClass("icon-svg", "x");
  });
});
