import { describe, expect, it } from "vitest";
import { snapshotFileParts } from "./queriesText";

describe("snapshotFileParts", () => {
  it("раскодирует каталог узла и отделяет время", () => {
    expect(snapshotFileParts("db1%3A5432/2025-10-29T11-00-00.json")).toEqual({ member: "db1:5432", stamp: "2025-10-29T11-00-00" });
    expect(snapshotFileParts("%5B%3A%3A1%5D%3A5432/x.json")).toEqual({ member: "[::1]:5432", stamp: "x" });
    expect(snapshotFileParts("my node/x.json")).toEqual({ member: "my node", stamp: "x" });
  });

  it("старый каталог и не код — как есть", () => {
    expect(snapshotFileParts("db1:5432/x.json")).toEqual({ member: "db1:5432", stamp: "x" });
    expect(snapshotFileParts("50%zz/x.json")).toEqual({ member: "50%zz", stamp: "x" });
    expect(snapshotFileParts("x.json")).toEqual({ member: "", stamp: "x" });
  });
});
