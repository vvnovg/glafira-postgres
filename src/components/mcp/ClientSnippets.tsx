import { useQuery } from "@tanstack/react-query";
import { useId } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
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

/**
 * «Подключить клиент» (спец. P4 §11): готовые фрагменты для Qwen Code, Claude Code, Claude Desktop, Cursor и любого
 * stdio-клиента с путём к установленному приложению (Rust, `get_mcp_client_configs`). Названия клиентов и файлов не
 * переводятся (глоссарий), подписи собираются из них и ключей словаря.
 */
export function ClientSnippets() {
  const { t } = useTranslation();
  const configs = useQuery({ queryKey: ["mcpClientConfigs"], queryFn: () => api.getMcpClientConfigs(), staleTime: Infinity });
  if (configs.error) return <ErrorBanner error={configs.error} onRetry={() => void configs.refetch()} />;
  const c = configs.data;
  if (!c) return <p className="empty">{t("common.loading")}</p>;
  const stdio = JSON.stringify({ command: c.command, args: c.args }, null, 2);
  return (
    <>
      <h4>{t("settings.mcp.connect.title")}</h4>
      <p className="muted">{t("settings.mcp.connect.hint")}</p>
      <Snippet label={t("settings.mcp.connect.command", { client: "Qwen Code" })} text={c.qwenCode} />
      <Snippet label={t("settings.mcp.connect.file", { client: "Qwen Code", file: "~/.qwen/settings.json" })} text={c.qwenSettings} />
      <Snippet label={t("settings.mcp.connect.command", { client: "Claude Code" })} text={c.claudeCode} />
      <Snippet label={t("settings.mcp.connect.file", { client: "Claude Desktop", file: "claude_desktop_config.json" })} text={c.claudeDesktop} />
      <Snippet label={t("settings.mcp.connect.file", { client: "Cursor", file: "~/.cursor/mcp.json" })} text={c.cursor} />
      <Snippet label={t("settings.mcp.connect.stdio")} text={stdio} />
    </>
  );
}
