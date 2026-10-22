import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn().mockResolvedValue(undefined) }));

import { openUrl } from "@tauri-apps/plugin-opener";
import { LICENSE_URL, SUPPORT_URL, openLicense, openSupport } from "./links";

describe("links", () => {
  it("адреса зафиксированы спекой", () => {
    expect(SUPPORT_URL).toBe("https://github.com/vvnovg/glafira-postgres-releases/blob/main/SUPPORT.md");
    expect(LICENSE_URL).toBe("https://github.com/vvnovg/glafira-postgres/blob/main/LICENSE");
  });

  it("openSupport и openLicense открывают свои адреса в системном браузере", () => {
    openSupport();
    expect(openUrl).toHaveBeenLastCalledWith(SUPPORT_URL);
    openLicense();
    expect(openUrl).toHaveBeenLastCalledWith(LICENSE_URL);
  });
});
