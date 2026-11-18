//! Risk-based lot size: "buy gold risking 1% with a stop at 3900".
//!
//! The volume is worked out here, in code, from the account's equity and the symbol's tick
//! values, never by the model. It is a plain number that then goes through the gate like any
//! volume the user typed, so `max_lot`, `max_positions` and the other limits all still apply.
//!
//! ```text
//! risk amount   = equity x risk% / 100
//! loss per lot  = |entry - stop| / tick_size x tick_value
//! volume        = risk amount / loss per lot, rounded DOWN to volume_step
//! ```

use jarvis_bridge_client::protocol::{Side, SymbolInfo};
use jarvis_gate::{GateError, MIN_RISK_PERCENT};
use serde::Serialize;

/// Added to a step count before it is rounded down, so binary noise (0.29 / 0.01 is
/// 28.999999999999996) does not lose a step.
const EPS: f64 = 1e-9;

/// How an order was sized, as shown in the preview (`sizing` of the preview DTO).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SizingDto {
    /// What was asked for, in percent of equity.
    pub risk_percent: f64,
    /// That share of equity, in the account currency.
    pub risk_amount: f64,
    pub currency: String,
    /// Distance between the entry and the stop loss, in price units.
    pub stop_distance: f64,
    /// The lot size worked out from it.
    pub volume: f64,
}

/// What the volume is worked out from.
#[derive(Debug, Clone, Copy)]
pub struct SizingInput<'a> {
    pub risk_percent: f64,
    /// The most a trade may risk (`max_risk_percent` of the limits).
    pub max_risk_percent: f64,
    pub side: Side,
    /// The ask for a buy, the bid for a sell, or the price of a pending order.
    pub entry: f64,
    pub stop: f64,
    pub equity: f64,
    pub currency: &'a str,
    pub spec: &'a SymbolInfo,
}

fn invalid(message: impl Into<String>) -> GateError {
    GateError::Invalid(message.into())
}

/// "1", "0.5", "1.25": no trailing zeros.
pub fn fmt_percent(v: f64) -> String {
    let mut s = format!("{v:.4}");
    while s.contains('.') && (s.ends_with('0') || s.ends_with('.')) {
        s.pop();
    }
    s
}

fn money(v: f64) -> String {
    format!("{v:.2}")
}

/// Decimal places of a lot step: 0.01 -> 2, 0.001 -> 3, 1 -> 0.
fn step_decimals(step: f64) -> i32 {
    (0..=8)
        .find(|d| {
            let scaled = step * 10f64.powi(*d);
            (scaled - scaled.round()).abs() < 1e-7
        })
        .unwrap_or(8)
}

fn round_to(v: f64, decimals: i32) -> f64 {
    let f = 10f64.powi(decimals);
    (v * f).round() / f
}

/// Round `volume` DOWN to a multiple of `step`.
pub fn round_down_to_step(volume: f64, step: f64) -> f64 {
    let steps = (volume / step + EPS).floor();
    round_to(steps * step, step_decimals(step))
}

