import { screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Receiver, ReplicationData } from "../../api/types";
import { diagReport, replication, sender } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";
import { ReplicationView } from "./ReplicationView";

const report = (patch: Partial<ReplicationData>) => diagReport({ kind: "replication", ...replication(patch) }, { inRecovery: patch.inRecovery ?? false });
const receiver = (patch: Partial<Receiver> = {}): Receiver => ({ status: "streaming", senderHost: "pg-1", senderPort: 5432, slotName: null, receivedTli: 1, lastMsgAgeS: 0, latestEndAgeS: 0, ...patch });

describe("ReplicationView", () => {
  it("на primary — отправители с лагами, на реплике — карточка приёмника и пауза", () => {
    const { rerender } = renderWithClient(<ReplicationView connectionId="c" report={diagReport({ kind: "replication", ...replication({ senders: [sender({ application: "pg-2", member: "pg-2", replayLagBytes: 20 * 1024 * 1024, replayLagS: 3 })] }) })} />);
    expect(screen.getByRole("row", { name: /pg-2/ })).toHaveTextContent("20,0 MB");
    rerender(<ReplicationView connectionId="c" report={diagReport({ kind: "replication", ...replication({ inRecovery: true, receiver: { status: "streaming", senderHost: "pg-1", senderPort: 5432, slotName: "pg_2", receivedTli: 3, lastMsgAgeS: 1, latestEndAgeS: 1 }, replay: { receiveReplayGapBytes: 0, replayDelayS: 0, pause: "paused" } }) }, { inRecovery: true })} />);
    const card = screen.getByRole("region", { name: "Приёмник WAL" });
    expect(card).toHaveTextContent("pg-1:5432");
    expect(card).toHaveTextContent("на паузе");
  });

  it("отправитель: узел (или приложение), клиент, состояние, sync, лаги по стадиям и во времени, слот", () => {
    renderWithClient(
      <ReplicationView
        connectionId="c"
        report={report({
          senders: [
            sender({
              pid: 7,
              application: "pg-2",
              member: "pg-2",
              clientAddr: "10.0.0.2",
              state: "streaming",
              syncState: "sync",
              sentLagBytes: 1024,
              writeLagBytes: 2048,
              flushLagBytes: 3 * 1024,
              replayLagBytes: 5 * 1024 * 1024,
              replayLagS: 125,
              replyAgeS: 2,
              slotName: "pg_2",
              xminAge: 77,
            }),
            sender({ pid: 8, application: "backup", member: null, state: "catchup", syncState: "async" }),
          ],
        })}
      />,
    );
    const row = screen.getByRole("row", { name: /pg-2/ });
    for (const text of ["10.0.0.2", "streaming", "sync", "1,0 KB", "2,0 KB", "3,0 KB", "5,0 MB", "2 мин 5 с", "2 с", "pg_2", "77"]) expect(row).toHaveTextContent(text);
    // Не сопоставился с участником — показывается имя приложения.
    expect(screen.getByRole("row", { name: /backup/ })).toHaveTextContent("catchup");
  });

  it("лаги, которых нет (null), — прочерк, а не ноль", () => {
    renderWithClient(<ReplicationView connectionId="c" report={report({ senders: [sender({ application: "pg-2", sentLagBytes: null, writeLagBytes: null, flushLagBytes: null, replayLagBytes: null, replayLagS: null, replyAgeS: null })] })} />);
    const cells = within(screen.getByRole("row", { name: /pg-2/ })).getAllByRole("cell");
    expect(cells.filter((c) => c.textContent === "—").length).toBeGreaterThanOrEqual(6);
    expect(screen.getByRole("row", { name: /pg-2/ })).not.toHaveTextContent("0 B");
  });

  it("настройки синхронной репликации; primary без отправителей — сообщение", () => {
    const { rerender } = renderWithClient(<ReplicationView connectionId="c" report={report({ synchronousStandbyNames: "ANY 1 (pg_2)", synchronousCommit: "remote_apply" })} />);
    expect(screen.getByText("synchronous_standby_names: ANY 1 (pg_2)")).toBeInTheDocument();
    expect(screen.getByText("synchronous_commit: remote_apply")).toBeInTheDocument();
    expect(screen.getByText("К этому узлу не подключены реплики")).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
    // Пустое значение настройки — прочерк.
    rerender(<ReplicationView connectionId="c" report={report({ synchronousStandbyNames: "", synchronousCommit: "on" })} />);
    expect(screen.getByText("synchronous_standby_names: —")).toBeInTheDocument();
  });

  it("реплика без каскада: таблицы отправителей нет, только карточка приёмника", () => {
    renderWithClient(<ReplicationView connectionId="c" report={report({ inRecovery: true, receiver: receiver(), replay: { receiveReplayGapBytes: 0, replayDelayS: 0, pause: "not paused" } })} />);
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
    expect(screen.queryByText("К этому узлу не подключены реплики")).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Приёмник WAL" })).toHaveTextContent("не на паузе");
  });

  it("карточка приёмника: статус, источник, слот, таймлайн, сообщения, разрыв, задержка, пауза, настройки", () => {
    renderWithClient(
      <ReplicationView
        connectionId="c"
        report={report({
          inRecovery: true,
          hotStandbyFeedback: true,
          maxStandbyStreamingDelayMs: 30_000,
          receiver: receiver({ slotName: "pg_2", receivedTli: 3, lastMsgAgeS: 4, latestEndAgeS: 65 }),
          replay: { receiveReplayGapBytes: 2 * 1024 * 1024, replayDelayS: 12, pause: "pause requested" },
        })}
      />,
    );
    const card = screen.getByRole("region", { name: "Приёмник WAL" });
    // Подпись и значение сверяются целиком: подстрока «3» нашлась бы и в «30 с».
    const fields = Object.fromEntries([...card.querySelectorAll("dl > div")].map((d) => [d.querySelector("dt")?.textContent, d.querySelector("dd")?.textContent]));
    expect(fields).toMatchObject({
      Статус: "streaming",
      Источник: "pg-1:5432",
      Слот: "pg_2",
      Таймлайн: "3",
      "Последнее сообщение": "4 с",
      "Последний отчёт о позиции": "1 мин 5 с",
      "Разрыв receive и replay": "2,0 MB",
      "Задержка replay": "12 с",
      "Применение WAL": "запрошена пауза",
    });
    expect(within(card).getByText("hot_standby_feedback = on")).toBeInTheDocument();
    expect(within(card).getByText("max_standby_streaming_delay: 30 с")).toBeInTheDocument();
  });

  it("max_standby_streaming_delay: -1 — без ограничения, доли секунды — в миллисекундах", () => {
    const base = { inRecovery: true, receiver: receiver(), replay: { receiveReplayGapBytes: 0, replayDelayS: 0, pause: "not paused" } };
    const { rerender } = renderWithClient(<ReplicationView connectionId="c" report={report({ ...base, maxStandbyStreamingDelayMs: -1 })} />);
    expect(screen.getByRole("region", { name: "Приёмник WAL" })).toHaveTextContent("max_standby_streaming_delay: без ограничения");
    rerender(<ReplicationView connectionId="c" report={report({ ...base, maxStandbyStreamingDelayMs: 500 })} />);
    expect(screen.getByRole("region", { name: "Приёмник WAL" })).toHaveTextContent("max_standby_streaming_delay: 500 ms");
  });

  it("приёмник не работает (нет статуса) — об этом сказано, а не пустая карточка", () => {
    renderWithClient(<ReplicationView connectionId="c" report={report({ inRecovery: true, receiver: receiver({ status: null, senderHost: null, senderPort: null, receivedTli: null, lastMsgAgeS: null, latestEndAgeS: null }), replay: { receiveReplayGapBytes: null, replayDelayS: null, pause: "not paused" } })} />);
    expect(screen.getByRole("region", { name: "Приёмник WAL" })).toHaveTextContent("Приёмник WAL не запущен");
  });

  it("конфликты запросов по видам и их рост с прошлого опроса", () => {
    const conflicts = { tablespace: 0, lock: 1, snapshot: 12, bufferpin: 0, deadlock: 0, activeLogicalslot: null, growth: 3 };
    const base = { inRecovery: true, receiver: receiver(), replay: { receiveReplayGapBytes: 0, replayDelayS: 0, pause: "not paused" } };
    const { rerender } = renderWithClient(<ReplicationView connectionId="c" report={report({ ...base, conflicts })} />);
    const card = screen.getByRole("region", { name: "Приёмник WAL" });
    expect(card).toHaveTextContent("snapshot: 12");
    expect(card).toHaveTextContent("lock: 1");
    // Вид, которого в версии сервера нет (activeLogicalslot — PostgreSQL 16+), не показывается.
    expect(card).not.toHaveTextContent("activeLogicalslot");
    expect(card).toHaveTextContent("+3");
    rerender(<ReplicationView connectionId="c" report={report({ ...base, conflicts: { ...conflicts, activeLogicalslot: 2, growth: null } })} />);
    expect(screen.getByRole("region", { name: "Приёмник WAL" })).toHaveTextContent("activeLogicalslot: 2");
    expect(screen.getByRole("region", { name: "Приёмник WAL" })).not.toHaveTextContent("+");
  });

  it("роль без pg_monitor: строки отправителей не показаны пустыми репликами — пояснение и число подключений", () => {
    renderWithClient(<ReplicationView connectionId="c" report={report({ detailsHidden: true, senders: [sender({ pid: 11, user: null, application: null, state: null }), sender({ pid: 12, user: null, application: null, state: null })] })} />);
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
    expect(screen.getByText(/Сведения о репликах скрыты/)).toBeInTheDocument();
    expect(screen.getByText("Подключено отправителей WAL: 2")).toBeInTheDocument();
    expect(screen.queryByText("К этому узлу не подключены реплики")).not.toBeInTheDocument();
  });

  it("роль без pg_monitor на реплике: в карточке приёмника только известное, без прочерков вместо скрытого", () => {
    renderWithClient(
      <ReplicationView
        connectionId="c"
        report={report({
          detailsHidden: true,
          inRecovery: true,
          receiver: receiver({ status: null, senderHost: null, senderPort: null, slotName: null, receivedTli: null, lastMsgAgeS: null, latestEndAgeS: null }),
          replay: { receiveReplayGapBytes: 1024, replayDelayS: 2, pause: "not paused" },
        })}
      />,
    );
    const card = screen.getByRole("region", { name: "Приёмник WAL" });
    expect(card).toHaveTextContent("1,0 KB");
    expect(card).not.toHaveTextContent("—");
    expect(card).not.toHaveTextContent("не запущен");
    expect(screen.getByText(/Сведения о репликах скрыты/)).toBeInTheDocument();
  });

  it("чужой вид отчёта не рисуется", () => {
    const { container } = renderWithClient(<ReplicationView connectionId="c" report={diagReport({ kind: "health", endpoints: [], members: [] })} />);
    expect(container).toBeEmptyDOMElement();
  });
});
