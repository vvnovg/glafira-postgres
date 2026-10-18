import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api/client";
import { setLanguage } from "../i18n";

/** Применяет язык интерфейса, выбранный в Rust (от лицензии не зависит). */
export function useLanguageSync() {
  const q = useQuery({ queryKey: ["language"], queryFn: api.getLanguageSettings });
  useEffect(() => {
    if (q.data) void setLanguage(q.data.effective);
  }, [q.data]);
}
