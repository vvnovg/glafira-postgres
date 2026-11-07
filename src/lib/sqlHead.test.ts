import { describe, expect, it } from "vitest";
import { differingHead, sameSql } from "./sqlHead";

describe("sqlHead", () => {
  it("тот же текст запроса — без хвостовых «;» и пробелов, как у Rust (`pg_advisor::sqltext::normalize`)", () => {
    expect(sameSql("SELECT 1 ;\n", "  SELECT 1")).toBe(true);
    expect(sameSql("SELECT 1;;", "SELECT 1")).toBe(true);
    expect(sameSql("SELECT 1", "SELECT 2")).toBe(false);
    expect(sameSql("SELECT 1; SELECT 2", "SELECT 1")).toBe(false);
  });

  it("начало текста рядом с другим: первая непустая строка, а если она общая — первая отличающаяся с «…»; не длиннее 80", () => {
    const mine = "SELECT o.id, o.total,\n  o.status\nFROM shop.orders o\nWHERE o.status = 'new'";
    expect(differingHead(mine, "SELECT 1")).toBe("SELECT o.id, o.total,");
    expect(differingHead(mine, "SELECT o.id, o.total,\n  o.status\nFROM shop.orders o\nWHERE o.total > 100")).toBe("… WHERE o.status = 'new'");
    expect(differingHead(mine, mine + "\nLIMIT 5")).toBe("SELECT o.id, o.total,");
    expect(differingHead("\n\n", "SELECT 1")).toBe("");
    expect(differingHead("x".repeat(100), "SELECT 1")).toHaveLength(80);
  });
});
