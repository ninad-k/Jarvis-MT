//! `get_symbol` live fields (spread, swaps, trade mode, today's sessions) against the mock EA
//! at fixed clocks. The session maths on its own is tested at the bottom.
#![cfg(feature = "mock")]

use std::time::Duration;

use jarvis_bridge_client::mock::{MockBridge, MockState};
use jarvis_bridge_client::protocol::{SECONDS_PER_DAY, Sessions, SymbolInfo, SymbolTradeMode};
use jarvis_bridge_client::{BridgeClient, BridgeError};

const TIMEOUT: Duration = Duration::from_millis(500);
/// Monday 2026-11-02 00:00:00 UTC. The mock reads its clock as server time.
const MONDAY: i64 = 1_793_577_600;
const DAY: i64 = SECONDS_PER_DAY;
/// MT5 `ERR_MARKET_UNKNOWN_SYMBOL`.
const UNKNOWN_SYMBOL: i64 = 4301;

/// `day` counts from Monday (0) to Sunday (6).
fn at(day: i64, hour: i64, minute: i64, second: i64) -> i64 {
    MONDAY + day * DAY + hour * 3600 + minute * 60 + second
}

fn state_at(clock: i64) -> MockState {
    MockState {
        clock: Some(clock),
        ..MockState::default()
    }
}

