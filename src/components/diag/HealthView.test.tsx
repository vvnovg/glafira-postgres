import { screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { AppError, HealthCell, HealthData, HealthGrade } from "../../api/types";
import { diagReport } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";
import { HealthView } from "./HealthView";

const timeout: AppError = { code: "PATRONI_ERROR", key: "errors.patroni.timeout", message: "x", retryable: false };
const cell = (endpoint: string, status: number | null, grade: HealthGrade = status === 200 ? "ok" : "bad"): HealthCell => ({ endpoint, status, error: status === null ? timeout : null, grade });
const render = (d: HealthData) => renderWithClient(<HealthView connectionId="c" report={diagReport({ kind: "health", ...d }, { member: null })} />);

describe("HealthView", () => {
  it("сетка «участники × адреса» с кодами и ошибкой", () => {
    const data = { kind: "health" as const, endpoints: ["primary", "replica", "health"], members: [
      { member: "pg-1", apiUrl: "http://pg-1:8008", cells: [cell("primary", 200), cell("replica", 503, "notApplicable"), cell("health", 200)] },
      { member: "pg-2", apiUrl: "http://pg-2:8008", cells: [cell("primary", null), cell("replica", null), cell("health", null)] },
    ] };
    renderWithClient(<HealthView connectionId="c" report={diagReport(data, { member: null })} />);
    const grid = screen.getByRole("table", { name: "Матрица health" });
    expect(within(grid).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["Участник", "/primary", "/replica", "/health"]);
    expect(within(within(grid).getByRole("row", { name: /pg-1/ })).getAllByRole("cell").map((c) => c.textContent)).toEqual(["pg-1", "200", "503", "200"]);
    expect(within(grid).getByRole("row", { name: /pg-2/ })).toHaveTextContent("нет ответа");
    expect(screen.getByText(/HAProxy/)).toBeInTheDocument();
  });

  it("клетка: класс по оценке роли, а не по коду; подсказка — смысл оценки или причина молчания", () => {
    render({ endpoints: ["primary", "replica", "read-only", "health"], members: [
      { member: "pg-1", apiUrl: "http://pg-1:8008", cells: [cell("primary", 503, "notApplicable"), cell("replica", 503, "degraded"), cell("read-only", 200), cell("health", 503, "bad")] },
      { member: "pg-2", apiUrl: "http://pg-2:8008", cells: [cell("primary", null), cell("replica", null), cell("read-only", null), cell("health", null)] },
    ] });
    const [, na, degraded, ok, bad] = within(screen.getByRole("row", { name: /pg-1/ })).getAllByRole("cell");
    expect(na).toHaveClass("na");
    expect(na).toHaveAttribute("title", "Адрес не относится к роли участника: 503 здесь норма");
    expect(degraded).toHaveClass("degraded");
    expect(degraded).toHaveAttribute("title", "Узел работает, но балансировщик не пошлёт на него трафик этого адреса");
    expect(ok).toHaveClass("ok");
    expect(ok).not.toHaveAttribute("title");
    expect(bad).toHaveClass("bad");
    expect(bad).toHaveAttribute("title", "Ответ противоречит роли участника или узел нездоров");
    // API не ответил: клетка красная, в подсказке — причина, а не общий смысл оценки.
    const silent = within(screen.getByRole("row", { name: /pg-2/ })).getAllByRole("cell")[1];
    expect(silent).toHaveClass("bad");
    expect(silent).toHaveTextContent("нет ответа");
    expect(silent.getAttribute("title")).not.toBe("Ответ противоречит роли участника или узел нездоров");
    expect(silent.getAttribute("title")).not.toBe("");
  });

  it("под матрицей — легенда цветов", () => {
    render({ endpoints: ["primary"], members: [{ member: "pg-1", apiUrl: "http://pg-1:8008", cells: [cell("primary", 200)] }] });
    expect(screen.getByText(/серый — адрес не относится к роли участника/)).toBeInTheDocument();
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
