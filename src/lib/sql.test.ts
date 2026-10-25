import { describe, expect, it } from "vitest";
import { quoteLiteral } from "./sql";

describe("quoteLiteral", () => {
  it("берёт в апострофы и удваивает апострофы внутри", () => {
    expect(quoteLiteral("gid-1")).toBe("'gid-1'");
    expect(quoteLiteral("it's")).toBe("'it''s'");
    expect(quoteLiteral("''")).toBe("''''''");
    expect(quoteLiteral("")).toBe("''");
  });

  it("обратная косая черта остаётся как есть: при standard_conforming_strings = on она не служебная", () => {
    expect(quoteLiteral("a\\b")).toBe("'a\\b'");
  });
});