async fn symbol_in(state: MockState, name: &str) -> SymbolInfo {
    let mock = MockBridge::start_with(state).await;
    let client = BridgeClient::new(mock.config(TIMEOUT)).expect("client");
    client
        .get_symbol(name)
        .await
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

async fn symbol_at(clock: i64, name: &str) -> SymbolInfo {
    symbol_in(state_at(clock), name).await
}

fn open_now(info: &SymbolInfo) -> bool {
    info.trade_open_now.expect("trade_open_now")
}

fn trade(info: &SymbolInfo) -> Vec<[i64; 2]> {
    info.sessions.clone().expect("sessions").trade
}

// ---- spread, swaps, trade mode ---------------------------------------------------------

#[tokio::test]
async fn every_symbol_carries_the_live_fields() {
    let state = state_at(at(1, 12, 0, 0));
    let names: Vec<String> = state.symbols.iter().map(|s| s.symbol.clone()).collect();
    assert_eq!(names.len(), 6);
    let mock = MockBridge::start_with(state).await;
    let client = BridgeClient::new(mock.config(TIMEOUT)).expect("client");
    for name in names {
        let info = client.get_symbol(&name).await.expect(&name);
        assert!(info.spread.is_some_and(|s| s > 0), "{name}: spread");
        assert!(
            info.swap_long.is_some() && info.swap_short.is_some() && info.swap_mode.is_some(),
            "{name}: swaps"
        );
        assert_eq!(info.trade_mode, Some(SymbolTradeMode::Full), "{name}");
        assert!(
            info.sessions.is_some() && info.trade_open_now.is_some(),
            "{name}: sessions"
        );
    }
}

#[tokio::test]
async fn spread_comes_from_the_mock_quote_in_points() {
    let state = state_at(at(1, 12, 0, 0));
    let expected: Vec<(String, i64)> = state
        .symbols
        .iter()
        .map(|s| {
            let p = state.prices.iter().find(|p| p.symbol == s.symbol).unwrap();
            (s.symbol.clone(), ((p.ask - p.bid) / s.point).round() as i64)
        })
        .collect();
    let mock = MockBridge::start_with(state).await;
    let client = BridgeClient::new(mock.config(TIMEOUT)).expect("client");
    for (name, spread) in expected {
        let got = client.get_symbol(&name).await.unwrap().spread;
        assert_eq!(got, Some(spread), "{name}");
    }
    // Spot checks against the default quotes: gold 2391.12 / 2391.40, EURUSD 8 points.
    assert_eq!(client.get_symbol("XAUUSD").await.unwrap().spread, Some(28));
    assert_eq!(client.get_symbol("EURUSD").await.unwrap().spread, Some(8));
}

#[tokio::test]
async fn swaps_are_realistic_per_market() {
    let eur = symbol_at(at(1, 12, 0, 0), "EURUSD").await;
    assert_eq!(eur.swap_mode.as_deref(), Some("points"));
    // A long on EURUSD pays the swap, a short earns it.
    assert!(eur.swap_long.unwrap() < 0.0 && eur.swap_short.unwrap() > 0.0);
    let btc = symbol_at(at(1, 12, 0, 0), "BTCUSD").await;
    assert_eq!(btc.swap_mode.as_deref(), Some("currency_symbol"));
    let us100 = symbol_at(at(1, 12, 0, 0), "US100").await;
    assert_eq!(us100.swap_mode.as_deref(), Some("interest_current"));
}

#[tokio::test]
async fn unknown_symbol_is_still_4301() {
    let mock = MockBridge::start().await;
    let client = BridgeClient::new(mock.config(TIMEOUT)).expect("client");
    match client.get_symbol("NOPE").await {
        Err(BridgeError::Remote { retcode, .. }) => assert_eq!(retcode, UNKNOWN_SYMBOL),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn fields_set_in_the_state_are_not_overwritten() {
    let mut state = state_at(at(1, 12, 0, 0));
    let gold = state
        .symbols
        .iter_mut()
        .find(|s| s.symbol == "XAUUSD")
        .unwrap();
    gold.spread = Some(999);
    gold.swap_mode = Some("reopen_bid".to_owned());
    gold.trade_mode = Some(SymbolTradeMode::LongOnly);
    gold.sessions = Some(Sessions {
        quote: vec![[0, 100]],
        trade: vec![[0, 100]],
    });
    let info = symbol_in(state, "XAUUSD").await;
    assert_eq!(info.spread, Some(999));
    assert_eq!(info.swap_mode.as_deref(), Some("reopen_bid"));
    assert_eq!(info.trade_mode, Some(SymbolTradeMode::LongOnly));
    // Noon is outside the pinned session of the first 100 seconds.
    assert!(!open_now(&info));
}

#[tokio::test]
async fn close_only_keeps_its_sessions_and_disabled_is_always_closed() {
    let mut state = state_at(at(1, 12, 0, 0));
    let eur = state
        .symbols
        .iter_mut()
        .find(|s| s.symbol == "EURUSD")
        .unwrap();
    eur.trade_allowed = false;
    let info = symbol_in(state.clone(), "EURUSD").await;
    assert_eq!(info.trade_mode, Some(SymbolTradeMode::CloseOnly));
    assert!(open_now(&info));

    let eur = state
        .symbols
        .iter_mut()
        .find(|s| s.symbol == "EURUSD")
        .unwrap();
    eur.trade_mode = Some(SymbolTradeMode::Disabled);
    let info = symbol_in(state, "EURUSD").await;
    assert!(!trade(&info).is_empty());
    assert!(!open_now(&info));
}

// ---- sessions at fixed clocks ----------------------------------------------------------

#[tokio::test]
async fn forex_trades_around_the_clock_except_a_break_at_the_rollover() {
    for (clock, expect_open, label) in [
        (at(0, 0, 2, 0), false, "Monday 00:02, in the break"),
        (at(0, 0, 5, 0), true, "Monday 00:05, trading starts"),
        (at(2, 12, 0, 0), true, "Wednesday noon"),
        (at(4, 23, 54, 59), true, "Friday 23:54:59"),
        (at(4, 23, 55, 0), false, "Friday 23:55, trading ends"),
    ] {
        let info = symbol_at(clock, "EURUSD").await;
        assert_eq!(open_now(&info), expect_open, "{label}");
        let sessions = info.sessions.unwrap();
        // Quotes run a minute past the trading hours on each side.
        assert_eq!(sessions.trade, vec![[300, DAY - 300]], "{label}");
        assert_eq!(sessions.quote, vec![[60, DAY - 60]], "{label}");
    }
}

#[tokio::test]
async fn weekend_has_no_sessions_for_fx_gold_and_indices() {
    for name in ["EURUSD", "GBPUSD", "XAUUSD", "US100", "US30"] {
        for day in [5, 6] {
            let info = symbol_at(at(day, 12, 0, 0), name).await;
            let sessions = info.sessions.clone().unwrap();
            assert!(
                sessions.quote.is_empty() && sessions.trade.is_empty(),
                "{name} day {day}"
            );
            assert!(!open_now(&info), "{name} day {day}");
        }
    }
}

#[tokio::test]
async fn gold_has_two_sessions_with_a_one_hour_break_and_the_last_ends_at_24h() {
    let info = symbol_at(at(1, 12, 0, 0), "XAUUSD").await;
    assert_eq!(trade(&info), vec![[0, 22 * 3600], [23 * 3600, DAY]]);
    assert!(open_now(&info));
    // 22:00 to 23:00 is closed; the second session includes the last second of the day.
    assert!(!open_now(&symbol_at(at(1, 22, 0, 0), "XAUUSD").await));
    assert!(!open_now(&symbol_at(at(1, 22, 59, 59), "XAUUSD").await));
    assert!(open_now(&symbol_at(at(1, 23, 0, 0), "XAUUSD").await));
    assert!(open_now(&symbol_at(at(1, 23, 59, 59), "XAUUSD").await));
}

#[tokio::test]
async fn gold_closes_for_the_week_at_22h_on_friday() {
    let friday = symbol_at(at(4, 12, 0, 0), "XAUUSD").await;
    assert_eq!(trade(&friday), vec![[0, 22 * 3600]]);
    assert!(open_now(&friday));
    assert!(!open_now(&symbol_at(at(4, 23, 30, 0), "XAUUSD").await));
}

#[tokio::test]
async fn indices_follow_cash_hours() {
    for name in ["US100", "US30"] {
        for (clock, expect, label) in [
            (at(1, 14, 29, 59), false, "one second before the open"),
            (at(1, 14, 30, 0), true, "the open"),
            (at(1, 20, 59, 59), true, "last second"),
            (at(1, 21, 0, 0), false, "the close"),
        ] {
            let info = symbol_at(clock, name).await;
            assert_eq!(open_now(&info), expect, "{name} {label}");
            assert_eq!(trade(&info), vec![[52_200, 75_600]], "{name}");
        }
    }
}

#[tokio::test]
async fn crypto_trades_every_day_all_day() {
    for day in 0..7 {
        let info = symbol_at(at(day, 3, 0, 0), "BTCUSD").await;
        let sessions = info.sessions.clone().unwrap();
        assert_eq!(sessions.trade, vec![[0, DAY]], "day {day}");
        assert_eq!(sessions.quote, vec![[0, DAY]], "day {day}");
        assert!(open_now(&info), "day {day}");
    }
    assert!(open_now(&symbol_at(at(6, 23, 59, 59), "BTCUSD").await));
}

#[tokio::test]
async fn live_fields_move_with_the_clock_between_two_calls() {
    let mock = MockBridge::start_with(state_at(at(1, 12, 0, 0))).await;
    let client = BridgeClient::new(mock.config(TIMEOUT)).expect("client");
    assert!(open_now(&client.get_symbol("EURUSD").await.unwrap()));
    mock.update_state(|s| s.clock = Some(at(5, 12, 0, 0)));
    assert!(!open_now(&client.get_symbol("EURUSD").await.unwrap()));
}

// ---- session maths ---------------------------------------------------------------------

#[test]
fn a_session_includes_its_start_and_excludes_its_end() {
    let s = Sessions {
        quote: vec![],
        trade: vec![[3600, 7200]],
    };
    assert!(!s.trade_open_at(3599));
    assert!(s.trade_open_at(3600));
    assert!(s.trade_open_at(7199));
    assert!(!s.trade_open_at(7200));
}

#[test]
fn a_session_ending_at_24h_covers_the_last_second_of_the_day() {
    let s = Sessions {
        quote: vec![[0, DAY]],
        trade: vec![[82_800, DAY]],
    };
    assert!(s.trade_open_at(DAY - 1));
    assert!(s.quote_open_at(0) && s.quote_open_at(DAY - 1));
    // Second 86400 belongs to tomorrow.
    assert!(!s.trade_open_at(DAY));
}

#[test]
fn several_sessions_leave_gaps_between_them() {
    let s = Sessions {
        quote: vec![],
        trade: vec![[0, 3600], [7200, 10_800], [82_800, DAY]],
    };
    let open: Vec<bool> = [0, 3599, 3600, 7199, 7200, 10_799, 10_800, 82_799, 82_800]
        .into_iter()
        .map(|t| s.trade_open_at(t))
        .collect();
    assert_eq!(
        open,
        [true, true, false, false, true, true, false, false, true]
    );
}

#[test]
fn no_session_today_is_never_open() {
    let s = Sessions::default();
    assert!(s.quote.is_empty() && s.trade.is_empty());
    assert!(!s.trade_open_at(0) && !s.trade_open_at(43_200) && !s.quote_open_at(43_200));
}

#[test]
fn quote_and_trade_sessions_are_independent() {
    let s = Sessions {
        quote: vec![[0, 7200]],
        trade: vec![[3600, 5400]],
    };
    assert!(s.quote_open_at(1800) && !s.trade_open_at(1800));
    assert!(s.quote_open_at(4000) && s.trade_open_at(4000));
}
