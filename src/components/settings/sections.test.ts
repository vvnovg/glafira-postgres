import { describe, expect, it } from "vitest";
import { loadGuide } from "../../guide/chapters";
import { parseChapter } from "../../guide/parse";
import { SECTION_HELP, SETTINGS_SECTIONS } from "./sections";

describe("разделы «Настроек»", () => {
  it("F1: «Язык» и «Отображение» — «Начало работы», разделы MCP — глава «LLM-ассистенты (MCP)» со своим разделом", () => {
    expect(SECTION_HELP).toEqual({
      language: { chapter: "getting-started" },
      display: { chapter: "getting-started" },
      mcp: { chapter: "llm-mcp", anchor: "enable" },
      agent: { chapter: "llm-mcp", anchor: "background" },
      sessions: { chapter: "llm-mcp", anchor: "sessions-log" },
      audit: { chapter: "llm-mcp", anchor: "sessions-log" },
    });
  });

  it("якорь F1 каждого раздела есть в своей главе на обоих языках", async () => {
    for (const lang of ["en", "ru"]) {
      const guide = await loadGuide(lang);
      for (const section of SETTINGS_SECTIONS) {
        const { chapter, anchor } = SECTION_HELP[section];
        if (anchor === undefined) continue;
        const anchors = parseChapter(chapter, guide[chapter], (key) => key).sections.map((x) => x.id);
        expect(anchors, `${lang}: якорь ${anchor} раздела ${section} в главе ${chapter}`).toContain(anchor);
      }
    }
  });
});
