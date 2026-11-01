import { describe, expect, it } from "vitest";
import type { LlmBundle } from "../api/types";
import { i18n } from "../i18n";
import { llmMarkdown } from "./llmMarkdown";

const bundle: LlmBundle = {
  serverVersionNum: 170_004, pgssVersion: "1.11", inRecovery: false, member: "pg-1", database: "perf",
  text: "SELECT * FROM shop.customers WHERE id = $1", textTruncated: false,
  stats: { sinceReset: { key: { queryId: "1", dbid: 1, userid: 1, toplevel: true }, datname: "perf", rolname: "app", calls: 1000, totalExecMs: 500, meanExecMs: 0.5, minExecMs: 0.1, maxExecMs: 3, stddevExecMs: 0.2, rows: 1000, rowsPerCall: 1, plans: 0, totalPlanMs: 0, sharedHit: 4000, sharedRead: 10, sharedDirtied: 0, sharedWritten: 0, localHit: 0, localRead: 0, localDirtied: 0, localWritten: 0, tempRead: 0, tempWritten: 0, ioReadMs: 0, ioWriteMs: 0, tempIoMs: null, walRecords: 0, walFpi: 0, walBytes: 0, jitFunctions: null, jitMs: null, parallelPlanned: null, parallelLaunched: null, sharePct: 12.5, hitRatio: 0.9975, text: null, textTruncated: false, own: false }, window3600: null },
  planText: "Index Scan using customers_pkey on shop.customers  (cost=0.42..8.44 rows=1 width=40)\n", tables: [{ schema: "shop", name: "customers", ddl: "CREATE TABLE shop.customers (\n    id bigint,\n    CONSTRAINT customers_pkey PRIMARY KEY (id)\n);\n\nCREATE INDEX customers_email_idx ON shop.customers USING btree (email);\n" }], tablesOmitted: 2, warnings: ["paramsNeeded"],
};

describe("llmMarkdown", () => {
  it("разделы на языке интерфейса, статистика таблицей, план и DDL в блоках кода, предупреждения и «ещё N таблиц»", () => {
    const md = llmMarkdown(bundle, i18n.t.bind(i18n));
    expect(md).toContain("## Среда");
    expect(md).toContain("PostgreSQL 17.4");
    expect(md).toContain("pg_stat_statements 1.11");
    expect(md).toContain("```sql\nSELECT * FROM shop.customers WHERE id = $1\n```");
    expect(md).toContain("| Вызовов | 1 000 |");
    expect(md).toContain("## План\n\n```\nIndex Scan using customers_pkey");
    // DDL каталога как есть: индексы уже в нём, каждый один раз; индекса PRIMARY KEY отдельной строкой нет.
    expect(md).toContain("### shop.customers\n\n```sql\nCREATE TABLE shop.customers (\n    id bigint,\n    CONSTRAINT customers_pkey PRIMARY KEY (id)\n);\n\nCREATE INDEX customers_email_idx ON shop.customers USING btree (email);\n```");
    expect(md.match(/CREATE INDEX customers_email_idx/g)).toHaveLength(1);
    expect(md).not.toContain("CREATE UNIQUE INDEX customers_pkey");
    expect(md).toContain("_… ещё 2 таблицы (сверх лимита или DDL не прочитан)_");
    expect(md).toContain("план требует значений параметров");
    // Планирование выключено (track_planning = off): строки «Планирование» с нулём нет — ноль читался бы как факт.
    expect(md).not.toContain("| Планирование |");
    expect(llmMarkdown({ ...bundle, tablesOmitted: 5 }, i18n.t.bind(i18n))).toContain("_… ещё 5 таблиц (сверх лимита или DDL не прочитан)_");
    expect(md).not.toMatch(/\{\{/);
  });

  it("каждый ключ предупреждения переведён; усечённый текст объясняет отсутствие плана; без статистики и плана — без их разделов", () => {
    const md = llmMarkdown(
      { ...bundle, inRecovery: true, pgssVersion: null, textTruncated: true, stats: null, planText: null, tables: [], tablesOmitted: 1, warnings: ["paramsNeeded", "queryGone", "planFailed", "ddlFailed"] },
      i18n.t.bind(i18n),
    );
    expect(md).toContain("PostgreSQL 17.4 (реплика)");
    expect(md).not.toContain("pg_stat_statements 1.");
    expect(md).toContain("Текст обрезан до 16 384 символов");
    expect(md).not.toContain("## Статистика");
    expect(md).not.toContain("## План");
    expect(md).toContain("… ещё 1 таблица");
    for (const w of ["paramsNeeded", "queryGone", "planFailed", "ddlFailed"]) expect(md).not.toContain(w);
    expect(md).toContain("DDL части таблиц прочитать не удалось");
    expect(md).not.toMatch(/\{\{| /);
  });

  it("окно — «до 60 мин за время работы сэмплера»; планирование печатается, когда оно было", () => {
    const window3600 = { ...bundle.stats!.sinceReset!, plans: 10, totalPlanMs: 2.5, sharedRead: 2, tempWritten: 1 };
    const md = llmMarkdown({ ...bundle, stats: { sinceReset: null, window3600 } }, i18n.t.bind(i18n));
    expect(md).toContain("### Окно до 60 мин (за время работы сэмплера)");
    expect(md).toContain("| Планирование | 2,50 мс |");
    expect(md).toContain("| Прочитано с диска | 16,0 KB |");
    expect(md).toContain("| Temp записано | 8,0 KB |");
  });

  it("неизвестный ключ предупреждения — как есть, без ключа словаря в тексте", () => {
    const md = llmMarkdown({ ...bundle, warnings: ["somethingNew"] }, i18n.t.bind(i18n));
    expect(md).toContain("- somethingNew");
    expect(md).not.toContain("llm.warnings");
  });
});
