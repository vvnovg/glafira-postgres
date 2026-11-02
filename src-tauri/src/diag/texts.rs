//! Кеш текстов запросов `pg_stat_statements` по узлу (замечание 12 смоука P3b): тексты и отметки «текста нет»,
//! прочитанные отчётами «Запросов», командой `query_texts` и карточкой запроса. Кеш не зависит от сэмплера pgss: свои
//! запросы Глафиры (`is_own_text`) распознаются и без подписки на сэмплер, и до неё. Узел — подключение, номер открытия
//! и участник (`NodeKey`): после переподключения кеш узла начинается заново, записи закрытого подключения и ушедшего
//! узла удаляются по `CloseListener`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pg_advisor::queries::QueryKey;

use crate::diag::query_detail::NodeKey;
use crate::explain::run::is_own_explain;
use crate::product::is_marked_sql;
use crate::sync::MutexExt;
use crate::topology::probe::is_unmarked_poll_sql;
use crate::topology::runtime::CloseListener;

/// Текстов на узел (вместе с отметками «текста нет»); при переполнении кеш узла очищается целиком.
pub const MAX_TEXTS: usize = 2000;

/// Байт текстов на узел: тексты pgss бывают длинными, а кеш живёт до закрытия подключения у каждого узла, где
/// открывались «Запросы»; при переполнении кеш узла очищается целиком.
pub const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;

/// Свой запрос Глафиры: текст с меткой, запрос опроса узла без метки (строка записана до метки) или обёртка EXPLAIN
/// консоли с опциями Глафиры (`explain::run::is_own_explain`).
pub fn is_own_text(text: &str) -> bool {
    is_marked_sql(text) || is_unmarked_poll_sql(text) || is_own_explain(text)
}

/// Тексты одного узла: текст и признак обрезки по ключу; `None` — текст уже читался, и у строки pgss его нет (NULL или
/// строки уже нет).
#[derive(Debug, Default)]
pub struct NodeTexts {
    texts: HashMap<QueryKey, Option<(Arc<str>, bool)>>,
    /// Сумма длин текстов в байтах (для `MAX_TEXT_BYTES`).
    bytes: usize,
}

impl NodeTexts {
    pub fn text(&self, key: &QueryKey) -> Option<(Arc<str>, bool)> {
        self.texts.get(key).cloned().flatten()
    }

    /// Текст ключа уже читался, и его нет: карточка и отчёт не перечитывают файл текстов на каждом опросе.
    pub fn text_missing(&self, key: &QueryKey) -> bool {
        matches!(self.texts.get(key), Some(None))
    }

    /// Про ключ что-то известно: текст или отметка «текста нет».
    pub fn known(&self, key: &QueryKey) -> bool {
        self.texts.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.texts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }

    /// Суммарная длина текстов в байтах.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Переполнение (по числу ключей или байтам) очищает кеш целиком: и тексты, и отметки «текста нет». Вызывается
    /// один раз на запись, до вставки: только что прочитанное не теряется.
    fn make_room(&mut self, adding: usize, adding_bytes: usize) {
        if self.texts.len() + adding > MAX_TEXTS || self.bytes + adding_bytes > MAX_TEXT_BYTES {
            self.texts.clear();
            self.bytes = 0;
        }
    }

    /// Одна запись чтения: тексты и отметки «текста нет» (отметка известный текст не затирает) — с одной проверкой
    /// предела на всю запись (M3 ревью волны смоука P3b).
    pub fn remember(
        &mut self,
        items: impl IntoIterator<Item = (QueryKey, String, bool)>,
        no_text: &[QueryKey],
    ) {
        let items: Vec<(QueryKey, String, bool)> = items.into_iter().collect();
        // Прирост с учётом заменяемых записей: замена текста того же ключа не раздувает кеш.
        let replaced: usize = items
            .iter()
            .filter_map(|(k, _, _)| self.text(k).map(|(t, _)| t.len()))
            .sum();
        let added = items.iter().map(|(_, t, _)| t.len()).sum::<usize>();
        let new_keys = items
            .iter()
            .map(|(k, _, _)| k)
            .chain(no_text)
            .filter(|k| !self.known(k))
            .count();
        self.make_room(new_keys, added.saturating_sub(replaced));
        for (k, t, truncated) in items {
            self.bytes += t.len();
            if let Some(Some((old, _))) = self
                .texts
                .insert(k, Some((Arc::from(t.as_str()), truncated)))
            {
                self.bytes -= old.len();
            }
        }
        for k in no_text {
            self.texts.entry(*k).or_insert(None);
        }
    }

