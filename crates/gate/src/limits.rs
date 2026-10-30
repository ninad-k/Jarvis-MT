use serde::{Deserialize, Serialize};

/// `[limits]` from `config/jarvis.toml`. Checked before any preview is issued.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// Max volume of one new order. It caps new exposure only: closing and cancelling
    /// reduce risk, so a close (single, partial or batch) is never limited by it.
    pub max_lot: f64,
    /// Max open positions + pending orders after a new order.
    pub max_positions: u32,
    /// Positive amount in account currency. New orders are blocked once today's
    /// realised P/L + current floating P/L is at or below `-max_daily_loss`.
    /// Closing/cancelling is still allowed (it reduces risk).
    pub max_daily_loss: f64,
    /// Broker symbols that may be traded. Empty means none (fail closed).
    pub allowed_symbols: Vec<String>,
    /// When new orders may be placed, in broker server time. `None` = always.
    /// Closing/modifying/cancelling is allowed outside these hours.
    #[serde(default)]
    pub trading_hours: Option<TradingHours>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

/// A daily window on the given days, server time, `"HH:MM"` inclusive start,
/// exclusive end. `start > end` means the window crosses midnight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradingHours {
    pub days: Vec<Weekday>,
    pub start: String,
    pub end: String,
}

const SECS_PER_DAY: i64 = 86_400;

impl Weekday {
    const ALL: [Weekday; 7] = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ];

    /// Weekday of a Unix timestamp, treating it as broker-local time.
    pub(crate) fn from_unix(ts: i64) -> Weekday {
        let days = ts.div_euclid(SECS_PER_DAY);
        // 1970-01-01 was a Thursday (index 3 with Monday = 0).
        Self::ALL[(days + 3).rem_euclid(7) as usize]
    }

    fn previous(self) -> Weekday {
        Self::ALL[(self as usize + 6) % 7]
    }
}

/// Parse a strict `"HH:MM"` (two digits each) into minutes since midnight.
fn parse_hhmm(s: &str) -> Result<u32, String> {
    let bad = || format!("{s:?} is not a valid HH:MM time");
    let (h, m) = s.split_once(':').ok_or_else(bad)?;
    let two_digits = |p: &str| p.len() == 2 && p.bytes().all(|b| b.is_ascii_digit());
    if !two_digits(h) || !two_digits(m) {
        return Err(bad());
    }
    let (h, m): (u32, u32) = (h.parse().map_err(|_| bad())?, m.parse().map_err(|_| bad())?);
    if h >= 24 || m >= 60 {
        return Err(bad());
    }
    Ok(h * 60 + m)
}

impl Limits {
    /// Validate config values (positive numbers, parseable times). Called at startup.
    ///
    /// An empty `allowed_symbols` list is valid: it means "trade nothing" (fail closed).
    pub fn validate(&self) -> Result<(), String> {
        if !(self.max_lot.is_finite() && self.max_lot > 0.0) {
            return Err(format!(
                "limits.max_lot must be a positive number, got {}",
                self.max_lot
            ));
        }
        if self.max_positions < 1 {
            return Err("limits.max_positions must be at least 1".to_owned());
        }
        if !(self.max_daily_loss.is_finite() && self.max_daily_loss > 0.0) {
            return Err(format!(
                "limits.max_daily_loss must be a positive number, got {}",
                self.max_daily_loss
            ));
        }
        if self.allowed_symbols.iter().any(|s| s.trim().is_empty()) {
            return Err("limits.allowed_symbols must not contain empty names".to_owned());
        }
        if let Some(h) = &self.trading_hours {
            h.validate()?;
        }
        Ok(())
    }
}

impl TradingHours {
    fn validate(&self) -> Result<(), String> {
        if self.days.is_empty() {
            return Err("limits.trading_hours.days must list at least one day".to_owned());
        }
        let start =
            parse_hhmm(&self.start).map_err(|e| format!("limits.trading_hours.start: {e}"))?;
        let end = parse_hhmm(&self.end).map_err(|e| format!("limits.trading_hours.end: {e}"))?;
        if start == end {
            return Err(
                "limits.trading_hours.start and end must differ (remove trading_hours to allow any time)"
                    .to_owned(),
            );
        }
        Ok(())
    }

