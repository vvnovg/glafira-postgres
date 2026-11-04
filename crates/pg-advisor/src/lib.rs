//! Правила и разбор без ввода-вывода (концепция §3): тестируются на наборах фактов и фикстурах.

// Ручные фикстуры планов в тестах `plan::*` — крупные литералы `json!`: стандартного предела рекурсии макроса не хватает.
#![cfg_attr(test, recursion_limit = "256")]

pub mod ash;
pub mod failover;
pub mod findings;
pub mod functions;
pub mod locktree;
pub mod pgss;
pub mod plan;
pub mod plan_rules;
pub mod queries;
pub mod rates;
pub mod readiness;
pub mod rules;
pub mod serde_util;
pub mod sqlcheck;
pub mod sqltext;
