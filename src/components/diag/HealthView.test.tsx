import { screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { AppError, HealthCell, HealthData } from "../../api/types";
import { diagReport } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";
import { HealthView } from "./HealthView";

const timeout: AppError = { code: "PATRONI_ERROR", key: "errors.patroni.timeout", message: "x", retryable: false };
const cell = (endpoint: string, status: number | null, error: AppError | null = status === null ? timeout : null): HealthCell => ({ endpoint, status, error });
const render = (d: HealthData) => renderWithClient(<HealthView connectionId="c" report={diagReport({ kind: "health", ...d }, { member: null })} />);

describe("HealthView", () => {
  it("сетка «участники × адреса» с кодами и ошибкой", () => {
    const cell = (endpoint: string, status: number | null): HealthCell => ({ endpoint, status, error: status === null ? { code: "PATRONI_ERROR", key: "errors.patroni.timeout", message: "x", retryable: false } : null });
    const data = { kind: "health" as const, endpoints: ["primary", "replica", "health"], members: [
      { member: "pg-1", apiUrl: "http://pg-1:8008", cells: [cell("primary", 200), cell("replica", 503), cell("health", 200)] },
      { member: "pg-2", apiUrl: "http://pg-2:8008", cells: [cell("primary", null), cell("replica", null), cell("health", null)] },
    ] };
    renderWithClient(<HealthView connectionId="c" report={diagReport(data, { member: null })} />);
    const grid = screen.getByRole("table", { name: "Матрица health" });
    expect(within(grid).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["Участник", "/primary", "/replica", "/health"]);
    expect(within(within(grid).getByRole("row", { name: /pg-1/ })).getAllByRole("cell").map((c) => c.textContent)).toEqual(["pg-1", "200", "503", "200"]);
    expect(within(grid).getByRole("row", { name: /pg-2/ })).toHaveTextContent("нет ответа");
    expect(screen.getByText(/HAProxy/)).toBeInTheDocument();
  });

  it("клетка: класс по коду (200 — ok, иной — bad, нет ответа — none); ошибка — в подсказке", () => {
    render({ endpoints: ["primary", "replica"], members: [{ member: "pg-1", apiUrl: "http://pg-1:8008", cells: [cell("primary", 200), cell("replica", 503)] }, { member: "pg-2", apiUrl: "http://pg-2:8008", cells: [cell("primary", null), cell("replica", null)] }] });
    const [, ok, bad] = within(screen.getByRole("row", { name: /pg-1/ })).getAllByRole("cell");
    expect(ok).toHaveClass("ok");
    expect(bad).toHaveClass("bad");
    const none = within(screen.getByRole("row", { name: /pg-2/ })).getAllByRole("cell")[1];
    expect(none).toHaveClass("none");
    expect(none).toHaveAttribute("title");
    expect(none.getAttribute("title")).not.toBe("");
    expect(ok).not.toHaveAttribute("title");
  });

  it("адрес REST API участника — в подсказке имени", () => {
    render({ endpoints: ["primary"], members: [{ member: "pg-1", apiUrl: "http://pg-1:8008", cells: [cell("primary", 200)] }] });
    expect(within(screen.getByRole("row", { name: /pg-1/ })).getAllByRole("cell")[0]).toHaveAttribute("title", "http://pg-1:8008");
  });

  it("участник без адреса REST API — одна клетка с пояснением вместо кодов", () => {
    render({ endpoints: ["primary", "replica"], members: [{ member: "pg-3", apiUrl: null, cells: [] }] });
    const row = screen.getByRole("row", { name: /pg-3/ });
    expect(row).toHaveTextContent("нет адреса REST API");
    expect(within(row).getAllByRole("cell")).toHaveLength(2);
    expect(within(row).getAllByRole("cell")[1]).toHaveAttribute("colspan", "2");
  });

  it("участников нет — сообщение; подпись про балансировщик остаётся", () => {
    render({ endpoints: ["primary"], members: [] });
    expect(screen.getByText(/HAProxy/)).toBeInTheDocument();
    expect(screen.getByText("Участников нет")).toBeInTheDocument();
  });

  it("чужой вид отчёта не рисуется", () => {
    const { container } = renderWithClient(<HealthView connectionId="c" report={diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] })} />);
    expect(container).toBeEmptyDOMElement();
  });
});
