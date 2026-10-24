//! Буфер сэмплов узла (спец. P2 §6): последние 60 мин, не больше 500 000 строк (порядка 40 МБ строк сэмплов),
//! старое вытесняется. Строки (роль, приложение, БД, клиент, wait event, текст запроса) хранятся один раз в
//! словаре и адресуются индексом.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use pg_advisor::ash::Sample;

pub const DEFAULT_MAX_ROWS: usize = 500_000;
pub const DEFAULT_MAX_AGE_MS: i64 = 3_600_000;
/// Пауза между тиками дольше — промежуток в истории.
const GAP_AFTER_MS: i64 = 2_000;
/// Интернированных строк больше — словарь пересобирается по живым строкам.
const COMPACT_AFTER: usize = 100_000;

/// Строка `pg_stat_activity` сэмпла, как пришла с сервера.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawSample {
    pub pid: i32,
    pub leader_pid: Option<i32>,
    pub backend_type: Option<String>,
    pub database: Option<String>,
    pub user: Option<String>,
    pub application: Option<String>,
    pub client: Option<String>,
    pub wait_type: Option<String>,
    pub wait_event: Option<String>,
    pub query_id: Option<i64>,
    pub query: Option<String>,
}

/// Словарь строк: текст лежит один раз (`Arc<str>` — общий для списка и индекса), строка сэмпла хранит номер.
#[derive(Default)]
struct Interner {
    strings: Vec<Arc<str>>,
    index: HashMap<Arc<str>, u32>,
}

impl Interner {
    fn intern(&mut self, s: &str) -> u32 {
        if let Some(i) = self.index.get(s) {
            return *i;
        }
        let i = self.strings.len() as u32;
        let a: Arc<str> = Arc::from(s);
        self.strings.push(a.clone());
        self.index.insert(a, i);
        i
    }

    fn get(&self, i: u32) -> &str {
        &self.strings[i as usize]
    }

    fn len(&self) -> usize {
        self.strings.len()
    }
}

/// Строка сэмпла в буфере: строки — номера в словаре (`None` в `RawSample` — пустая строка, кроме ожидания).
#[derive(Debug, Clone, Copy)]
struct Row {
    at_ms: i64,
    pid: i32,
    leader_pid: Option<i32>,
    wait_type: Option<u32>,
    wait_event: Option<u32>,
    query_id: Option<i64>,
    query: u32,
    application: u32,
    user: u32,
    database: u32,
    client: u32,
}

pub struct Buffer {
    rows: VecDeque<Row>,
    ticks: VecDeque<i64>,
    gaps: Vec<(i64, i64)>,
    strings: Interner,
    /// Словарь пересобирается, когда строк в нём больше: после сборки — вдвое больше живых (не меньше
    /// `COMPACT_AFTER`), чтобы при множестве живых строк сборка не шла на каждом тике.
    compact_at: usize,
    max_rows: usize,
    max_age_ms: i64,
}

impl Buffer {
    pub fn new(max_rows: usize, max_age_ms: i64) -> Self {
        Self {
            rows: VecDeque::new(),
            ticks: VecDeque::new(),
            gaps: Vec::new(),
            strings: Interner::default(),
            compact_at: COMPACT_AFTER,
            max_rows,
            max_age_ms,
        }
    }

    /// Тик сэмплера в `at_ms` с его строками (пустой список — тоже тик). Возвращает промежуток, если пауза после
    /// прошлого тика дольше 2 с (сэмплер стоял: вкладка была закрыта, запрос не успевал или падал).
    pub fn push(&mut self, at_ms: i64, samples: Vec<RawSample>) -> Option<(i64, i64)> {
        let gap = self
            .ticks
            .back()
            .copied()
            .filter(|last| at_ms - last > GAP_AFTER_MS)
            .map(|last| (last + 1_000, at_ms));
        if let Some(g) = gap {
            self.gaps.push(g);
        }
        self.ticks.push_back(at_ms);
        for s in samples {
            let st = &mut self.strings;
            let row = Row {
                at_ms,
                pid: s.pid,
                leader_pid: s.leader_pid,
                wait_type: s.wait_type.as_deref().map(|x| st.intern(x)),
                wait_event: s.wait_event.as_deref().map(|x| st.intern(x)),
                query_id: s.query_id,
                query: st.intern(s.query.as_deref().unwrap_or("")),
                application: st.intern(s.application.as_deref().unwrap_or("")),
                user: st.intern(s.user.as_deref().unwrap_or("")),
                database: st.intern(s.database.as_deref().unwrap_or("")),
                client: st.intern(s.client.as_deref().unwrap_or("")),
            };
            self.rows.push_back(row);
        }
        self.evict(at_ms);
        gap
    }