    /// Is `server_time` (Unix seconds in broker server time) inside the window?
    ///
    /// The timestamp is read as broker-local wall-clock time (no timezone maths).
    /// `start` is inclusive and `end` exclusive. When `start > end` the window crosses
    /// midnight and the day check applies to the day the window *starts* on, so the
    /// early-morning tail belongs to the previous day's entry. An unparseable window
    /// contains nothing (fail closed).
    pub fn contains(&self, server_time: i64) -> bool {
        let (Ok(start), Ok(end)) = (parse_hhmm(&self.start), parse_hhmm(&self.end)) else {
            return false;
        };
        let day = Weekday::from_unix(server_time);
        let minute = (server_time.rem_euclid(SECS_PER_DAY) / 60) as u32;
        if start < end {
            self.days.contains(&day) && minute >= start && minute < end
        } else if start > end {
            (self.days.contains(&day) && minute >= start)
                || (self.days.contains(&day.previous()) && minute < end)
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_limits() -> Limits {
        Limits {
            max_lot: 1.0,
            max_positions: 5,
            max_daily_loss: 500.0,
            allowed_symbols: vec!["XAUUSD".into(), "EURUSD".into()],
            trading_hours: Some(TradingHours {
                days: vec![
                    Weekday::Mon,
                    Weekday::Tue,
                    Weekday::Wed,
                    Weekday::Thu,
                    Weekday::Fri,
                ],
                start: "00:05".into(),
                end: "23:55".into(),
            }),
        }
    }

    // 2026-11-02 is a Monday (1_793_577_600 = 00:00 UTC).
    const MON: i64 = 1_793_577_600;
    const DAY: i64 = 86_400;

    fn at(day_offset: i64, hh: i64, mm: i64) -> i64 {
        MON + day_offset * DAY + hh * 3600 + mm * 60
    }

    fn hours(days: &[Weekday], start: &str, end: &str) -> TradingHours {
        TradingHours {
            days: days.to_vec(),
            start: start.into(),
            end: end.into(),
        }
    }

    #[test]
    fn valid_limits_pass() {
        assert_eq!(ok_limits().validate(), Ok(()));
    }

    #[test]
    fn empty_allowed_symbols_is_valid_and_fails_closed() {
        let mut l = ok_limits();
        l.allowed_symbols.clear();
        assert_eq!(l.validate(), Ok(()));
    }

    #[test]
    fn missing_trading_hours_is_valid() {
        let mut l = ok_limits();
        l.trading_hours = None;
        assert_eq!(l.validate(), Ok(()));
    }

    #[test]
    fn invalid_limits_are_rejected() {
        type Mutation = Box<dyn Fn(&mut Limits)>;
        let cases: Vec<(&str, Mutation, &str)> = vec![
            ("zero max_lot", Box::new(|l| l.max_lot = 0.0), "max_lot"),
            (
                "negative max_lot",
                Box::new(|l| l.max_lot = -1.0),
                "max_lot",
            ),
            ("nan max_lot", Box::new(|l| l.max_lot = f64::NAN), "max_lot"),
            (
                "inf max_lot",
                Box::new(|l| l.max_lot = f64::INFINITY),
                "max_lot",
            ),
            (
                "zero max_positions",
                Box::new(|l| l.max_positions = 0),
                "max_positions",
            ),
            (
                "zero daily loss",
                Box::new(|l| l.max_daily_loss = 0.0),
                "max_daily_loss",
            ),
            (
                "negative daily loss",
                Box::new(|l| l.max_daily_loss = -5.0),
                "max_daily_loss",
            ),
            (
                "nan daily loss",
                Box::new(|l| l.max_daily_loss = f64::NAN),
                "max_daily_loss",
            ),
            (
                "empty symbol",
                Box::new(|l| l.allowed_symbols.push(String::new())),
                "allowed_symbols",
            ),
            (
                "blank symbol",
                Box::new(|l| l.allowed_symbols.push("  ".into())),
                "allowed_symbols",
            ),
            (
                "bad start",
                Box::new(|l| l.trading_hours.as_mut().unwrap().start = "25:00".into()),
                "start",
            ),
            (
                "bad start format",
                Box::new(|l| l.trading_hours.as_mut().unwrap().start = "9am".into()),
                "start",
            ),
            (
                "bad minutes",
                Box::new(|l| l.trading_hours.as_mut().unwrap().end = "12:60".into()),
                "end",
            ),
            (
                "short end",
                Box::new(|l| l.trading_hours.as_mut().unwrap().end = "1:30".into()),
                "end",
            ),
            (
                "no days",
                Box::new(|l| l.trading_hours.as_mut().unwrap().days.clear()),
                "days",
            ),
            (
                "empty window",
                Box::new(|l| l.trading_hours.as_mut().unwrap().end = "00:05".into()),
                "differ",
            ),
        ];
        for (name, mutate, needle) in cases {
            let mut l = ok_limits();
            mutate(&mut l);
            let err = l.validate().expect_err(name);
            assert!(
                err.contains(needle),
                "{name}: error {err:?} should mention {needle:?}"
            );
        }
    }

    #[test]
    fn weekday_from_unix_known_dates() {
        assert_eq!(Weekday::from_unix(0), Weekday::Thu); // 1970-01-01
        assert_eq!(Weekday::from_unix(MON), Weekday::Mon);
        assert_eq!(Weekday::from_unix(MON + 6 * DAY + 86_399), Weekday::Sun);
        assert_eq!(Weekday::from_unix(-1), Weekday::Wed); // 1969-12-31
    }

    #[test]
    fn contains_start_inclusive_end_exclusive() {
        let h = hours(&[Weekday::Mon], "09:00", "17:00");
        let cases = [
            (at(0, 8, 59), false),
            (at(0, 9, 0), true),
            (at(0, 12, 30), true),
            (at(0, 16, 59), true),
            (at(0, 17, 0), false),
            (at(0, 23, 59), false),
        ];
        for (t, expected) in cases {
            assert_eq!(h.contains(t), expected, "t={t}");
        }
    }

    #[test]
    fn contains_respects_days() {
        let h = hours(&[Weekday::Mon, Weekday::Wed], "00:00", "23:59");
        assert!(h.contains(at(0, 10, 0)));
        assert!(!h.contains(at(1, 10, 0))); // Tue
        assert!(h.contains(at(2, 10, 0)));
        assert!(!h.contains(at(5, 10, 0))); // Sat
    }

    #[test]
    fn contains_midnight_crossing_uses_start_day() {
        // Sunday evening open: Sun 22:00 -> Mon 01:00, only Sun listed.
        let h = hours(&[Weekday::Sun], "22:00", "01:00");
        assert!(h.contains(at(6, 22, 0))); // Sun 22:00
        assert!(h.contains(at(6, 23, 59)));
        assert!(h.contains(at(7, 0, 0))); // Mon 00:00 (belongs to Sun window)
        assert!(h.contains(at(7, 0, 59)));
        assert!(!h.contains(at(7, 1, 0))); // end exclusive
        assert!(!h.contains(at(6, 21, 59)));
        // Monday evening is not a listed start day.
        assert!(!h.contains(at(0, 22, 30)));
        // Early Sunday is the tail of a Saturday window, which is not listed.
        assert!(!h.contains(at(6, 0, 30)));
    }

    #[test]
    fn contains_midnight_crossing_week_wrap() {
        // Sat 23:00 -> Sun 02:00 with only Sat listed.
        let h = hours(&[Weekday::Sat], "23:00", "02:00");
        assert!(h.contains(at(5, 23, 30)));
        assert!(h.contains(at(6, 1, 59)));
        assert!(!h.contains(at(6, 2, 0)));
    }

    #[test]
    fn contains_fails_closed_on_unparseable_window() {
        let h = hours(&[Weekday::Mon], "nine", "17:00");
        assert!(!h.contains(at(0, 10, 0)));
    }

    #[test]
    fn contains_handles_pre_epoch_times() {
        let h = hours(&[Weekday::Wed], "23:00", "23:59");
        assert!(h.contains(-1 - 60)); // 1969-12-31 23:58:59 -> Wed
    }

    #[test]
    fn config_example_window_matches() {
        let l = ok_limits();
        let h = l.trading_hours.as_ref().unwrap();
        assert!(!h.contains(at(0, 0, 4)));
        assert!(h.contains(at(0, 0, 5)));
        assert!(h.contains(at(4, 23, 54)));
        assert!(!h.contains(at(4, 23, 55)));
        assert!(!h.contains(at(5, 12, 0)));
    }
}
