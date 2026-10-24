//! Профиль ожиданий (спец. P2 §5.3): находка о доле ожиданий блокировок.

use crate::ash::{WaitClass, WaitsData};
use crate::findings::{Finding, grade, thresholds as th};

/// `lockShare`: доля сэмплов `Lock` в окне. Мало сэмплов (`LOCK_SHARE_MIN_SAMPLES`) — доля случайна, находки нет.
/// `top_events` урезан до 10 строк: доля `Lock` по нему занижается, только если событий больше десяти, — приемлемо.
pub fn evaluate(member: Option<&str>, d: &WaitsData) -> Vec<Finding> {
    if d.total_samples < th::LOCK_SHARE_MIN_SAMPLES {
        return vec![];
    }
    let lock: u64 = d
        .top_events
        .iter()
        .filter(|e| e.class == WaitClass::Lock)
        .map(|e| e.samples)
        .sum();
    let share = lock as f64 / d.total_samples as f64;
    grade(share, th::LOCK_SHARE.0, th::LOCK_SHARE.1)
        .map(|sev| {
            vec![
                Finding::new("lockShare", sev)
                    .member(member)
                    .param("sharePct", share * 100.0)
                    .link_tool("locks", member),
            ]
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::findings::Severity;

    #[test]
    fn lock_share_needs_enough_samples() {
        use crate::ash::{TopEvent, WaitClass, WaitsData};
        let data = |lock: u64, total: u64| WaitsData {
            total_samples: total,
            top_events: vec![TopEvent {
                class: WaitClass::Lock,
                event: Some("relation".into()),
                samples: lock,
                share: lock as f64 / total as f64,
            }],
            ..Default::default()
        };
        assert_eq!(
            evaluate(None, &data(60, 100))[0].severity,
            Severity::Critical
        );
        assert_eq!(
            evaluate(None, &data(25, 100))[0].severity,
            Severity::Warning
        );
        assert!(evaluate(None, &data(10, 100)).is_empty());
        assert!(evaluate(None, &data(10, 12)).is_empty(), "мало сэмплов");
    }
}
