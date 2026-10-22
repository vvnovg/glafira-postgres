import { test } from "node:test";
import assert from "node:assert/strict";
import { C_LIBRARIES, npmRows, render, rustRows } from "./third-party.mjs";

const pkg = (id, name, version, extra = {}) => ({ id, name, version, source: "registry+https://github.com/rust-lang/crates.io-index", license: "MIT", license_file: null, repository: `https://example.org/${name}`, ...extra });
const metadata = {
  packages: [
    pkg("app", "glafira-pg", "0.1.0", { source: null, license: "Apache-2.0" }),
    pkg("a", "alpha", "1.0.0"),
    pkg("b", "beta", "2.0.0", { license: "MIT OR Apache-2.0" }),
    pkg("c", "gamma", "0.1.0", { license: null, license_file: "LICENSE.txt" }),
    pkg("d", "devonly", "9.9.9"),
    pkg("e", "unused", "0.0.1"),
    pkg("f", "serde_json", "1.0.0"),
    pkg("g", "serde-untagged", "0.1.0"),
    pkg("h", "Inflector", "0.11.0"),
    pkg("l", "pg-advisor", "0.1.0", { source: null, license: "Apache-2.0" }),
  ],
  resolve: {
    root: "app",
    nodes: [
      { id: "app", deps: [{ pkg: "a", dep_kinds: [{ kind: null }] }, { pkg: "b", dep_kinds: [{ kind: "build" }] }, { pkg: "d", dep_kinds: [{ kind: "dev" }] }, { pkg: "f", dep_kinds: [{ kind: null }] }, { pkg: "g", dep_kinds: [{ kind: null }] }, { pkg: "h", dep_kinds: [{ kind: null }] }, { pkg: "l", dep_kinds: [{ kind: null }] }] },
      { id: "a", deps: [{ pkg: "c", dep_kinds: [{ kind: null }] }] },
      { id: "b", deps: [] },
      { id: "c", deps: [] },
      { id: "d", deps: [{ pkg: "e", dep_kinds: [{ kind: null }] }] },
      { id: "e", deps: [] },
      { id: "f", deps: [] },
      { id: "g", deps: [] },
      { id: "h", deps: [] },
      { id: "l", deps: [] },
    ],
  },
};

test("rustRows: обычные и build-зависимости от корня, без dev, без локальных крейтов", () => {
  const rows = rustRows(metadata);
  assert.deepEqual(
    rows.filter((r) => ["alpha", "beta", "gamma"].includes(r.name)).map((r) => `${r.name} ${r.version} ${r.license}`),
    ["alpha 1.0.0 MIT", "beta 2.0.0 MIT OR Apache-2.0", "gamma 0.1.0 see LICENSE.txt in the crate"],
  );
  assert.equal(rows.find((r) => r.name === "alpha").url, "https://example.org/alpha");
  assert.ok(!rows.some((r) => ["glafira-pg", "pg-advisor", "devonly", "unused"].includes(r.name)));
});

test("rustRows: порядок по кодовым точкам, а не по локали («-» раньше «_», заглавные раньше строчных)", () => {
  // localeCompare дал бы alpha, beta, gamma, Inflector, serde_json, serde-untagged.
  assert.deepEqual(
    rustRows(metadata).map((r) => r.name),
    ["Inflector", "alpha", "beta", "gamma", "serde-untagged", "serde_json"],
  );
});

test("npmRows: группы по лицензии разворачиваются в строки по версиям", () => {
  const rows = npmRows({
    MIT: [{ name: "react", versions: ["19.1.0"], homepage: "https://react.dev" }, { name: "i18next", versions: ["25.0.0", "25.1.0"] }],
    "Apache-2.0": [{ name: "zustand", versions: ["5.0.0"] }],
  });
  assert.deepEqual(
    rows.map((r) => `${r.name}@${r.version}:${r.license}`),
    ["i18next@25.0.0:MIT", "i18next@25.1.0:MIT", "react@19.1.0:MIT", "zustand@5.0.0:Apache-2.0"],
  );
  assert.equal(rows[2].url, "https://react.dev");
  assert.equal(rows[0].url, "");
});

test("render: три раздела, счётчики, C-библиотеки и экранирование «|»", () => {
  const md = render({
    rust: [{ name: "a|b", version: "1.0.0", license: "MIT", url: "" }],
    npm: [{ name: "react", version: "19.1.0", license: "MIT", url: "https://react.dev" }],
  });
  assert.match(md, /^# Third-party software/);
  assert.match(md, /Glafira for PostgreSQL is licensed under the Apache License 2\.0/);
  assert.match(md, /## C libraries compiled into the binary/);
  for (const lib of C_LIBRARIES) assert.ok(md.includes(`| ${lib.name} | ${lib.license} |`), lib.name);
  assert.ok(C_LIBRARIES.some((l) => l.name === "OpenSSL 3") && C_LIBRARIES.some((l) => l.name === "AWS-LC"));
  assert.match(md, /## Rust crates \(1\)/);
  assert.ok(md.includes("| a\\|b | 1.0.0 | MIT |"));
  assert.match(md, /## npm packages in the frontend \(1\)/);
  assert.ok(md.includes("| react | 19.1.0 | MIT | https://react.dev |"));
  assert.ok(md.endsWith("\n"));
});
