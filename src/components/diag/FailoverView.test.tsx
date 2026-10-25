import { screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Candidate, DynConfig, FailoverData, FailoverReason } from "../../api/types";
import { diagReport } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";
import { FailoverView } from "./FailoverView";

const config = (patch: Partial<DynConfig> = {}): DynConfig => ({ maximumLagOnFailover: 1048576, synchronousMode: "off", synchronousModeStrict: false, synchronousNodeCount: null, ttl: 30, loopWait: 10, retryTimeout: 10, permanentSlots: [], ...patch });
const candidate = (patch: Partial<Candidate> = {}): Candidate => ({ member: "pg-2", ready: true, reasons: [], lagBytes: 0, pendingRestart: false, ...patch });
const data = (patch: Partial<FailoverData> = {}): FailoverData & { kind: "failover" } => ({ kind: "failover", paused: false, readyCount: 1, config: config(), candidates: [candidate()], ...patch });
const render = (d: FailoverData & { kind: "failover" }) => renderWithClient(<FailoverView connectionId="c" report={diagReport(d, { member: null })} />);

describe("FailoverView", () => {
  it("вердикт по кандидату с причинами и сводка конфигурации", () => {
    const data = { kind: "failover" as const, paused: false, readyCount: 1,
      config: { maximumLagOnFailover: 1048576, synchronousMode: "off" as const, synchronousModeStrict: false, synchronousNodeCount: null, ttl: 30, loopWait: 10, retryTimeout: 10, permanentSlots: [] },
      candidates: [{ member: "pg-2", ready: true, reasons: [], lagBytes: 0, pendingRestart: false }, { member: "pg-3", ready: false, reasons: ["lagTooHigh" as const, "noFailoverTag" as const], lagBytes: 5000000, pendingRestart: true }] };
    renderWithClient(<FailoverView connectionId="c" report={diagReport(data, { member: null })} />);
    expect(screen.getByRole("row", { name: /pg-2/ })).toHaveTextContent("готов");
    const bad = screen.getByRole("row", { name: /pg-3/ });
    expect(bad).toHaveTextContent("лаг больше maximum_lag_on_failover");
    expect(bad).toHaveTextContent("тег nofailover");
    expect(bad).toHaveTextContent("ждёт рестарта");
    expect(screen.getByText(/maximum_lag_on_failover: 1,0 MB/)).toBeInTheDocument();
  });

  it("не готовый кандидат назван «не готов», готовый — «готов» (не «не готов»); лаг — в байтах", () => {
    render(data({ candidates: [candidate({ member: "pg-2", lagBytes: 2048 }), candidate({ member: "pg-3", ready: false, reasons: ["unreachable"], lagBytes: null })] }));
    const ok = screen.getByRole("row", { name: /pg-2/ });
    expect(ok).toHaveTextContent(/^pg-2\s*готов/);
    expect(ok).toHaveTextContent("2,0 KB");
    const bad = screen.getByRole("row", { name: /pg-3/ });
    expect(bad).toHaveTextContent("не готов");
    expect(bad).toHaveTextContent("—");
  });

  it("каждая причина имеет свой текст", () => {
    const texts: Record<FailoverReason, string> = {
      unreachable: "недоступен",
      notStreaming: "не streaming",
      lagTooHigh: "лаг больше maximum_lag_on_failover",
      lagUnknown: "лаг неизвестен",
      noFailoverTag: "тег nofailover",
      zeroPriority: "failover_priority = 0",
      notSynchronous: "не синхронная реплика",
    };
    for (const [reason, text] of Object.entries(texts)) {
      const { unmount } = render(data({ candidates: [candidate({ member: "pg-9", ready: false, reasons: [reason as FailoverReason] })] }));
      expect(screen.getByRole("row", { name: /pg-9/ })).toHaveTextContent(text);
      unmount();
    }
  });

  it("сводка: синхронный режим и strict, число синхронных узлов, тайминги, постоянные слоты, число готовых", () => {
    render(data({ readyCount: 1, config: config({ synchronousMode: "quorum", synchronousModeStrict: true, synchronousNodeCount: 2, ttl: 30, loopWait: 10, retryTimeout: 10, permanentSlots: ["cdc", "audit"] }), candidates: [candidate(), candidate({ member: "pg-3", ready: false, reasons: ["unreachable"] })] }));
    expect(screen.getByText("synchronous_mode: quorum")).toBeInTheDocument();
    expect(screen.getByText("synchronous_mode_strict: on")).toBeInTheDocument();
    expect(screen.getByText("synchronous_node_count: 2")).toBeInTheDocument();
    expect(screen.getByText("ttl / loop_wait / retry_timeout: 30 с / 10 с / 10 с")).toBeInTheDocument();
    expect(screen.getByText("Постоянные слоты: cdc, audit")).toBeInTheDocument();
    expect(screen.getByText("Готовы принять роль лидера: 1 из 2")).toBeInTheDocument();
  });

  it("неизвестные тайминги — прочерки; число синхронных узлов и слоты, которых нет, — без лишних строк", () => {
    render(data({ config: config({ ttl: null, loopWait: null, retryTimeout: null }) }));
    expect(screen.getByText("ttl / loop_wait / retry_timeout: — / — / —")).toBeInTheDocument();
    expect(screen.queryByText(/synchronous_node_count/)).not.toBeInTheDocument();
    expect(screen.getByText("Постоянные слоты: —")).toBeInTheDocument();
  });

  it("кластер на паузе — предупреждение, что автоматического failover нет", () => {
    const { rerender } = render(data({ paused: true }));
    expect(screen.getByText(/Кластер на паузе/)).toBeInTheDocument();
    rerender(<FailoverView connectionId="c" report={diagReport(data({ paused: false }), { member: null })} />);
    expect(screen.queryByText(/Кластер на паузе/)).not.toBeInTheDocument();
  });

  it("кандидатов нет — сообщение вместо таблицы", () => {
    render(data({ candidates: [], readyCount: 0 }));
    expect(screen.getByText("Других участников, кроме лидера, нет")).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("чужой вид отчёта не рисуется", () => {
    const { container } = renderWithClient(<FailoverView connectionId="c" report={diagReport({ kind: "health", endpoints: [], members: [] })} />);
    expect(container).toBeEmptyDOMElement();
  });
});