/// The lot size for `input`, or the reason there is none. Errors are [`GateError`]s so they
/// reach the user the way every other refusal does.
pub fn size_by_risk(input: &SizingInput<'_>) -> Result<SizingDto, GateError> {
    let spec = input.spec;
    let percent = input.risk_percent;
    if !percent.is_finite() || percent < MIN_RISK_PERCENT {
        return Err(invalid(format!(
            "risk_percent must be at least {MIN_RISK_PERCENT}, got {}",
            fmt_percent(percent)
        )));
    }
    if percent > input.max_risk_percent {
        return Err(GateError::LimitExceeded {
            rule: "max_risk_percent",
            detail: format!(
                "a risk of {}% exceeds the maximum of {}% of equity per trade",
                fmt_percent(percent),
                fmt_percent(input.max_risk_percent)
            ),
        });
    }
    if !(input.entry.is_finite() && input.entry > 0.0) {
        return Err(invalid(format!(
            "there is no usable entry price for {}",
            spec.symbol
        )));
    }
    let wrong_side = match input.side {
        Side::Buy => input.stop >= input.entry,
        Side::Sell => input.stop <= input.entry,
    };
    if !input.stop.is_finite() || input.stop <= 0.0 || wrong_side {
        let (side, place) = match input.side {
            Side::Buy => ("buy", "below"),
            Side::Sell => ("sell", "above"),
        };
        return Err(invalid(format!(
            "the stop loss {} must be {place} the entry {} for a {side}, so a risk-based size \
             cannot be worked out",
            input.stop, input.entry
        )));
    }
    if !(spec.tick_size.is_finite()
        && spec.tick_size > 0.0
        && spec.tick_value.is_finite()
        && spec.tick_value > 0.0)
    {
        return Err(invalid(format!(
            "{} has no usable tick size or tick value, so a lot size cannot be worked out from \
             risk; give a volume instead",
            spec.symbol
        )));
    }
    if !(spec.volume_step.is_finite() && spec.volume_step > 0.0) {
        return Err(invalid(format!("{} has no valid volume step", spec.symbol)));
    }
    if !(input.equity.is_finite() && input.equity > 0.0) {
        return Err(invalid(
            "the account equity is not above zero, so a risk-based size cannot be worked out",
        ));
    }

    let distance = (input.entry - input.stop).abs();
    let risk_amount = input.equity * percent / 100.0;
    let loss_per_lot = distance / spec.tick_size * spec.tick_value;
    let volume = round_down_to_step(risk_amount / loss_per_lot, spec.volume_step);
    if volume < spec.volume_min - EPS {
        let smallest = spec.volume_min * loss_per_lot;
        return Err(invalid(format!(
            "{}% risk is {} {}; the smallest lot ({}) risks {} {} with a stop {} away. Use a \
             tighter stop or a higher risk.",
            fmt_percent(percent),
            money(risk_amount),
            input.currency,
            fmt_percent(spec.volume_min),
            money(smallest),
            input.currency,
            fmt_percent(round_to(distance, i32::try_from(spec.digits).unwrap_or(8))),
        )));
    }
    Ok(SizingDto {
        risk_percent: percent,
        risk_amount: round_to(risk_amount, 2),
        currency: input.currency.to_owned(),
        stop_distance: round_to(distance, i32::try_from(spec.digits).unwrap_or(8)),
        volume,
    })
}

