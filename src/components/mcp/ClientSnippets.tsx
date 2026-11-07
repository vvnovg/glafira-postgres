import { useQuery } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { McpClientConfigs } from "../../api/types";
import { CopyButton } from "../common/CopyButton";
import { ErrorBanner } from "../common/ErrorBanner";

/** Фрагмент конфигурации: подпись, текст как есть и «Копировать» (имя кнопки называет фрагмент). */
function Snippet({ label, text }: { label: string; text: string }) {
  const { t } = useTranslation();
  const id = useId();
  return (
    <div className="mcp-snippet">
      <div className="toolbar">
        <strong id={id}>{label}</strong>
        <CopyButton text={text} label={t("settings.mcp.connect.copy", { name: label })} />
      </div>
      <pre aria-labelledby={id}>{text}</pre>
    </div>
  );
}

type Client = "qwen" | "claudeCode" | "claudeDesktop" | "cursor" | "stdio";
const CLIENTS: Client[] = ["qwen", "claudeCode", "claudeDesktop", "cursor", "stdio"];

/** Название клиента в выборе: названия программ не переводятся (глоссарий), «Другой stdio-клиент» — из словаря. */
function clientName(c: Client, t: TFunction): string {
  switch (c) {
    case "qwen":
      return "Qwen Code";
    case "claudeCode":
      return "Claude Code";
    case "claudeDesktop":
      return "Claude Desktop";
    case "cursor":
      return "Cursor";
    case "stdio":
      return t("settings.mcp.connect.other");
  }
}

/** Фрагменты клиента: у Qwen Code — команда и файл `~/.qwen/settings.json`, у остальных — один. */
function snippets(c: Client, cfg: McpClientConfigs, t: TFunction): { label: string; text: string }[] {
  switch (c) {
    case "qwen":
      return [
        { label: t("settings.mcp.connect.command", { client: "Qwen Code" }), text: cfg.qwenCode },
        { label: t("settings.mcp.connect.file", { client: "Qwen Code", file: "~/.qwen/settings.json" }), text: cfg.qwenSettings },
      ];
    case "claudeCode":
      return [{ label: t("settings.mcp.connect.command", { client: "Claude Code" }), text: cfg.claudeCode }];
    case "claudeDesktop":
      return [{ label: t("settings.mcp.connect.file", { client: "Claude Desktop", file: "claude_desktop_config.json" }), text: cfg.claudeDesktop }];
    case "cursor":
      return [{ label: t("settings.mcp.connect.file", { client: "Cursor", file: "~/.cursor/mcp.json" }), text: cfg.cursor }];
    case "stdio":
      return [{ label: t("settings.mcp.connect.stdio"), text: JSON.stringify({ command: cfg.command, args: cfg.args }, null, 2) }];
  }
}

/**
 * «Подключить клиент» (спец. P4 §11, смоук P4c R9): выбор клиента — Qwen Code, Claude Code, Claude Desktop, Cursor или
 * другой stdio-клиент — и готовые фрагменты только для него, с путём к установленному приложению (Rust,
 * `get_mcp_client_configs`). Выбор живёт, пока открыта вкладка «Настроек» (разделы остаются смонтированными). Названия
 * клиентов и файлов не переводятся (глоссарий), подписи собираются из них и ключей словаря.
 */
export function ClientSnippets() {
  const { t } = useTranslation();
  const titleId = useId();
  const name = useId();
  const [client, setClient] = useState<Client>("qwen");
  const configs = useQuery({ queryKey: ["mcpClientConfigs"], queryFn: () => api.getMcpClientConfigs(), staleTime: Infinity });
  const c = configs.data;
  return (
    <>
      <h4 id={titleId}>{t("settings.mcp.connect.title")}</h4>
      <p className="muted">{t("settings.mcp.connect.hint")}</p>
      <div role="radiogroup" aria-labelledby={titleId} className="mcp-clients">
        {CLIENTS.map((x) => (
          <label key={x} className="inline">
            <input type="radio" name={name} value={x} checked={client === x} onChange={() => setClient(x)} />
            {clientName(x, t)}
          </label>
        ))}
      </div>
      {configs.error ? (
        <ErrorBanner error={configs.error} onRetry={() => void configs.refetch()} />
      ) : !c ? (
        <p className="muted">{t("common.loading")}</p>
      ) : (
        snippets(client, c, t).map((s) => <Snippet key={s.label} label={s.label} text={s.text} />)
      )}
    </>
  );
}