    /// Промежуток, известный без тика (например, сэмплер остановлен и запущен снова).
    pub fn mark_gap(&mut self, from_ms: i64, to_ms: i64) {
        self.gaps.push((from_ms, to_ms));
    }

    fn evict(&mut self, now_ms: i64) {
        let oldest = now_ms - self.max_age_ms;
        while self.rows.front().is_some_and(|r| r.at_ms < oldest) {
            self.rows.pop_front();
        }
        while self.ticks.front().is_some_and(|t| *t < oldest) {
            self.ticks.pop_front();
        }
        // Лимит строк снимает тик целиком — вместе с его строками: иначе тик остался бы без части строк,
        // и окно показывало бы секунды работы сэмплера (`sampled_s`) без их сэмплов, а `oldest_tick` —
        // начало истории, которой уже нет. Один тик крупнее лимита вытесняется сам (на практике невозможно:
        // лимит — сотни тысяч строк, а тик — число активных сессий узла).
        while self.rows.len() > self.max_rows {
            let Some(t) = self.ticks.pop_front() else {
                // Строки без тика невозможны (`push` пишет тик раньше строк); на всякий случай — по строкам.
                self.rows.pop_front();
                continue;
            };
            while self.rows.front().is_some_and(|r| r.at_ms <= t) {
                self.rows.pop_front();
            }
            // Тик с тем же временем (часы приложения с миллисекундным шагом) ушёл вместе со строками.
            while self.ticks.front().is_some_and(|x| *x <= t) {
                self.ticks.pop_front();
            }
        }
        self.gaps.retain(|(_, to)| *to >= oldest);
        if self.strings.len() > self.compact_at {
            self.compact();
        }
    }

    /// Пересборка словаря по живым строкам: вытесненные тексты больше не держат память.
    fn compact(&mut self) {
        let old = std::mem::take(&mut self.strings);
        let mut map: HashMap<u32, u32> = HashMap::new();
        let mut keep = |i: u32, new: &mut Interner| -> u32 {
            *map.entry(i).or_insert_with(|| new.intern(old.get(i)))
        };
        let mut fresh = Interner::default();
        for r in &mut self.rows {
            r.wait_type = r.wait_type.map(|i| keep(i, &mut fresh));
            r.wait_event = r.wait_event.map(|i| keep(i, &mut fresh));
            r.query = keep(r.query, &mut fresh);
            r.application = keep(r.application, &mut fresh);
            r.user = keep(r.user, &mut fresh);
            r.database = keep(r.database, &mut fresh);
            r.client = keep(r.client, &mut fresh);
        }
        self.strings = fresh;
        self.compact_at = COMPACT_AFTER.max(self.strings.len() * 2);
    }

