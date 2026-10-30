//! Tighten-only trading limits set from the UI.
//!
//! The config file sets the limits ([`jarvis_gate::Limits`]). The user may add *overrides*
//! that make them stricter, never looser. What the gate enforces is always the config limits
//! intersected with the overrides ([`effective`]): the smaller number, the common symbols,
//! the narrower trading window. Because the effective limits are recomputed from the config
//! on every change and on every start, a stored override can never loosen anything, even
//! after the config file was edited.

use jarvis_gate::{Limits, TradingHours, Weekday};
use serde::{Deserialize, Serialize};

/// Limits the user wants stricter than the config. Absent fields are not overridden.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_lot: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_positions: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_daily_loss: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_symbols: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trading_hours: Option<TradingHours>,
}

impl PartialLimits {
    pub fn is_empty(&self) -> bool {
        self.max_lot.is_none()
            && self.max_positions.is_none()
            && self.max_daily_loss.is_none()
            && self.allowed_symbols.is_none()
            && self.trading_hours.is_none()
    }

    /// Fields set in `newer` replace the ones here; the rest stay.
    pub fn merged_with(&self, newer: &PartialLimits) -> PartialLimits {
        PartialLimits {
            max_lot: newer.max_lot.or(self.max_lot),
            max_positions: newer.max_positions.or(self.max_positions),
            max_daily_loss: newer.max_daily_loss.or(self.max_daily_loss),
            allowed_symbols: newer
                .allowed_symbols
                .clone()
                .or_else(|| self.allowed_symbols.clone()),
            trading_hours: newer
                .trading_hours
                .clone()
                .or_else(|| self.trading_hours.clone()),
        }
    }
}

/// An override that is not at least as strict as the config. `field` names the limit.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct NotStricter {
    pub field: &'static str,
    pub message: String,
}

/// A value that cannot be a limit at all (not a number, not a time).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Malformed(pub String);

/// Why an override was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// 400: the value is malformed.
    Malformed(Malformed),
    /// 422 `limit_not_stricter`.
    NotStricter(NotStricter),
}

fn not_stricter(field: &'static str, message: String) -> Refusal {
    Refusal::NotStricter(NotStricter { field, message })
}

/// Check that every field of `wanted` is well-formed and at least as strict as `config`.
/// Symbols must already be resolved to broker symbols (the same form as `config`'s).
pub fn validate_stricter(config: &Limits, wanted: &PartialLimits) -> Result<(), Refusal> {
    // Well-formedness first: reuse the config validator on a copy that carries the values.
    let mut probe = config.clone();
    if let Some(v) = wanted.max_lot {
        probe.max_lot = v;
    }
    if let Some(v) = wanted.max_positions {
        probe.max_positions = v;
    }
    if let Some(v) = wanted.max_daily_loss {
        probe.max_daily_loss = v;
    }
    if let Some(v) = &wanted.allowed_symbols {
        probe.allowed_symbols = v.clone();
    }
    if let Some(v) = &wanted.trading_hours {
        probe.trading_hours = Some(v.clone());
    }
    // The config itself may have empty allowed_symbols (fail closed); that is not the
    // user's problem here.
    if let Err(message) = probe.validate() {
        return Err(Refusal::Malformed(Malformed(message)));
    }

    if let Some(v) = wanted.max_lot
        && v > config.max_lot
    {
        return Err(not_stricter(
            "max_lot",
            format!(
                "max_lot {v} is higher than the configured {}; limits can only be made stricter",
                config.max_lot
            ),
        ));
    }
    if let Some(v) = wanted.max_positions
        && v > config.max_positions
    {
        return Err(not_stricter(
            "max_positions",
            format!(
                "max_positions {v} is higher than the configured {}; limits can only be made \
                 stricter",
                config.max_positions
            ),
        ));
    }
    if let Some(v) = wanted.max_daily_loss
        && v > config.max_daily_loss
    {
        return Err(not_stricter(
            "max_daily_loss",
            format!(
                "max_daily_loss {v} is higher than the configured {}; limits can only be made \
                 stricter",
                config.max_daily_loss
            ),
        ));
    }
    if let Some(symbols) = &wanted.allowed_symbols {
        let extra: Vec<&str> = symbols
            .iter()
            .filter(|s| !config.allowed_symbols.contains(s))
            .map(String::as_str)
            .collect();
        if !extra.is_empty() {
            return Err(not_stricter(
                "allowed_symbols",
                format!(
                    "allowed_symbols may only list symbols the config allows; not allowed by the \
                     config: {}",
                    extra.join(", ")
                ),
            ));
        }
    }
    if let (Some(hours), Some(base)) = (&wanted.trading_hours, &config.trading_hours) {
        check_hours_stricter(base, hours)?;
    }
    Ok(())
}

