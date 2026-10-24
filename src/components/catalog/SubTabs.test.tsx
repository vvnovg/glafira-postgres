import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { SubTabs } from "./SubTabs";

type Id = "a" | "b" | "c";
const tabs: { id: Id; label: string }[] = [
  { id: "a", label: "Первая" },
  { id: "b", label: "Вторая" },
  { id: "c", label: "Третья" },
];

describe("SubTabs", () => {
  it("содержимое строится по выбранной подвкладке", () => {
    render(
      <SubTabs label="Разделы" tabs={tabs} value="b" onChange={() => {}}>
        {(active) => <p>тело {active}</p>}
      </SubTabs>,
    );
    expect(screen.getByRole("tab", { name: "Вторая" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("тело b")).toBeInTheDocument();
    expect(screen.getByRole("tabpanel")).toHaveAccessibleName("Вторая");
  });

  it("подвкладки value нет среди tabs: выбрана и показана первая — подсветка и тело согласованы", () => {
    const body = vi.fn((active: Id) => <p>тело {active}</p>);
    // «gone» — подвкладка, которая исчезла после обновления данных (тип сужен до Id через приведение).
    render(
      <SubTabs label="Разделы" tabs={tabs} value={"gone" as Id} onChange={() => {}}>
        {body}
      </SubTabs>,
    );
    expect(screen.getByRole("tab", { name: "Первая" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tab", { name: "Первая" })).toHaveAttribute("tabindex", "0");
    expect(screen.getByRole("tab", { name: "Вторая" })).toHaveAttribute("aria-selected", "false");
    expect(screen.getByText("тело a")).toBeInTheDocument();
    expect(screen.getByRole("tabpanel")).toHaveAccessibleName("Первая");
    // Тело ни разу не строилось по исчезнувшей подвкладке.
    expect(body.mock.calls.every(([active]) => active === "a")).toBe(true);
  });

  it("стрелки от замещённой подвкладки идут от первой", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SubTabs label="Разделы" tabs={tabs} value={"gone" as Id} onChange={onChange}>
        {(active) => <p>тело {active}</p>}
      </SubTabs>,
    );
    screen.getByRole("tab", { name: "Первая" }).focus();
    await user.keyboard("{ArrowRight}");
    expect(onChange).toHaveBeenCalledWith("b");
  });
});
