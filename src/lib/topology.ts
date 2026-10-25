import type { ConnectionKind, Member, Role } from "../api/types";
import { i18n } from "../i18n";

export const isStandby = (role: Role) => role === "replica" || role === "syncStandby" || role === "quorumStandby";

/** Подпись роли: у кластера Patroni primary — «Leader», у списка узлов — «Primary». */
export function roleLabel(role: Role, kind: ConnectionKind): string {
  if (role === "primary") return i18n.t(kind === "patroni" ? "topology.role.leader" : "topology.role.primary");
  return i18n.t(`topology.role.${role}`);
}

/**
 * Лидер принимает запись: отвечает на опрос и в состоянии `running`. Условие то же, что `running_leader` в Rust
 * (`actions/patroni.rs`): от него зависят switchover (нужен такой лидер) и failover (при таком лидере отказ).
 * Решает Rust; здесь — чтобы не открывать диалог, который заведомо закончится отказом.
 */
export const isRunningLeader = (reachable: boolean | null | undefined, state: string | null | undefined): boolean => reachable === true && state === "running";

/** Лидер топологии, как `Topology::leader` в Rust: первый primary, иначе первый standby leader. */
export const topologyLeader = (members: Member[]): Member | undefined =>
  members.find((m) => m.role === "primary") ?? members.find((m) => m.role === "standbyLeader");

/** В топологии есть лидер, принимающий запись (см. `isRunningLeader`). */
export const hasRunningLeader = (members: Member[]): boolean => {
  const leader = topologyLeader(members);
  return leader !== undefined && isRunningLeader(leader.reachable, leader.state);
};
