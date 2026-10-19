import type { ConnectionKind, Role } from "../api/types";
import { i18n } from "../i18n";

export const isStandby = (role: Role) => role === "replica" || role === "syncStandby" || role === "quorumStandby";

/** Подпись роли: у кластера Patroni primary — «Leader», у списка узлов — «Primary». */
export function roleLabel(role: Role, kind: ConnectionKind): string {
  if (role === "primary") return i18n.t(kind === "patroni" ? "topology.role.leader" : "topology.role.primary");
  return i18n.t(`topology.role.${role}`);
}