/// The preview text of a risk-sized order: `"1% risk (39.20 USD)"`.
pub fn describe(sizing: &SizingDto) -> String {
    format!(
        "{}% risk ({} {})",
        fmt_percent(sizing.risk_percent),
        money(sizing.risk_amount),
        sizing.currency
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A symbol as MT5 reports it: tick size and value, lot step and minimum.
    fn spec(symbol: &str, digits: u32, tick_size: f64, tick_value: f64, step: f64) -> SymbolInfo {
        SymbolInfo {
            symbol: symbol.to_owned(),
            digits,
            point: tick_size,
            volume_min: step,
            volume_max: 100.0,
            volume_step: step,
            tick_size,
            tick_value,
            trade_allowed: true,
            currency_profit: "USD".to_owned(),
            ..SymbolInfo::default()
        }
    }

    fn input<'a>(
        spec: &'a SymbolInfo,
        side: Side,
        entry: f64,
        stop: f64,
        equity: f64,
        percent: f64,
    ) -> SizingInput<'a> {
        SizingInput {
            risk_percent: percent,
            max_risk_percent: 2.0,
            side,
            entry,
            stop,
            equity,
            currency: "USD",
            spec,
        }
    }

    #[test]
    fn gold_is_sized_from_the_tick_value() {
        // XAUUSD: 100 oz contract, tick 0.01 = 1.00 USD per lot.
        // 100,000 USD x 1% = 1,000 USD. Buy at 3950.10 (ask), stop 3900.00: 50.10 away =
        // 5,010 ticks = 5,010 USD per lot. 1000 / 5010 = 0.1996 lots -> 0.19.
        let gold = spec("XAUUSD", 2, 0.01, 1.0, 0.01);
        let s = size_by_risk(&input(&gold, Side::Buy, 3950.10, 3900.00, 100_000.0, 1.0)).unwrap();
        assert_eq!(s.volume, 0.19);
        assert_eq!(s.risk_amount, 1000.0);
        assert_eq!(s.stop_distance, 50.1);
        assert_eq!(s.currency, "USD");
        assert_eq!(describe(&s), "1% risk (1000.00 USD)");
    }

    #[test]
    fn gold_sells_from_the_bid_with_the_stop_above() {
        // Sell at 3950.00, stop 3960.00: 10.00 = 1,000 ticks = 1,000 USD per lot.
        // 10,000 x 0.5% = 50 USD -> 0.05 lots.
        let gold = spec("XAUUSD", 2, 0.01, 1.0, 0.01);
        let s = size_by_risk(&input(&gold, Side::Sell, 3950.0, 3960.0, 10_000.0, 0.5)).unwrap();
        assert_eq!(s.volume, 0.05);
        assert_eq!(s.risk_amount, 50.0);
        assert_eq!(describe(&s), "0.5% risk (50.00 USD)");
    }

    #[test]
    fn eurusd_is_sized_in_pips() {
        // EURUSD: tick 0.00001 = 1.00 USD per lot. Buy at 1.08470, stop 1.08270: 200 pips
        // = 20,000 ticks... no: 0.00200 / 0.00001 = 200 ticks = 200 USD per lot.
        // 10,000 x 1% = 100 USD -> 0.50 lots.
        let eur = spec("EURUSD", 5, 0.00001, 1.0, 0.01);
        let s = size_by_risk(&input(&eur, Side::Buy, 1.0847, 1.0827, 10_000.0, 1.0)).unwrap();
        assert_eq!(s.volume, 0.5);
        assert_eq!(s.stop_distance, 0.002);
        // A 35-pip stop: 0.0035 / 0.00001 = 350 USD per lot; 100 / 350 = 0.2857 -> 0.28.
        let s = size_by_risk(&input(&eur, Side::Buy, 1.0847, 1.0812, 10_000.0, 1.0)).unwrap();
        assert_eq!(s.volume, 0.28);
    }

    #[test]
    fn an_index_is_sized_with_its_own_tick_value() {
        // US100: tick 0.1 = 0.10 USD per lot (a contract of 1). Buy at 19,823.3, stop 19,773.3:
        // 50 points = 500 ticks = 50 USD per lot. 10,000 x 0.5% = 50 USD -> 1.0 lot, step 0.1.
        let us100 = spec("US100", 1, 0.1, 0.1, 0.1);
        let s = size_by_risk(&input(&us100, Side::Buy, 19_823.3, 19_773.3, 10_000.0, 0.5)).unwrap();
        assert_eq!(s.volume, 1.0);
        // A 70-point stop: 70 USD per lot; 50 / 70 = 0.714 -> 0.7.
        let s = size_by_risk(&input(&us100, Side::Buy, 19_823.3, 19_753.3, 10_000.0, 0.5)).unwrap();
        assert_eq!(s.volume, 0.7);
    }

    #[test]
    fn the_lot_is_rounded_down_never_up() {
        let eur = spec("EURUSD", 5, 0.00001, 1.0, 0.01);
        // 100 / 199.9 = 0.50025 -> 0.50; 100 / 200.1 = 0.4997 -> 0.49.
        let a = size_by_risk(&input(&eur, Side::Buy, 1.0847, 1.08270 + 0.00001, 10_000.0, 1.0));
        assert_eq!(a.unwrap().volume, 0.5);
        let b = size_by_risk(&input(&eur, Side::Buy, 1.0847, 1.08270 - 0.00001, 10_000.0, 1.0));
        assert_eq!(b.unwrap().volume, 0.49);
    }

    #[test]
    fn a_coarser_step_rounds_to_its_own_grid() {
        // Index CFD with a whole-lot step: 7.9 lots -> 7.
        let idx = spec("GER40", 1, 0.1, 0.1, 1.0);
        // 100 points = 1000 ticks = 100 USD per lot; 790 / 100 = 7.9.
        let s = size_by_risk(&input(&idx, Side::Buy, 18_000.0, 17_900.0, 79_000.0, 1.0)).unwrap();
        assert_eq!(s.volume, 7.0);
    }

    #[test]
    fn below_the_smallest_lot_is_refused_with_the_numbers() {
        // 3,920 x 1% = 39.20; stop 52.10 away = 5,210 USD per lot; smallest lot 0.01 risks 52.10.
        let gold = spec("XAUUSD", 2, 0.01, 1.0, 0.01);
        let err = size_by_risk(&input(&gold, Side::Buy, 3952.10, 3900.00, 3_920.0, 1.0))
            .unwrap_err();
        let GateError::Invalid(m) = err else {
            panic!("expected an invalid request")
        };
        assert!(m.contains("1% risk is 39.20 USD"), "{m}");
        assert!(m.contains("risks 52.10 USD"), "{m}");
    }

    #[test]
    fn the_risk_must_be_in_range() {
        let gold = spec("XAUUSD", 2, 0.01, 1.0, 0.01);
        let low = size_by_risk(&input(&gold, Side::Buy, 3950.0, 3900.0, 100_000.0, 0.05));
        assert!(matches!(low, Err(GateError::Invalid(_))), "{low:?}");
        let high = size_by_risk(&input(&gold, Side::Buy, 3950.0, 3900.0, 100_000.0, 3.0));
        match high {
            Err(GateError::LimitExceeded { rule, detail }) => {
                assert_eq!(rule, "max_risk_percent");
                assert!(detail.contains("3%") && detail.contains("2%"), "{detail}");
            }
            other => panic!("expected the risk limit, got {other:?}"),
        }
        // The edge itself is fine.
        assert!(size_by_risk(&input(&gold, Side::Buy, 3950.0, 3900.0, 100_000.0, 2.0)).is_ok());
        assert!(size_by_risk(&input(&gold, Side::Buy, 3950.0, 3900.0, 100_000.0, 0.1)).is_ok());
        assert!(size_by_risk(&input(&gold, Side::Buy, 3950.0, 3900.0, 100_000.0, f64::NAN)).is_err());
    }

    #[test]
    fn a_stop_on_the_wrong_side_or_a_broken_spec_is_refused() {
        let gold = spec("XAUUSD", 2, 0.01, 1.0, 0.01);
        for (side, entry, stop) in [
            (Side::Buy, 3950.0, 3960.0),
            (Side::Sell, 3950.0, 3940.0),
            (Side::Buy, 3950.0, 3950.0),
            (Side::Buy, 3950.0, 0.0),
        ] {
            let r = size_by_risk(&input(&gold, side, entry, stop, 100_000.0, 1.0));
            assert!(matches!(r, Err(GateError::Invalid(_))), "{side:?} {stop}: {r:?}");
        }
        let mut broken = gold.clone();
        broken.tick_value = 0.0;
        let r = size_by_risk(&input(&broken, Side::Buy, 3950.0, 3900.0, 100_000.0, 1.0));
        assert!(matches!(&r, Err(GateError::Invalid(m)) if m.contains("tick")), "{r:?}");
        let r = size_by_risk(&input(&gold, Side::Buy, 3950.0, 3900.0, 0.0, 1.0));
        assert!(matches!(&r, Err(GateError::Invalid(m)) if m.contains("equity")), "{r:?}");
    }

    #[test]
    fn percentages_print_without_trailing_zeros() {
        assert_eq!(fmt_percent(1.0), "1");
        assert_eq!(fmt_percent(0.5), "0.5");
        assert_eq!(fmt_percent(1.25), "1.25");
        assert_eq!(fmt_percent(2.0), "2");
    }
}
