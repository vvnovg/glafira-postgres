//! Тёплая модель (спец. P4c §6) отделена от очереди трейтом: очередь зовёт `before_run` на старте запуска и `after_run`
//! в конце, не зная про Ollama. Тот же трейт отвечает, есть ли модель в локальной Ollama (`has_local_model`): по нему
//! Qwen Code с моделью не из своих настроек идёт к Ollama. `NoWarmth` ничего не делает и локальных моделей не знает;
//! `agent::ollama::OllamaWarmth` — задача 7.

use futures_util::future::BoxFuture;

pub trait ModelWarmth: Send + Sync {
    /// Старт запуска: предзагрузка модели (в фоне) и предупреждения для записи запуска (§6.3, `evict …`).
    fn before_run(&self, model: Option<&str>) -> BoxFuture<'_, Vec<String>>;
    /// Конец запуска: продлить срок модели в памяти, не укорачивая его.
    fn after_run(&self, model: Option<&str>) -> BoxFuture<'_, ()>;
    /// Модель скачана в локальную Ollama (`/api/tags`; адрес — «Адрес Ollama», только локальный): Qwen Code с такой
    /// моделью не из своих настроек получает адрес и ключ Ollama (итоговая волна правок смоука P4c). Ollama не ответила —
    /// нет. По умолчанию — нет.
    fn has_local_model<'a>(&'a self, _model: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async { false })
    }
}

/// Glafira моделью не управляет.
pub struct NoWarmth;

impl ModelWarmth for NoWarmth {
    fn before_run(&self, _model: Option<&str>) -> BoxFuture<'_, Vec<String>> {
        Box::pin(async { Vec::new() })
    }

    fn after_run(&self, _model: Option<&str>) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn no_warmth_does_nothing() {
        assert!(
            NoWarmth
                .before_run(Some("qwen3.6-64k:35b-a3b"))
                .await
                .is_empty()
        );
        NoWarmth.after_run(None).await;
        assert!(!NoWarmth.has_local_model("qwen3.8:27b").await);
    }
}
