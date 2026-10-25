import Markdown, { type Components } from "react-markdown";
import { ExternalLink } from "../common/ExternalLink";

const components: Components = {
  a: ({ href, children }) => <ExternalLink href={href}>{children}</ExternalLink>,
};

/** Заметки к обновлению — Markdown из latest.json: без HTML и картинок, https-ссылки — в системный браузер. */
export function ReleaseNotes({ markdown }: { markdown: string }) {
  return (
    <Markdown skipHtml disallowedElements={["img"]} components={components}>
      {markdown}
    </Markdown>
  );
}
