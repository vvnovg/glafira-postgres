//! Профиль ожиданий (спец. P2 §5.3): читает буфер сэмплера узла; сэмплер не запущен вкладкой — отчёт содержит
//! накопленное и `samplerRunning = false`.

use crate::diag::{DiagData, DiagReport, Tool, make_report};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::sync::MutexExt;
use crate::topology::runtime::DiagTarget;

pub async fn report(
    st: &AppState,
    id: &str,
    target: &DiagTarget,
    window_s: u32,
) -> AppResult<DiagReport> {
    let member = target
        .member
        .as_ref()
        .map(|m| m.name.clone())
        .ok_or_else(|| AppError::internal_with("waits without a member"))?;
    // Окно — по часам приложения, как и время сэмплов (уточнение 10 плана P2a).
    let now = crate::topology::model::now_ms();
    let from = now - window_s as i64 * 1000;
    let running = st.samplers.is_running(id, &member);
    let data = match st.samplers.buffer(id, &member) {
        Some(b) => {
            let b = b.lock_safe();
            pg_advisor::ash::profile(
                &b.samples(from),
                &b.ticks(from),
                &b.gaps(from),
                window_s,
                now,
                b.oldest_tick(),
                running,
            )
        }
        None => pg_advisor::ash::profile(&[], &[], &[], window_s, now, None, running),
    };
    let findings = pg_advisor::rules::waits::evaluate(Some(&member), &data);
    Ok(make_report(
        Tool::Waits,
        target,
        None,
        DiagData::Waits(data),
        findings,
        vec![],
    ))
}
