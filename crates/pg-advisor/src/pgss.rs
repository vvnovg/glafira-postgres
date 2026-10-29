//! Возможности установленной версии `pg_stat_statements` (спец. P3 §3.3): набор колонок определяется
//! `pg_extension.extversion`, а не версией сервера — после `pg_upgrade` без `ALTER EXTENSION … UPDATE` сервер 17
//! может работать с pgss 1.10.

/// Версия расширения `major.minor`; сравнение числовое (1.10 новее 1.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PgssVersion {
    pub major: u32,
    pub minor: u32,
}

impl PgssVersion {
    pub const V1_8: Self = Self { major: 1, minor: 8 };
    pub const V1_9: Self = Self { major: 1, minor: 9 };
    pub const V1_10: Self = Self {
        major: 1,
        minor: 10,
    };
    pub const V1_11: Self = Self {
        major: 1,
        minor: 11,
    };
    pub const V1_12: Self = Self {
        major: 1,
        minor: 12,
    };

    pub fn parse(s: &str) -> Option<Self> {
        let (major, minor) = s.trim().split_once('.')?;
        Some(Self {
            major: major.parse().ok()?,
            minor: minor.parse().ok()?,
        })
    }
}

/// Какие колонки есть у установленной версии (спец. §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PgssCaps {
    pub version: PgssVersion,
    /// 1.9: колонка `toplevel`.
    pub toplevel: bool,
    /// 1.9: представление `pg_stat_statements_info` (`dealloc`, `stats_reset`).
    pub info: bool,
    /// 1.10: `jit_*`.
    pub jit: bool,
    /// 1.10: `temp_blk_read_time`, `temp_blk_write_time`.
    pub temp_io_time: bool,
    /// 1.11: `shared_blk_read_time` / `shared_blk_write_time` вместо `blk_read_time` / `blk_write_time`; `local_blk_*_time`;
    /// `jit_deform_time`.
    pub shared_io_names: bool,
    /// 1.11: `stats_since`, `minmax_stats_since`.
    pub stats_since: bool,
    /// 1.12: `parallel_workers_to_launch`, `parallel_workers_launched`, `generic_plan_calls`, `custom_plan_calls`.
    pub parallel: bool,
}

impl PgssCaps {
    /// `None` — версия старше 1.8 (нет `*_exec_time`, планирования и WAL) или строка не разобралась.
    pub fn from_version(s: &str) -> Option<Self> {
        let v = PgssVersion::parse(s)?;
        if v < PgssVersion::V1_8 {
            return None;
        }
        Some(Self {
            version: v,
            toplevel: v >= PgssVersion::V1_9,
            info: v >= PgssVersion::V1_9,
            jit: v >= PgssVersion::V1_10,
            temp_io_time: v >= PgssVersion::V1_10,
            shared_io_names: v >= PgssVersion::V1_11,
            stats_since: v >= PgssVersion::V1_11,
            parallel: v >= PgssVersion::V1_12,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(
            PgssVersion::parse("1.10"),
            Some(PgssVersion {
                major: 1,
                minor: 10
            })
        );
        assert_eq!(PgssVersion::parse(" 1.8 "), Some(PgssVersion::V1_8));
        assert_eq!(PgssVersion::parse("1"), None);
        assert_eq!(PgssVersion::parse("x.y"), None);
        assert!(
            PgssVersion::V1_9 < PgssVersion::V1_10,
            "1.10 новее 1.9 — сравнение не строковое"
        );
    }

    #[test]
    fn caps_follow_the_extension_version() {
        assert!(
            PgssCaps::from_version("1.7").is_none(),
            "старше 1.8 — нет *_exec_time"
        );
        let c8 = PgssCaps::from_version("1.8").unwrap();
        assert!(
            !c8.toplevel
                && !c8.info
                && !c8.jit
                && !c8.temp_io_time
                && !c8.shared_io_names
                && !c8.parallel
        );
        let c9 = PgssCaps::from_version("1.9").unwrap();
        assert!(c9.toplevel && c9.info && !c9.jit);
        let c10 = PgssCaps::from_version("1.10").unwrap();
        assert!(c10.jit && c10.temp_io_time && !c10.shared_io_names);
        let c11 = PgssCaps::from_version("1.11").unwrap();
        assert!(c11.shared_io_names && c11.stats_since && !c11.parallel);
        let c12 = PgssCaps::from_version("1.12").unwrap();
        assert!(c12.parallel && c12.jit && c12.toplevel);
    }
}