    /// Сэмплы с `from_ms`; строки — ссылки в словарь буфера (живут, пока держится блокировка буфера).
    pub fn samples(&self, from_ms: i64) -> Vec<Sample<'_>> {
        let s = &self.strings;
        self.rows
            .iter()
            .filter(|r| r.at_ms >= from_ms)
            .map(|r| Sample {
                at_ms: r.at_ms,
                pid: r.pid,
                leader_pid: r.leader_pid,
                wait_type: r.wait_type.map(|i| s.get(i)),
                wait_event: r.wait_event.map(|i| s.get(i)),
                query_id: r.query_id,
                query: s.get(r.query),
                application: s.get(r.application),
                user: s.get(r.user),
                database: s.get(r.database),
                client: s.get(r.client),
            })
            .collect()
    }

    pub fn ticks(&self, from_ms: i64) -> Vec<i64> {
        self.ticks
            .iter()
            .copied()
            .filter(|t| *t >= from_ms)
            .collect()
    }

    /// Промежутки, заканчивающиеся после `from_ms`.
    pub fn gaps(&self, from_ms: i64) -> Vec<(i64, i64)> {
        self.gaps
            .iter()
            .copied()
            .filter(|(_, to)| *to > from_ms)
            .collect()
    }

    pub fn oldest_tick(&self) -> Option<i64> {
        self.ticks.front().copied()
    }

    /// Число строк сэмплов.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_ROWS, DEFAULT_MAX_AGE_MS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(pid: i32, query: &str) -> RawSample {
        RawSample {
            pid,
            query: Some(query.into()),
            application: Some("api".into()),
            ..Default::default()
        }
    }

    #[test]
    fn push_records_ticks_gaps_and_interns_strings() {
        let mut b = Buffer::new(1000, 60_000);
        assert_eq!(
            b.push(1_000, vec![raw(1, "select 1"), raw(2, "select 1")]),
            None
        );
        assert_eq!(b.push(2_000, vec![]), None, "пустой тик — тоже тик");
        assert_eq!(
            b.push(10_000, vec![raw(1, "select 1")]),
            Some((3_000, 10_000)),
            "пауза больше 2 с — промежуток"
        );
        assert_eq!(b.ticks(0), [1_000, 2_000, 10_000]);
        assert_eq!(b.gaps(0), [(3_000, 10_000)]);
        let s = b.samples(0);
        assert_eq!(s.len(), 3);
        assert!(
            std::ptr::eq(s[0].query.as_ptr(), s[1].query.as_ptr()),
            "текст хранится один раз"
        );
    }

    #[test]
    fn compaction_keeps_only_live_strings() {
        let mut b = Buffer::new(1000, 10_000);
        b.push(1_000, vec![raw(1, "old")]);
        b.push(20_000, vec![raw(2, "new")]);
        assert_eq!(b.strings.len(), 4, "old, api, пустая строка, new");
        b.compact();
        assert_eq!(b.strings.len(), 3, "вытесненный текст ушёл из словаря");
        let s = b.samples(0);
        assert_eq!((s.len(), s[0].query, s[0].application), (1, "new", "api"));
        assert_eq!((s[0].user, s[0].wait_type), ("", None));
        assert_eq!(b.compact_at, COMPACT_AFTER);
    }

    #[test]
    fn eviction_by_age_and_by_rows() {
        let mut b = Buffer::new(3, 10_000);
        b.push(1_000, vec![raw(1, "a"), raw(2, "b")]);
        b.push(2_000, vec![raw(3, "c"), raw(4, "d")]);
        assert_eq!(
            (b.len(), b.oldest_tick(), b.ticks(0)),
            (2, Some(2_000), vec![2_000]),
            "лимит строк снимает тик целиком: его строки и сам тик"
        );
        assert_eq!(
            b.samples(0).iter().map(|s| s.pid).collect::<Vec<_>>(),
            [3, 4]
        );
        b.push(20_000, vec![]);
        assert_eq!(
            (b.len(), b.oldest_tick()),
            (0, Some(20_000)),
            "старше срока хранения — вытеснено"
        );
    }

    #[test]
    fn row_cap_drops_whole_ticks_and_the_profile_does_not_count_them() {
        use pg_advisor::ash::profile;
        let (now, window_s) = (1_000_000, 300);
        let from = now - window_s as i64 * 1000;
        let mut b = Buffer::new(3, 3_600_000);
        // Первый тик — в начале окна: пока он в буфере, история полная (`truncated = false`).
        b.push(from + 1_000, vec![raw(1, "a"), raw(2, "b")]);
        let full = profile(
            &b.samples(from),
            &b.ticks(from),
            &b.gaps(from),
            window_s,
            now,
            b.oldest_tick(),
            true,
        );
        assert!(!full.truncated);
        assert_eq!(full.buckets.iter().map(|x| x.sampled_s).sum::<f64>(), 1.0);
        b.push(from + 60_000, vec![raw(3, "c"), raw(4, "d")]);
        let d = profile(
            &b.samples(from),
            &b.ticks(from),
            &b.gaps(from),
            window_s,
            now,
            b.oldest_tick(),
            true,
        );
        assert!(d.truncated, "начало истории вытеснено лимитом");
        assert_eq!(d.total_samples, 2);
        assert_eq!(
            d.buckets.iter().map(|x| x.sampled_s).sum::<f64>(),
            1.0,
            "секунды вытесненного тика не считаются"
        );
        assert_eq!(d.buckets[0].sampled_s, 0.0);
        assert!(
            d.buckets
                .iter()
                .all(|x| x.sampled_s > 0.0 || x.aas.is_empty()),
            "нет интервала со средним без секунд работы сэмплера"
        );
    }

    #[test]
    fn row_cap_drops_only_as_many_ticks_as_needed_and_empty_ticks_go_in_turn() {
        let mut b = Buffer::new(2, 3_600_000);
        b.push(1_000, vec![raw(1, "a")]);
        b.push(2_000, vec![]);
        b.push(3_000, vec![raw(2, "b"), raw(3, "c")]);
        assert_eq!(
            (b.len(), b.ticks(0)),
            (2, vec![2_000, 3_000]),
            "снят только первый тик: лимит уже соблюдён"
        );
        // Пустой тик занимает место только в списке тиков.
        b.push(4_000, vec![]);
        assert_eq!((b.len(), b.ticks(0)), (2, vec![2_000, 3_000, 4_000]));
        // Тик крупнее лимита вытесняется сам, буфер не превышает лимит.
        b.push(5_000, vec![raw(4, "d"), raw(5, "e"), raw(6, "f")]);
        assert_eq!((b.len(), b.ticks(0)), (0, vec![]));
    }
}