    pub fn remember_texts(&mut self, items: impl IntoIterator<Item = (QueryKey, String, bool)>) {
        self.remember(items, &[]);
    }

    /// Отметка «текста нет» для ключей; известный текст она не затирает.
    pub fn remember_no_text(&mut self, keys: &[QueryKey]) {
        self.remember([], keys);
    }
}

/// Кеши текстов узлов открытых подключений.
#[derive(Debug, Default)]
pub struct QueryTexts {
    nodes: Mutex<HashMap<NodeKey, NodeTexts>>,
}

impl QueryTexts {
    /// Чтение кеша узла под короткой блокировкой (без `await` внутри); узла нет — пустой кеш.
    pub fn with<R>(&self, node: &NodeKey, f: impl FnOnce(&NodeTexts) -> R) -> R {
        let nodes = self.nodes.lock_safe();
        match nodes.get(node) {
            Some(t) => f(t),
            None => f(&NodeTexts::default()),
        }
    }

    /// Запись в кеш узла. Кеш прежнего открытия того же узла удаляется: его тексты относятся к закрытому подключению.
    /// Запись устаревшего открытия (чтение началось до переподключения и закончилось после него) отбрасывается и кеш
    /// нового открытия не трогает.
    pub fn update<R>(&self, node: &NodeKey, f: impl FnOnce(&mut NodeTexts) -> R) -> R {
        let mut nodes = self.nodes.lock_safe();
        let stale = nodes
            .keys()
            .any(|k| k.id == node.id && k.member == node.member && k.generation > node.generation);
        if stale {
            return f(&mut NodeTexts::default());
        }
        nodes.retain(|k, _| {
            k.id != node.id || k.member != node.member || k.generation == node.generation
        });
        f(nodes.entry(node.clone()).or_default())
    }

    pub fn text(&self, node: &NodeKey, key: &QueryKey) -> Option<(Arc<str>, bool)> {
        self.with(node, |t| t.text(key))
    }
}

impl CloseListener for QueryTexts {
    fn connection_closed(&self, connection_id: &str) {
        self.nodes.lock_safe().retain(|k, _| k.id != connection_id);
    }

    fn member_removed(&self, connection_id: &str, member: &str) {
        self.nodes
            .lock_safe()
            .retain(|k, _| !(k.id == connection_id && k.member == member));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: i64) -> QueryKey {
        QueryKey {
            query_id: id,
            dbid: 1,
            userid: 10,
            toplevel: true,
        }
    }

    fn node(id: &str, generation: u64, member: &str) -> NodeKey {
        NodeKey {
            id: id.into(),
            generation,
            member: member.into(),
        }
    }

    #[test]
    fn texts_cache_is_bounded() {
        let mut t = NodeTexts::default();
        t.remember_texts([(key(1), "SELECT 1".to_string(), false)]);
        assert_eq!(
            t.text(&key(1)).map(|(s, _)| s.to_string()),
            Some("SELECT 1".into())
        );
        t.remember_texts((2..=(MAX_TEXTS as i64 + 1)).map(|i| (key(i), format!("q{i}"), false)));
        assert!(t.text(&key(1)).is_none(), "переполнение очищает кеш");
        assert_eq!(t.len(), MAX_TEXTS);
    }

    /// M3 ревью: у границы предела одна запись (тексты + отметки) очищает кеш один раз и сохраняет всё прочитанное.
    #[test]
    fn one_write_clears_once_and_keeps_what_it_read() {
        let mut t = NodeTexts::default();
        t.remember_no_text(&(1..=(MAX_TEXTS as i64 - 1)).map(key).collect::<Vec<_>>());
        t.remember(
            [(key(5000), "SELECT 1".to_string(), false)],
            &[key(5001), key(5002)],
        );
        assert!(t.text(&key(5000)).is_some(), "прочитанный текст не потерян");
        assert!(t.text_missing(&key(5001)) && t.text_missing(&key(5002)));
        assert!(!t.known(&key(1)), "старое вытеснено");
        assert_eq!(t.len(), 3);
    }

