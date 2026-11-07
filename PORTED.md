# Перенесено из других версий Glafira

Файлы этапа U1 «Ревизия интерфейса под macOS» скопированы скриптом `scripts/port.sh` из Glafira for Apache Kafka
(`vvnovg/glafira-kafka`, локально `~/projects/glafira-kafka`) и Glafira for Kubernetes (`vvnovg/glafira-kubernetes`,
локально `~/projects/glafira-openshift`, `SOURCE_REPO=…`). Исправление общей части нужно перенести в другие версии;
строка показывает, откуда взят файл и что в нём поправлено. Спецификация переноса —
`docs/superpowers/specs/2026-11-07-glafira-postgres-u1-ui-revision-design.md`.

Общие файлы этапа P1 (i18n, хранилище, политика, форма подключения), перенесённые до заведения этого списка, здесь
не перечислены: их опись — отдельная задача.

| Здесь | Источник | Коммит | Правки |
|---|---|---|---|
| `src/styles.css` | kafka `src/styles.css` | `6a3fcfe` | как есть; правила и токены PostgreSQL-версии — в `src/app.css` |
| `src/styles.test.ts` | kafka `src/styles.test.ts` | `6a3fcfe` | адаптирован: кегль проверяется и в `app.css`, расширенный набор глифов (`▴ ○ ★ ✓ ↑ ↓ ✅ ⚠ ❌ ⏳ ⏭`, «»» в начале строки разметки) |
| `vitest.config.ts` | kafka `vitest.config.ts` | `6a3fcfe` | как есть |
| `src/components/common/Icon.tsx` | kafka `src/components/common/Icon.tsx` | `6a3fcfe` | адаптирован: значки Kubernetes-версии (`server`, `star`, `clock`, `code`, `logs`, `check`, `error`, `hourglass`, `5bfad06`) и PostgreSQL-версии (каталог, инструменты «Диагностики», `arrowUp`, `arrowDown`, `chevrons`, `skip`) |
| `src/components/common/Icon.test.tsx` | kafka `src/components/common/Icon.test.tsx` | `6a3fcfe` | как есть |
| `src/components/common/Kbd.tsx` | kafka `src/components/common/Kbd.tsx` | `6a3fcfe` | как есть |
| `src/components/common/Kbd.test.tsx` | kafka `src/components/common/Kbd.test.tsx` | `6a3fcfe` | как есть |
| `src/components/common/StateMark.tsx` | kubernetes `src/components/common/StateMark.tsx` | `5bfad06` | адаптирован: состояние `skip` («пропущено», шаг проверки подключения) |
| `src/components/common/EnvBadge.tsx` | kafka `src/components/common/EnvBadge.tsx` | `6a3fcfe` | как есть |
| `src/components/common/Hint.tsx` | kafka `src/components/common/Hint.tsx` | `6a3fcfe` | правка U1 (значок `help`) наложена на версию PostgreSQL: параметр `values` остаётся |
| `src/components/about/UpdateNotice.tsx` | kafka `src/components/about/UpdateNotice.tsx` | `6a3fcfe` | правка U1 (значок `close`) наложена; комментарий PostgreSQL-версии («выключено» тоже молча) |
| `src/components/connections/tree/TreeRow.tsx` | kafka `src/components/connections/tree/TreeRow.tsx` | `6a3fcfe` | адаптирован: раздельный режим «шеврон + действие строки», `id` и `buttonRef` у шеврона, `activateLabel`, `activateDescribedBy` — **перенести в Kafka-версию** |
| `src/components/connections/tree/TreeRow.test.tsx` | kafka `src/components/connections/tree/TreeRow.test.tsx` | `6a3fcfe` | адаптирован: тест раздельного режима |
| `src/lib/platform.ts` | kafka `src/lib/platform.ts` | `6a3fcfe` | как есть |
| `src/lib/useResizable.ts` | kafka `src/lib/useResizable.ts` | `6a3fcfe` | как есть |
| `src/lib/env.ts` | kafka `src/lib/env.ts` | `6a3fcfe` | как есть |
| `src/lib/shortcuts.ts` | kafka `src/lib/shortcuts.ts` | `6a3fcfe` | адаптирован: действие `run` (⌘⏎), `[data-action]` только среди видимых (не внутри `[hidden]`) — **перенести в Kafka-версию**, «Настройки» не сбрасывают раздел |
| `src/lib/shortcuts.test.tsx` | kafka `src/lib/shortcuts.test.tsx` | `6a3fcfe` | адаптирован: ⌘⏎, скрытые разделы, раздел «Настроек» |
| `src/lib/useAppMenu.ts` | kafka `src/lib/useAppMenu.ts` | `6a3fcfe` | адаптирован: «Вид → Запустить» (⌘⏎), «Справка» PostgreSQL-версии (руководство по экрану, без «Решения проблем») |
| `src/lib/useAppMenu.test.ts` | kafka `src/lib/useAppMenu.test.ts` | `6a3fcfe` | адаптирован: пункты «Вида» и «Справки», руководство по экрану, «Запустить» |
| `src-tauri/tauri.conf.json` | kafka `src-tauri/tauri.conf.json` | `6a3fcfe` | правка U1 (`titleBarStyle`, `hiddenTitle`) наложена |
| `src-tauri/capabilities/default.json` | kafka `src-tauri/capabilities/default.json` | `6a3fcfe` | правка U1 (три разрешения окна) наложена |
| `src-tauri/Cargo.toml`, `src-tauri/src/lib.rs` | kafka, те же файлы | `6a3fcfe` | правка U1 (`tauri-plugin-window-state`) наложена |

Своё у PostgreSQL-версии, что стоит перенести в Kafka-версию (в `src/app.css`, пометка «перенести в Kafka-версию»):
`--link` — цвет ссылок с контрастом 4,5:1 (акцент `#007aff` текстом даёт 4,0:1 на белом), правило `.tree-extra`
(дополнительный текст в узкой панели скрывается целиком, а не обрезается), стили раздельного режима `TreeRow`.
