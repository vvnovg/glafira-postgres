//! Сэмплер активности (спец. P2 §6): раз в секунду не-idle сессии узла, в памяти, пока подписана вкладка.

pub mod buffer;
pub mod query;
pub mod registry;

use std::sync::Arc;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::session::SessionKind;
use crate::state::AppState;
use registry::SamplerSink;

/// Подписка на сэмплер узла (`member = None` — «за лидером» на момент подписки). Возвращает id сессии для
/// `stop_session`. Нужен рантайм tokio: задача сэмплера запускается здесь же.
pub fn start(
    st: &AppState,
    id: &str,
    member: Option<&str>,
    sink: Arc<dyn SamplerSink>,
) -> AppResult<String> {
    let target = st.runtimes.diag_target(id, member, false)?;
    let m = target
        .member
        .map(|m| m.name)
        .ok_or_else(|| AppError::internal_with("sampler without a member"))?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let source = Arc::new(query::PoolSampleSource::new(st.clone(), id, &m));
    // Слабая ссылка: реестр сэмплеров сам держит реестр сессий, сильная дала бы цикл ссылок.
    let (samplers, conn, member_name, sid) = (
        Arc::downgrade(&st.samplers),
        id.to_string(),
        m.clone(),
        session_id.clone(),
    );
    // Остановка сессии только отписывает её (std-мьютекс реестра сэмплеров): годится и для слушателя закрытия.
    st.sessions.insert(
        session_id.clone(),
        id,
        SessionKind::Sampler,
        Box::new(move || {
            if let Some(s) = samplers.upgrade() {
                s.unsubscribe(&conn, &member_name, &sid);
            }
        }),
    );
    st.samplers
        .subscribe(id, target.generation, &m, &session_id, sink, source);
    // Подключение могли закрыть (или переоткрыть) между выбором узла и подпиской: слушатели закрытия тогда
    // не видели ни сессии, ни подписки. Проверка после подписки — под той же блокировкой `Runtimes`, что и
    // рассылка закрытий: закрытие либо уже было (видно здесь), либо будет и найдёт сессию.
    let current = st.runtimes.diag_target(id, Some(&m), false);
    if current.as_ref().map(|t| t.generation).ok() != Some(target.generation) {
        st.sessions.stop(&session_id);
        return Err(current.err().unwrap_or_else(|| {
            app_err!(
                ErrorCode::NodeUnavailable,
                "errors.topology.notConnected",
                "The connection is not open"
            )
        }));
    }
    Ok(session_id)
}
