import { screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { renderWithClient } from "../test/render";
import { TabContent } from "./TabContent";

vi.mock("./help/HelpPanel", () => ({
  HelpPanel: () => {
    throw new Error("chunk broken");
  },
}));

describe("TabContent help", () => {
  beforeEach(() => vi.spyOn(console, "error").mockImplementation(() => {}));
  afterEach(() => vi.restoreAllMocks());

  it("shows the load error instead of crashing when the guide fails", async () => {
    renderWithClient(<TabContent tab={{ kind: "help", id: "help" }} />);
    expect(await screen.findByText("Не удалось загрузить руководство")).toBeInTheDocument();
  });
});
