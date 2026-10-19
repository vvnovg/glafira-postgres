//! Топология подключения: участники, роли, лаг, журнал и фоновое наблюдение (спец. P1 §6).

pub mod connector;
pub mod journal;
pub mod model;
pub mod patroni;
pub mod probe;

pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