    /// Предел по байтам: длинные тексты не держат десятки мегабайт на узел.
    #[test]
    fn texts_cache_is_bounded_by_bytes() {
        let mut t = NodeTexts::default();
        let long = "x".repeat(MAX_TEXT_BYTES / 4);
        for i in 1..=4 {
            t.remember_texts([(key(i), long.clone(), true)]);
        }
        assert_eq!(t.bytes(), MAX_TEXT_BYTES);
        t.remember_texts([(key(1), "SELECT 1".to_string(), false)]);
        assert_eq!(
            t.bytes(),
            MAX_TEXT_BYTES - long.len() + 8,
            "замена текста учитывается"
        );
        t.remember_texts([(key(9), long.clone(), true)]);
        assert!(!t.known(&key(2)), "переполнение по байтам очищает кеш");
        assert_eq!((t.len(), t.bytes()), (1, long.len()));
    }

    /// M2 ревью: запись устаревшего открытия не стирает кеш нового и сама не сохраняется.
    #[test]
    fn a_stale_generation_write_is_dropped() {
        let texts = QueryTexts::default();
        texts.update(&node("c1", 2, "pg-1"), |t| {
            t.remember_texts([(key(1), "SELECT 1".into(), false)])
        });
        texts.update(&node("c1", 1, "pg-1"), |t| t.remember_no_text(&[key(2)]));
        assert!(
            texts.text(&node("c1", 2, "pg-1"), &key(1)).is_some(),
            "кеш нового открытия цел"
        );
        assert!(!texts.with(&node("c1", 1, "pg-1"), |t| t.known(&key(2))));
    }

    #[test]
    fn missing_text_is_remembered_and_a_text_replaces_it() {
        let mut t = NodeTexts::default();
        t.remember_no_text(&[key(1), key(2)]);
        assert!(t.text_missing(&key(1)) && t.known(&key(1)) && !t.known(&key(3)));
        t.remember_texts([(key(1), "SELECT 1".to_string(), false)]);
        assert!(!t.text_missing(&key(1)) && t.text(&key(1)).is_some());
        t.remember_no_text(&[key(1)]);
        assert!(t.text(&key(1)).is_some(), "отметка не затирает текст");
    }

    #[test]
    fn nodes_are_separate_and_a_reconnect_starts_over() {
        let texts = QueryTexts::default();
        let a = node("c1", 1, "pg-1");
        texts.update(&a, |t| {
            t.remember_texts([(key(1), "SELECT 1".into(), false)])
        });
        texts.update(&node("c1", 1, "pg-2"), |t| t.remember_no_text(&[key(1)]));
        assert!(texts.text(&a, &key(1)).is_some());
        assert!(texts.with(&node("c1", 1, "pg-2"), |t| t.text_missing(&key(1))));
        assert!(
            texts.text(&node("c1", 2, "pg-1"), &key(1)).is_none(),
            "другое открытие"
        );
        texts.update(&node("c1", 2, "pg-1"), |t| t.remember_no_text(&[key(2)]));
        assert!(
            texts.text(&a, &key(1)).is_none(),
            "кеш прежнего открытия удалён"
        );
        texts.member_removed("c1", "pg-2");
        assert!(!texts.with(&node("c1", 1, "pg-2"), |t| t.known(&key(1))));
        texts.update(&node("c2", 1, "pg-1"), |t| t.remember_no_text(&[key(1)]));
        texts.connection_closed("c1");
        assert!(!texts.with(&node("c1", 2, "pg-1"), |t| t.known(&key(2))));
        assert!(texts.with(&node("c2", 1, "pg-1"), |t| t.known(&key(1))));
    }

    #[test]
    fn own_texts() {
        assert!(is_own_text("/* glafira */ SELECT 1"));
        assert!(!is_own_text("SELECT 1"));
    }
}
