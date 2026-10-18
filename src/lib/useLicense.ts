import { useQuery } from "@tanstack/react-query";
import { api } from "../api/client";
import { i18n } from "../i18n";

export const proHint = () => i18n.t("common.proHint");

/** locked — статус известен и тариф не Pro. Пока статус не загружен, интерфейс не блокируется: проверку делает Rust. */
export function useLicense() {
  const q = useQuery({ queryKey: ["license"], queryFn: api.getLicenseStatus, staleTime: 60_000 });
  const status = q.data;
  return { status, locked: status !== undefined && status.tier !== "pro" };
}
