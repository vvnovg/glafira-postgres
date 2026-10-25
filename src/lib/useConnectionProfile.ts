import { useQuery } from "@tanstack/react-query";
import { api } from "../api/client";
import type { ConnectionProfile } from "../api/types";

/** Профиль подключения вкладки (тот же запрос `["connections"]`, что у дерева): окружение, имя, read-only. */
export function useConnectionProfile(connectionId: string): ConnectionProfile | undefined {
  const { data } = useQuery({ queryKey: ["connections"], queryFn: api.listConnections });
  return data?.find((c) => c.id === connectionId);
}

/** Действия доступны: профиль известен и не read-only (Rust всё равно проверит, спец. §8.1). */
export function canAct(profile: ConnectionProfile | undefined): profile is ConnectionProfile {
  return profile !== undefined && !profile.readOnly;
}
