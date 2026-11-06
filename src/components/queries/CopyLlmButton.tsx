import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../api/client";
import type { AppError, LlmSource, PlanTree } from "../../api/types";
import { tr } from "../../i18n/text";
import { llmMarkdown } from "../../lib/llmMarkdown";
import { Hint } from "../common/Hint";

/** Сколько держится итог «Скопировано» / «Не удалось скопировать», мс (как у `CopyButton`). */
const RESULT_MS = 2500;

/**
 * «Скопировать для LLM» (спец. P3 §7.4): данные собирает Rust (`query_llm_bundle`), markdown — здесь, на языке
 * интерфейса, затем буфер обмена. Ошибка сборки (подключение, права) остаётся видна до следующей попытки; итог
 * копирования — как у `CopyButton`. Значок «?» отличает её от «Передать LLM-ассистенту»: снимок текстом для любого чата,
 * без MCP, ответ в Glafira не вернётся.
 */
export function CopyLlmButton({ connectionId, member, database, source, plan }: { connectionId: string; member: string; database: string; source: LlmSource; plan: PlanTree | null }) {
  const { t } = useTranslation();
  const [state, setState] = useState<"idle" | "busy" | "copied" | "failed">("idle");
  const [error, setError] = useState<AppError | null>(null);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const settle = (next: "copied" | "failed") => {
    setState(next);
    timer.current = window.setTimeout(() => setState("idle"), RESULT_MS);
  };
  const copy = async () => {
    window.clearTimeout(timer.current);
    setState("busy");
    setError(null);
    let markdown: string;
    try {
      markdown = llmMarkdown(await api.queryLlmBundle(connectionId, member, database, source, plan), t);
    } catch (e) {
      setError(toAppError(e));
      setState("failed");
      return;
    }
    try {
      if (!navigator.clipboard) throw new Error("clipboard unavailable");
      await navigator.clipboard.writeText(markdown);
      settle("copied");
    } catch {
      settle("failed");
    }
  };
  const status = state === "copied" ? t("test.copied") : state === "failed" ? (error ? tr(error) : t("test.copyFailed")) : "";
  return (
    <>
      <button type="button" disabled={state === "busy"} onClick={() => void copy()}>
        {t("diag.query.copyLlm")}
      </button>
      <Hint k="hints.llmAssist.copy" label={t("diag.query.copyLlm")} />
      <span role="status" className="muted">
        {status}
      </span>
    </>
  );
}