fn check_hours_stricter(base: &TradingHours, wanted: &TradingHours) -> Result<(), Refusal> {
    let field = "trading_hours";
    if let Some(day) = wanted.days.iter().find(|d| !base.days.contains(d)) {
        return Err(not_stricter(
            field,
            format!(
                "trading_hours.days includes {day:?}, which the config does not allow; limits \
                 can only be made stricter"
            ),
        ));
    }
    let (Some((base_start, base_end)), Some((start, end))) = (same_day(base), same_day(wanted))
    else {
        return Err(not_stricter(
            field,
            "trading_hours can only be tightened when both the configured window and the new \
             one lie within a single day (start before end)"
                .to_owned(),
        ));
    };
    if start < base_start || end > base_end {
        return Err(not_stricter(
            field,
            format!(
                "trading_hours {}-{} is not inside the configured window {}-{}; limits can only \
                 be made stricter",
                wanted.start, wanted.end, base.start, base.end
            ),
        ));
    }
    Ok(())
}

/// Minutes since midnight of a strict `HH:MM`.
fn minutes(text: &str) -> Option<u32> {
    let (h, m) = text.split_once(':')?;
    let two = |p: &str| p.len() == 2 && p.bytes().all(|b| b.is_ascii_digit());
    if !two(h) || !two(m) {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// `(start, end)` in minutes when the window lies within one day.
fn same_day(hours: &TradingHours) -> Option<(u32, u32)> {
    let (start, end) = (minutes(&hours.start)?, minutes(&hours.end)?);
    (start < end).then_some((start, end))
}

fn hhmm(total: u32) -> String {
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// The limits the gate enforces: `config` intersected with `overrides`. Never looser than
/// `config`, whatever the overrides hold.
pub fn effective(config: &Limits, overrides: Option<&PartialLimits>) -> Limits {
    let mut out = config.clone();
    let Some(o) = overrides else {
        return out;
    };
    if let Some(v) = o.max_lot.filter(|v| v.is_finite() && *v > 0.0) {
        out.max_lot = out.max_lot.min(v);
    }
    if let Some(v) = o.max_positions.filter(|v| *v >= 1) {
        out.max_positions = out.max_positions.min(v);
    }
    if let Some(v) = o.max_daily_loss.filter(|v| v.is_finite() && *v > 0.0) {
        out.max_daily_loss = out.max_daily_loss.min(v);
    }
    if let Some(symbols) = &o.allowed_symbols {
        out.allowed_symbols.retain(|s| symbols.contains(s));
    }
    if let Some(wanted) = &o.trading_hours {
        match &config.trading_hours {
            None => out.trading_hours = Some(wanted.clone()),
            Some(base) => match intersect_hours(base, wanted) {
                Some(Some(hours)) => out.trading_hours = Some(hours),
                // Nothing is left: no moment is allowed. The gate has no "never" window, so
                // trade nothing (fail closed).
                Some(None) => out.allowed_symbols.clear(),
                // Windows that cross midnight cannot be intersected here; keep the config's.
                None => tracing::warn!(
                    "the trading_hours override cannot be combined with the configured window \
                     (one crosses midnight); the configured window applies"
                ),
            },
        }
    }
    out
}

/// `Some(Some(window))`: the overlap; `Some(None)`: no overlap; `None`: not comparable.
fn intersect_hours(base: &TradingHours, wanted: &TradingHours) -> Option<Option<TradingHours>> {
    let (base_start, base_end) = same_day(base)?;
    let (start, end) = same_day(wanted)?;
    let days: Vec<Weekday> = base
        .days
        .iter()
        .copied()
        .filter(|d| wanted.days.contains(d))
        .collect();
    let (start, end) = (start.max(base_start), end.min(base_end));
    if days.is_empty() || start >= end {
        return Some(None);
    }
    Some(Some(TradingHours {
        days,
        start: hhmm(start),
        end: hhmm(end),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEEK: [Weekday; 5] = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
    ];

    fn config() -> Limits {
        Limits {
            max_lot: 1.0,
            max_positions: 5,
            max_daily_loss: 500.0,
            allowed_symbols: vec!["XAUUSD".into(), "EURUSD".into(), "US30".into()],
            trading_hours: Some(TradingHours {
                days: WEEK.to_vec(),
                start: "00:05".into(),
                end: "23:55".into(),
            }),
        }
    }

    fn hours(days: &[Weekday], start: &str, end: &str) -> TradingHours {
        TradingHours {
            days: days.to_vec(),
            start: start.into(),
            end: end.into(),
        }
    }

    fn refused_field(wanted: &PartialLimits) -> &'static str {
        match validate_stricter(&config(), wanted) {
            Err(Refusal::NotStricter(n)) => n.field,
            other => panic!("expected not_stricter, got {other:?}"),
        }
    }

    #[test]
    fn equal_and_stricter_values_pass() {
        let wanted = PartialLimits {
            max_lot: Some(1.0),
            max_positions: Some(2),
            max_daily_loss: Some(100.0),
            allowed_symbols: Some(vec!["XAUUSD".into()]),
            trading_hours: Some(hours(&[Weekday::Mon, Weekday::Tue], "08:00", "17:00")),
        };
        assert_eq!(validate_stricter(&config(), &wanted), Ok(()));
        assert_eq!(
            validate_stricter(&config(), &PartialLimits::default()),
            Ok(())
        );
    }

    #[test]
    fn every_looser_value_names_its_field() {
        let cases = [
            (
                PartialLimits {
                    max_lot: Some(1.01),
                    ..PartialLimits::default()
                },
                "max_lot",
            ),
            (
                PartialLimits {
                    max_positions: Some(6),
                    ..PartialLimits::default()
                },
                "max_positions",
            ),
            (
                PartialLimits {
                    max_daily_loss: Some(500.5),
                    ..PartialLimits::default()
                },
                "max_daily_loss",
            ),
            (
                PartialLimits {
                    allowed_symbols: Some(vec!["XAUUSD".into(), "BTCUSD".into()]),
                    ..PartialLimits::default()
                },
                "allowed_symbols",
            ),
            (
                PartialLimits {
                    trading_hours: Some(hours(&[Weekday::Sat], "09:00", "10:00")),
                    ..PartialLimits::default()
                },
                "trading_hours",
            ),
            (
                PartialLimits {
                    trading_hours: Some(hours(&WEEK, "00:00", "12:00")),
                    ..PartialLimits::default()
                },
                "trading_hours",
            ),
            (
                PartialLimits {
                    trading_hours: Some(hours(&WEEK, "12:00", "23:59")),
                    ..PartialLimits::default()
                },
                "trading_hours",
            ),
            (
                PartialLimits {
                    // Crosses midnight: not comparable, so refused.
                    trading_hours: Some(hours(&WEEK, "22:00", "02:00")),
                    ..PartialLimits::default()
                },
                "trading_hours",
            ),
        ];
        for (wanted, field) in cases {
            assert_eq!(refused_field(&wanted), field, "{wanted:?}");
        }
    }

    #[test]
    fn malformed_values_are_not_limits() {
        for wanted in [
            PartialLimits {
                max_lot: Some(0.0),
                ..PartialLimits::default()
            },
            PartialLimits {
                max_lot: Some(f64::NAN),
                ..PartialLimits::default()
            },
            PartialLimits {
                max_positions: Some(0),
                ..PartialLimits::default()
            },
            PartialLimits {
                max_daily_loss: Some(-5.0),
                ..PartialLimits::default()
            },
            PartialLimits {
                trading_hours: Some(hours(&WEEK, "9:00", "17:00")),
                ..PartialLimits::default()
            },
            PartialLimits {
                trading_hours: Some(hours(&[], "09:00", "17:00")),
                ..PartialLimits::default()
            },
            PartialLimits {
                trading_hours: Some(hours(&WEEK, "09:00", "09:00")),
                ..PartialLimits::default()
            },
        ] {
            assert!(
                matches!(
                    validate_stricter(&config(), &wanted),
                    Err(Refusal::Malformed(_))
                ),
                "{wanted:?}"
            );
        }
    }

    #[test]
    fn without_configured_hours_any_window_is_stricter() {
        let mut c = config();
        c.trading_hours = None;
        let wanted = PartialLimits {
            trading_hours: Some(hours(&[Weekday::Sat], "22:00", "02:00")),
            ..PartialLimits::default()
        };
        assert_eq!(validate_stricter(&c, &wanted), Ok(()));
        assert_eq!(
            effective(&c, Some(&wanted)).trading_hours,
            wanted.trading_hours
        );
    }

    #[test]
    fn effective_is_the_intersection() {
        let o = PartialLimits {
            max_lot: Some(0.5),
            max_positions: Some(9),
            max_daily_loss: Some(100.0),
            allowed_symbols: Some(vec!["XAUUSD".into(), "BTCUSD".into()]),
            trading_hours: Some(hours(&[Weekday::Mon, Weekday::Sat], "08:00", "23:59")),
        };
        let e = effective(&config(), Some(&o));
        assert_eq!(e.max_lot, 0.5);
        assert_eq!(e.max_positions, 5, "a looser stored value never wins");
        assert_eq!(e.max_daily_loss, 100.0);
        assert_eq!(e.allowed_symbols, vec!["XAUUSD".to_owned()]);
        assert_eq!(
            e.trading_hours,
            Some(hours(&[Weekday::Mon], "08:00", "23:55"))
        );
        assert_eq!(effective(&config(), None), config());
        assert_eq!(
            effective(&config(), Some(&PartialLimits::default())),
            config()
        );
    }

    #[test]
    fn disjoint_windows_trade_nothing() {
        let o = PartialLimits {
            trading_hours: Some(hours(&[Weekday::Sat], "08:00", "09:00")),
            ..PartialLimits::default()
        };
        let e = effective(&config(), Some(&o));
        assert!(e.allowed_symbols.is_empty());
        let o = PartialLimits {
            trading_hours: Some(hours(&WEEK, "00:01", "00:04")),
            ..PartialLimits::default()
        };
        assert!(effective(&config(), Some(&o)).allowed_symbols.is_empty());
    }

    #[test]
    fn merge_keeps_old_fields_and_replaces_new_ones() {
        let old = PartialLimits {
            max_lot: Some(0.5),
            max_positions: Some(3),
            ..PartialLimits::default()
        };
        let newer = PartialLimits {
            max_lot: Some(0.2),
            max_daily_loss: Some(50.0),
            ..PartialLimits::default()
        };
        let merged = old.merged_with(&newer);
        assert_eq!(merged.max_lot, Some(0.2));
        assert_eq!(merged.max_positions, Some(3));
        assert_eq!(merged.max_daily_loss, Some(50.0));
        assert!(PartialLimits::default().is_empty());
        assert!(!merged.is_empty());
    }

    #[test]
    fn unknown_fields_are_rejected_when_parsing() {
        assert!(serde_json::from_str::<PartialLimits>(r#"{"max_lot":0.5,"max_lots":1}"#).is_err());
        let ok: PartialLimits = serde_json::from_str(r#"{"max_lot":0.5}"#).unwrap();
        assert_eq!(ok.max_lot, Some(0.5));
        assert_eq!(
            serde_json::to_value(&ok).unwrap(),
            serde_json::json!({"max_lot":0.5})
        );
    }
}
