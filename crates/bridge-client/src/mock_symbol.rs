//! Live fields of the mock `get_symbol`: spread, swaps, trade mode and today's sessions.
//!
//! The stored `SymbolInfo` only holds the static specification. The reply fills every new
//! field that the state leaves unset, so a test can pin one by setting it in `MockState`.
//! The mock's server time is the mock clock read as UTC, with no offset.
use crate::mock::{MockState, unix_now};
use crate::protocol::{SECONDS_PER_DAY, Sessions, SymbolInfo, SymbolTradeMode};

/// The kinds of market the canned sessions and swaps tell apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Market {
    Forex,
    Gold,
    Index,
    Crypto,
}

fn market_of(symbol: &str) -> Market {
    match symbol {
        "XAUUSD" => Market::Gold,
        "US100" | "US30" => Market::Index,
        "BTCUSD" => Market::Crypto,
        _ => Market::Forex,
    }
}

/// Day of the week of a Unix time, 0 = Monday .. 6 = Sunday (1970-01-01 was a Thursday).
fn weekday(unix: i64) -> u32 {
    u32::try_from((unix.div_euclid(SECONDS_PER_DAY) + 3).rem_euclid(7)).unwrap_or(0)
}

/// Quote and trade sessions of `symbol` on `weekday` (0 = Monday), seconds since 00:00.
fn sessions_for(symbol: &str, weekday: u32) -> Sessions {
    let weekend = weekday >= 5;
    let day = SECONDS_PER_DAY;
    let (quote, trade): (Vec<[i64; 2]>, Vec<[i64; 2]>) = match market_of(symbol) {
        // 24h on weekdays with a short break around the server rollover at midnight. Quotes
        // run a few minutes longer than trading.
        Market::Forex if weekend => (vec![], vec![]),
        Market::Forex => (vec![[60, day - 60]], vec![[300, day - 300]]),
        // One-hour break from 22:00 to 23:00, and the week closes at 22:00 on Friday.
        Market::Gold if weekend => (vec![], vec![]),
        Market::Gold => {
            let mut s = vec![[0, 22 * 3600]];
            if weekday < 4 {
                s.push([23 * 3600, day]);
            }
            (s.clone(), s)
        }
        // US cash hours, 14:30 to 21:00.
        Market::Index if weekend => (vec![], vec![]),
        Market::Index => (vec![[52_200, 75_600]], vec![[52_200, 75_600]]),
        Market::Crypto => (vec![[0, day]], vec![[0, day]]),
    };
    Sessions { quote, trade }
}

/// `(swap_mode, swap_long, swap_short)`.
fn swaps_for(symbol: &str) -> (&'static str, f64, f64) {
    match symbol {
        "EURUSD" => ("points", -6.8, 2.4),
        "GBPUSD" => ("points", -5.2, 1.1),
        "XAUUSD" => ("points", -38.5, 14.2),
        "US100" => ("interest_current", -3.5, -1.2),
        "US30" => ("interest_current", -3.2, -1.0),
        "BTCUSD" => ("currency_symbol", -26.0, -9.5),
        _ => ("disabled", 0.0, 0.0),
    }
}

/// Spread in points from the mock quote, or `None` for a symbol without a price.
fn spread_from_quote(state: &MockState, info: &SymbolInfo) -> Option<i64> {
    let price = state.prices.iter().find(|p| p.symbol == info.symbol)?;
    if info.point > 0.0 {
        // Rounded to whole points, as the terminal reports it.
        Some(((price.ask - price.bid) / info.point).round() as i64)
    } else {
        Some(price.spread_points)
    }
}

/// `info` with every unset live field filled in for the mock's current time.
pub(crate) fn with_live_fields(state: &MockState, info: &SymbolInfo) -> SymbolInfo {
    let now = state.clock.unwrap_or_else(unix_now);
    let mut out = info.clone();

    if out.spread.is_none() {
        out.spread = spread_from_quote(state, info);
    }
    let (mode, long, short) = swaps_for(&info.symbol);
    out.swap_mode.get_or_insert_with(|| mode.to_owned());
    out.swap_long.get_or_insert(long);
    out.swap_short.get_or_insert(short);

    let trade_mode = *out.trade_mode.get_or_insert(if info.trade_allowed {
        SymbolTradeMode::Full
    } else {
        SymbolTradeMode::CloseOnly
    });
    let sessions = out
        .sessions
        .get_or_insert_with(|| sessions_for(&info.symbol, weekday(now)))
        .clone();
    if out.trade_open_now.is_none() {
        let second_of_day = now.rem_euclid(SECONDS_PER_DAY);
        out.trade_open_now =
            Some(trade_mode != SymbolTradeMode::Disabled && sessions.trade_open_at(second_of_day));
    }
    out
}
