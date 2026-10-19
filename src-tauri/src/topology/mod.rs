//! Топология подключения: участники, роли, лаг, журнал и фоновое наблюдение (спец. P1 §6).

pub mod journal;
pub mod model;

pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
