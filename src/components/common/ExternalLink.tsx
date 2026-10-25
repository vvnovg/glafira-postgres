import { openUrl } from "@tauri-apps/plugin-opener";
import type { ReactNode } from "react";

/** Ссылка из Markdown (справка, заметки к обновлению): https — в системный браузер, прочие адреса не активны. */
export function ExternalLink({ href, children }: { href?: string; children: ReactNode }) {
  if (!href?.startsWith("https://")) return <span>{children}</span>;
  return (
    <a
      href={href}
      onClick={(e) => {
        e.preventDefault();
        void openUrl(href);
      }}
    >
      {children}
    </a>
  );
}
