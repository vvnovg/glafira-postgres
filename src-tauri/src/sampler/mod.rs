//! Сэмплер активности (спец. P2 §6): раз в секунду не-idle сессии узла, в памяти, пока подписана вкладка.

pub mod buffer;
pub mod query;
pub mod registry;

use std::sync::Arc;

use crate::error::{AppError, AppResult};
use crate::session::{SessionKind, SessionRegistry};
use crate::state::AppState;
use crate::topology::runtime::not_connected;
use registry::{SamplerRegistry, SamplerSink};

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
    // Остановка сессии только отписывает её (std-мьютекс реестра сэмплеров): годится и для слушателя закрытия.
    st.sessions.insert(
        session_id.clone(),
        id,
        SessionKind::Sampler,
        unsubscribe_on_stop(&st.samplers, id, &m, &session_id),
    );
    st.samplers
        .subscribe(id, target.generation, &m, &session_id, sink, source);
    // Подключение могли закрыть (или переоткрыть) между выбором узла и подпиской: слушатели закрытия тогда
    // не видели ни сессии, ни подписки. Проверка после подписки — под той же блокировкой `Runtimes`, что и
    // рассылка закрытий: закрытие либо уже было (видно здесь), либо будет и найдёт сессию. Если оно уже было,
    // запись сессии могла уйти раньше подписки, и её остановка подписку не снимет: отписка здесь явная.
    let current = st.runtimes.diag_target(id, Some(&m), false);
    if current.as_ref().map(|t| t.generation).ok() != Some(target.generation) {
        withdraw_late(
            &st.sessions,
            &st.samplers,
            id,
            &m,
            &session_id,
            target.generation,
        );
        return Err(current.err().unwrap_or_else(not_connected));
    }
    Ok(session_id)
}

/// Остановка сессии сэмплера: отписка вкладки от узла. Реестр сэмплеров — слабая ссылка: он сам держит реестр
/// сессий, сильная дала бы цикл ссылок.
fn unsubscribe_on_stop(
    samplers: &Arc<SamplerRegistry>,
    connection: &str,
    member: &str,
    session_id: &str,
) -> Box<dyn FnOnce() + Send> {
    let (samplers, connection, member, session_id) = (
        Arc::downgrade(samplers),
        connection.to_string(),
        member.to_string(),
        session_id.to_string(),
    );
    Box::new(move || {
        if let Some(s) = samplers.upgrade() {
            s.unsubscribe(&connection, &member, &session_id);
        }
    })
}

/// Снятие подписки, оформленной на закрытое или переоткрытое подключение. Закрытие могло вынуть сессию из
/// реестра раньше подписки: тогда `SessionRegistry::stop` ничего не вызовет, замыкание отписки не сработает, и
/// свежая подписка осталась бы на узле закрытого подключения — задача узла сэмплировала бы его дальше. Поэтому
/// отписка здесь явная, а запись сессии (если она ещё есть) забывается без вызова остановки: повторной
/// отписки нет. Пустой узел этого открытия убирается вместе с буфером.
fn withdraw_late(
    sessions: &SessionRegistry,
    samplers: &SamplerRegistry,
    connection: &str,
    member: &str,
    session_id: &str,
    generation: u64,
) {
    samplers.unsubscribe(connection, member, session_id);
    sessions.forget(session_id);
    samplers.forget_empty_node(connection, member, generation);
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use futures_util::future::BoxFuture;

    use super::*;
    use crate::sampler::buffer::RawSample;
    use crate::sampler::registry::{SampleSource, SamplerEvent};
    use crate::sync::MutexExt;
    use crate::topology::runtime::CloseListener;

    #[derive(Default)]
    struct Calls(AtomicUsize);
    impl SampleSource for Calls {
        fn sample<'a>(&'a self) -> BoxFuture<'a, AppResult<Vec<RawSample>>> {
            Box::pin(async move {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(vec![])
            })
        }
    }

    #[derive(Default)]
    struct Events(Mutex<Vec<SamplerEvent>>);
    impl SamplerSink for Events {
        fn send(&self, e: SamplerEvent) -> bool {
            self.0.lock_safe().push(e);
            true
        }
    }

    /// Закрытие подключения между вставкой сессии и подпиской (`start`): сессию закрытие уже вынуло из реестра,
    /// её замыкание отписки отработало вхолостую, а подписка оформилась позже. Проверка поколения после подписки
    /// снимает такую подписку, и задача узла закрытого подключения не остаётся.
    #[tokio::test]
    async fn subscription_that_lost_the_race_with_a_close_is_withdrawn() {
        let sessions = Arc::new(SessionRegistry::default());
        let samplers = SamplerRegistry::new(Duration::from_millis(20), sessions.clone());
        sessions.insert(
            "s1".into(),
            "c",
            SessionKind::Sampler,
            unsubscribe_on_stop(&samplers, "c", "m", "s1"),
        );
        // Порядок слушателей — как в `AppState`: сначала сэмплеры, потом сессии.
        samplers.connection_closed("c");
        sessions.connection_closed("c");
        let (source, sink) = (Arc::new(Calls::default()), Arc::new(Events::default()));
        samplers.subscribe("c", 1, "m", "s1", sink.clone(), source.clone());
        assert!(samplers.is_running("c", "m"), "окно гонки: подписка есть");

        withdraw_late(&sessions, &samplers, "c", "m", "s1", 1);

        assert!(!samplers.is_running("c", "m"), "задача узла снята");
        assert!(samplers.buffer("c", "m").is_none(), "пустой узел убран");
        let calls = source.0.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            source.0.load(Ordering::SeqCst),
            calls,
            "закрытое подключение не сэмплируется"
        );
        assert!(!sessions.stop("s1"), "записи сессии нет");
    }

    /// Обычная отмена (сессия ещё в реестре): отписка одна, без вреда соседям по узлу.
    #[tokio::test]
    async fn withdrawing_keeps_the_other_subscribers_of_the_node() {
        let sessions = Arc::new(SessionRegistry::default());
        let samplers = SamplerRegistry::new(Duration::from_millis(20), sessions.clone());
        let (source, other, late) = (
            Arc::new(Calls::default()),
            Arc::new(Events::default()),
            Arc::new(Events::default()),
        );
        for sid in ["other", "late"] {
            sessions.insert(
                sid.into(),
                "c",
                SessionKind::Sampler,
                unsubscribe_on_stop(&samplers, "c", "m", sid),
            );
        }
        samplers.subscribe("c", 1, "m", "other", other, source.clone());
        samplers.subscribe("c", 1, "m", "late", late, source);

        withdraw_late(&sessions, &samplers, "c", "m", "late", 1);

        assert!(
            samplers.is_running("c", "m"),
            "соседняя вкладка держит узел"
        );
        assert!(!sessions.stop("late"), "сессия отменённой подписки забыта");
        assert!(sessions.stop("other"), "сессия соседа цела");
        assert!(
            !samplers.is_running("c", "m"),
            "последняя отписка снимает задачу"
        );
    }
}
