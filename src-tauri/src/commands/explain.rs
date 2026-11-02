//! Команды планов (спец. P3 §10): EXPLAIN на узле и разбор вставленного плана без сервера.

use pg_advisor::plan::json::from_any;
use pg_advisor::plan::{PlanError, PlanTree};
use pg_advisor::plan_rules::{Recommendation, evaluate};
use serde::Serialize;
use tauri::State;

use crate::error::{AppResult, ErrorCode};
use crate::explain::hypopg::{self, HypoRequest, HypoResult};
use crate::explain::{ExplainRequest, ExplainResult};
use crate::state::AppState;

#[tauri::command]
pub async fn explain(
    state: State<'_, AppState>,
    id: String,
    request: ExplainRequest,
) -> AppResult<ExplainResult> {
    crate::explain::run::run(&state, &id, &request).await
}

/// Проверка гипотезы hypopg: стоимость до и после, использован ли индекс, размер.
#[tauri::command]
pub async fn hypo_check(
    state: State<'_, AppState>,
    id: String,
    request: HypoRequest,
) -> AppResult<HypoResult> {
    hypopg::check(&state, &id, &request).await
}

/// Разобранный вставленный план и рекомендации по нему: правила чистые (дерево → советы), сервер не нужен.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedPlan {
    pub plan: PlanTree,
    pub recommendations: Vec<Recommendation>,
    /// Правила «факт», пропущенные у плана без ANALYZE.
    pub recommendations_skipped: Vec<String>,
}

/// Вставленный план: JSON или текстовый EXPLAIN (psql, auto_explain), формат определяется сам (`from_any`). Без сервера;
/// разбор — вне главного потока (вставка может быть в мегабайты).
#[tauri::command]
pub async fn parse_plan(text: String) -> AppResult<ParsedPlan> {
    super::blocking(move || parse_pasted(&text)).await
}

fn parse_pasted(text: &str) -> AppResult<ParsedPlan> {
    let plan = parse_plan_text(text)?;
    let report = evaluate(&plan);
    Ok(ParsedPlan {
        plan,
        recommendations: report.recommendations,
        recommendations_skipped: report.skipped,
    })
}

fn parse_plan_text(text: &str) -> AppResult<PlanTree> {
    let unparsed = |e: PlanError| {
        app_err!(
            ErrorCode::InvalidInput,
            "errors.plan.unparsed",
            "The plan could not be parsed (line {{line}}): {{message}}",
            line = e.line.unwrap_or(1),
            message = e.message
        )
    };
    from_any(text).map_err(unparsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_and_text_plans_are_both_accepted() {
        let json = "[{\"Plan\": {\"Node Type\": \"Result\", \"Startup Cost\": 0, \"Total Cost\": 0.01, \"Plan Rows\": 1, \"Plan Width\": 4}}]";
        assert_eq!(parse_plan_text(json).unwrap().root.node_type, "Result");
        let text = " QUERY PLAN\n------------\n Limit  (cost=0.00..0.10 rows=1 width=4)\n   ->  Seq Scan on orders  (cost=0.00..5.00 rows=50 width=4)\n(2 rows)\n";
        let t = parse_plan_text(text).unwrap();
        assert_eq!(
            t.root.children[0]
                .relation
                .as_ref()
                .map(|r| r.name.as_str()),
            Some("orders")
        );
        // Ошибка — с номером строки исходного текста.
        let e = parse_plan_text(
            "Limit  (cost=0.00..0.10 rows=1 width=4)\n  ->  Seq Scan on t  (cost=bad)\n",
        )
        .unwrap_err();
        assert_eq!(
            (e.key.as_str(), e.params.get("line").map(String::as_str)),
            ("errors.plan.unparsed", Some("2"))
        );
        let e = parse_plan_text("nothing to see").unwrap_err();
        assert_eq!(e.params.get("line").map(String::as_str), Some("1"));
    }

    #[test]
    fn pasted_plan_gets_recommendations() {
        // С ANALYZE: Seq Scan отбросил почти всё — совет про индекс по колонкам фильтра.
        let analyzed = "Seq Scan on orders  (cost=0.00..10000.00 rows=500 width=4) (actual time=0.010..50.000 rows=500 loops=1)\n  Filter: (status = 'new'::text)\n  Rows Removed by Filter: 499500\nPlanning Time: 0.100 ms\nExecution Time: 50.100 ms\n";
        let p = parse_pasted(analyzed).unwrap();
        assert!(
            p.recommendations
                .iter()
                .any(|r| r.id == "plan_seq_scan_filter"),
            "{:?}",
            p.recommendations
        );
        assert!(p.recommendations_skipped.is_empty());
        // Без ANALYZE правила «факт» пропущены — интерфейс скажет, сколько.
        let p = parse_pasted("Seq Scan on orders  (cost=0.00..10000.00 rows=500 width=4)\n  Filter: (status = 'new'::text)\n").unwrap();
        assert!(!p.recommendations_skipped.is_empty());
        let v = serde_json::to_value(&p).unwrap();
        assert!(v.get("recommendationsSkipped").is_some() && v.get("plan").is_some());
    }
}
