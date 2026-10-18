import { describe, expect, it } from "vitest";
import { installPlainInputs } from "./plainInputs";

describe("plainInputs", () => {
  it("отключает автокоррекцию и заглавную букву у полей при фокусе", () => {
    installPlainInputs();
    const input = document.createElement("input");
    const pw = document.createElement("input");
    pw.setAttribute("autocomplete", "new-password");
    document.body.append(input, pw);
    input.focus();
    pw.focus();
    expect(input.getAttribute("autocapitalize")).toBe("off");
    expect(input.getAttribute("autocorrect")).toBe("off");
    expect(input.getAttribute("spellcheck")).toBe("false");
    expect(pw.getAttribute("autocomplete")).toBe("new-password");
  });
});
