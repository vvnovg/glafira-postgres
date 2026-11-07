import { render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Kbd } from "./Kbd";

const realUA = navigator.userAgent;
const setUA = (ua: string) => Object.defineProperty(window.navigator, "userAgent", { value: ua, configurable: true });
afterEach(() => setUA(realUA));

describe("Kbd", () => {
  it("на macOS — символы без разделителя, скрыто от экранного диктора", () => {
    setUA("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
    const { container, rerender } = render(<Kbd keys="Mod+R" />);
    const kbd = container.querySelector("kbd")!;
    expect(kbd).toHaveTextContent("⌘R");
    expect(kbd).toHaveAttribute("aria-hidden", "true");
    rerender(<Kbd keys="Mod+Shift+F" />);
    expect(container.querySelector("kbd")).toHaveTextContent("⌘⇧F");
    rerender(<Kbd keys="Enter" />);
    expect(container.querySelector("kbd")).toHaveTextContent("⏎");
  });

  it("на Windows и Linux — Ctrl и плюсы", () => {
    setUA("Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
    const { container, rerender } = render(<Kbd keys="Mod+R" />);
    expect(container.querySelector("kbd")).toHaveTextContent("Ctrl+R");
    rerender(<Kbd keys="Mod+Shift+F" />);
    expect(container.querySelector("kbd")).toHaveTextContent("Ctrl+Shift+F");
  });
});
