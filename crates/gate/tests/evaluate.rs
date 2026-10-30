//! `evaluate`: every limit rule (pass and fail), every action type, rounding, side and
//! stops validation, batch detection and est_pnl numbers (SPEC rules 2, 4, 5, 6).

use std::collections::HashMap;

use jarvis_bridge_client::protocol::{
    Account, Command, ExecType, JARVIS_MAGIC, Order, OrderCancelArgs, OrderSendArgs, OrderType,
    Position, PositionCloseArgs, PositionClosePartialArgs, PositionModifyArgs, Price, Side,
    SymbolInfo, TradeMode,
};
use jarvis_gate::{
    ActionPreview, CancelSelector, CloseSelector, GateError, Limits, MarketSnapshot, ModifyIntent,
    OrderIntent, ProposedAction, TradingHours, Weekday, evaluate,
};

/// 2026-11-02 00:00 (a Monday).
const MON: i64 = 1_793_577_600;
const NOON: i64 = MON + 12 * 3600;

// ---- fixtures ---------------------------------------------------------------------

fn gold_info() -> SymbolInfo {
    SymbolInfo {
        symbol: "XAUUSD".into(),
        description: "Gold".into(),
        digits: 2,
        point: 0.01,
        volume_min: 0.01,
        volume_max: 100.0,
        volume_step: 0.01,
        contract_size: 100.0,
        tick_value: 1.0,
        tick_size: 0.01,
        stops_level: 10,
        trade_allowed: true,
        currency_profit: "USD".into(),
    }
}

fn eur_info() -> SymbolInfo {
    SymbolInfo {
        symbol: "EURUSD".into(),
        description: "Euro".into(),
        digits: 5,
        point: 0.00001,
        volume_min: 0.01,
        volume_max: 100.0,
        volume_step: 0.01,
        contract_size: 100_000.0,
        tick_value: 1.0,
        tick_size: 0.00001,
        stops_level: 0,
        trade_allowed: true,
        currency_profit: "USD".into(),
    }
}

fn price(symbol: &str, bid: f64, ask: f64, spread_points: i64, digits: u32) -> Price {
    Price {
        symbol: symbol.into(),
        bid,
        ask,
        last: 0.0,
        spread_points,
        digits,
        time: NOON,
    }
}

fn account() -> Account {
    Account {
        currency: "USD".into(),
        balance: 10_000.0,
        equity: 10_000.0,
        profit: 0.0,
        margin: 0.0,
        free_margin: 10_000.0,
        margin_level: 0.0,
        leverage: 100,
        trade_mode: TradeMode::Demo,
    }
}

fn snap() -> MarketSnapshot {
    let mut prices = HashMap::new();
    prices.insert(
        "XAUUSD".to_owned(),
        price("XAUUSD", 4000.00, 4000.40, 40, 2),
    );
    prices.insert(
        "EURUSD".to_owned(),
        price("EURUSD", 1.10000, 1.10010, 10, 5),
    );
    let mut symbols = HashMap::new();
    symbols.insert("XAUUSD".to_owned(), gold_info());
    symbols.insert("EURUSD".to_owned(), eur_info());
    MarketSnapshot {
        account: account(),
        positions: vec![],
        orders: vec![],
        prices,
        symbols,
        server_time: NOON,
        realized_pnl_today: 0.0,
    }
}

fn limits() -> Limits {
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

fn pos(ticket: u64, symbol: &str, side: Side, volume: f64, open: f64, profit: f64) -> Position {
    Position {
        ticket,
        symbol: symbol.into(),
        side,
        volume,
        price_open: open,
        price_current: open,
        sl: None,
        tp: None,
        profit,
        swap: 0.0,
        magic: 0,
        comment: String::new(),
        time: NOON - 3600,
    }
}

fn pending(ticket: u64, symbol: &str, kind: OrderType, volume: f64, at: f64) -> Order {
    Order {
        ticket,
        symbol: symbol.into(),
        order_type: kind,
        volume_initial: volume,
        volume_current: volume,
        price_open: at,
        sl: None,
        tp: None,
        time_setup: NOON - 3600,
        comment: String::new(),
    }
}

fn buy_order(volume: f64, sl: Option<f64>, tp: Option<f64>) -> ProposedAction {
    ProposedAction::Order(OrderIntent {
        symbol: "XAUUSD".into(),
        side: Side::Buy,
        volume,
        exec: ExecType::Market,
        price: None,
        sl,
        tp,
    })
}

fn order(
    symbol: &str,
    side: Side,
    exec: ExecType,
    volume: f64,
    price: Option<f64>,
    sl: Option<f64>,
    tp: Option<f64>,
) -> ProposedAction {
    ProposedAction::Order(OrderIntent {
        symbol: symbol.into(),
        side,
        volume,
        exec,
        price,
        sl,
        tp,
    })
}

fn ok(action: &ProposedAction, s: &MarketSnapshot) -> ActionPreview {
    evaluate(action, s, &limits(), false).unwrap_or_else(|e| panic!("expected a preview: {e}"))
}

fn err(action: &ProposedAction, s: &MarketSnapshot) -> GateError {
    evaluate(action, s, &limits(), false).expect_err("expected the gate to refuse")
}

fn is_invalid(e: &GateError) -> bool {
    matches!(e, GateError::Invalid(_))
}

fn order_args(p: &ActionPreview) -> &OrderSendArgs {
    match p.commands.as_slice() {
        [Command::OrderSend(a)] => a,
        other => panic!("expected one OrderSend, got {other:?}"),
    }
}

// ---- kill switch ------------------------------------------------------------------

#[test]
fn kill_switch_blocks_every_action_type() {
    let mut s = snap();
    s.positions
        .push(pos(1, "XAUUSD", Side::Buy, 0.1, 3990.0, 5.0));
    s.orders
        .push(pending(2, "EURUSD", OrderType::BuyLimit, 0.1, 1.09));
    let actions = [
        buy_order(0.1, None, None),
        ProposedAction::Modify(ModifyIntent {
            ticket: 1,
            sl: Some(3990.0),
            tp: None,
        }),
        ProposedAction::Close(CloseSelector {
            tickets: vec![1],
            ..Default::default()
        }),
        ProposedAction::Cancel(CancelSelector {
            tickets: vec![2],
            ..Default::default()
        }),
    ];
    for a in &actions {
        assert_eq!(
            evaluate(a, &s, &limits(), true),
            Err(GateError::KillSwitchEngaged),
            "{a:?}"
        );
    }
}

#[test]
fn kill_switch_wins_over_every_other_error() {
    // symbol not allowed, unknown ticket, ambiguous close: all would fail anyway
    let s = snap();
    let bad_symbol = order(
        "DOGEUSD",
        Side::Buy,
        ExecType::Market,
        50.0,
        None,
        None,
        None,
    );
    let unknown_ticket = ProposedAction::Modify(ModifyIntent {
        ticket: 9,
        sl: None,
        tp: None,
    });
    let ambiguous = ProposedAction::Close(CloseSelector::default());
    for a in [bad_symbol, unknown_ticket, ambiguous] {
        assert_eq!(
            evaluate(&a, &s, &limits(), true),
            Err(GateError::KillSwitchEngaged)
        );
    }
}

// ---- new orders: happy paths ------------------------------------------------------

#[test]
fn market_buy_preview_full_shape() {
    let s = snap();
    let p = ok(&buy_order(0.1, Some(3990.0), Some(4020.0)), &s);
    assert_eq!(
        p.description,
        "Buy 0.10 XAUUSD at market, SL 3990.00, TP 4020.00"
    );
    assert_eq!(p.symbols, vec!["XAUUSD".to_owned()]);
    assert!(p.tickets.is_empty());
    assert_eq!(p.total_volume, 0.1);
    assert_eq!(p.count, 1);
    assert!(!p.is_batch);
    let a = order_args(&p);
    assert_eq!(a.symbol, "XAUUSD");
    assert_eq!(a.side, Side::Buy);
    assert_eq!(a.volume, 0.1);
    assert_eq!(a.exec, ExecType::Market);
    assert_eq!(a.price, None);
    assert_eq!(a.sl, Some(3990.0));
    assert_eq!(a.tp, Some(4020.0));
    assert_eq!(a.deviation_points, None);
    assert_eq!(a.magic, JARVIS_MAGIC);
}

#[test]
fn order_comment_carries_first_eight_hex_of_preview_id() {
    let p = ok(&buy_order(0.1, Some(3990.0), None), &snap());
    let a = order_args(&p);
    let expected = format!("jarvis:{}", &p.id.simple().to_string()[..8]);
    assert_eq!(a.comment, expected);
    assert!(a.comment.len() <= 31);
}

#[test]
fn preview_ids_are_unique() {
    let s = snap();
    let a = ok(&buy_order(0.1, None, None), &s);
    let b = ok(&buy_order(0.1, None, None), &s);
    assert_ne!(a.id, b.id);
}

#[test]
fn market_buy_est_pnl_uses_ask_as_entry() {
    let p = ok(&buy_order(0.1, Some(3990.0), Some(4020.0)), &snap());
    // entry 4000.40: (3990 - 4000.40) / 0.01 * 1.0 * 0.1 = -104; (4020 - 4000.40) -> +196
    assert_eq!(p.est_pnl.at_sl, Some(-104.0));
    assert_eq!(p.est_pnl.at_tp, Some(196.0));
    assert_eq!(p.est_pnl.current, None);
}

#[test]
fn market_sell_est_pnl_uses_bid_as_entry() {
    let a = order(
        "XAUUSD",
        Side::Sell,
        ExecType::Market,
        0.1,
        None,
        Some(4010.0),
        Some(3980.0),
    );
    let p = ok(&a, &snap());
    // entry 4000.00: -100 at SL, +200 at TP
    assert_eq!(
        p.description,
        "Sell 0.10 XAUUSD at market, SL 4010.00, TP 3980.00"
    );
    assert_eq!(p.est_pnl.at_sl, Some(-100.0));
    assert_eq!(p.est_pnl.at_tp, Some(200.0));
}

#[test]
fn est_pnl_is_none_without_sl_or_tp() {
    let p = ok(&buy_order(0.1, None, None), &snap());
    assert_eq!(p.est_pnl.at_sl, None);
    assert_eq!(p.est_pnl.at_tp, None);
    let p = ok(&buy_order(0.1, Some(3990.0), None), &snap());
    assert!(p.est_pnl.at_sl.is_some());
    assert_eq!(p.est_pnl.at_tp, None);
}

#[test]
fn est_pnl_scales_with_tick_value_and_volume() {
    let mut s = snap();
    s.symbols.get_mut("EURUSD").unwrap().tick_value = 0.91;
    // buy limit 1.09000, TP 1.10000: +1000 ticks * 0.91 * 0.5 lots = 455.00
    let a = order(
        "EURUSD",
        Side::Buy,
        ExecType::Limit,
        0.5,
        Some(1.09000),
        Some(1.08000),
        Some(1.10000),
    );
    let p = ok(&a, &s);
    assert_eq!(p.est_pnl.at_tp, Some(455.0));
    assert_eq!(p.est_pnl.at_sl, Some(-455.0));
}

#[test]
fn est_pnl_rounds_to_two_decimals() {
    let mut s = snap();
    s.symbols.get_mut("EURUSD").unwrap().tick_value = 0.7777;
    let a = order(
        "EURUSD",
        Side::Buy,
        ExecType::Limit,
        0.03,
        Some(1.09000),
        Some(1.08990),
        None,
    );
    let p = ok(&a, &s);
    // -10 ticks * 0.7777 * 0.03 = -0.23331 -> -0.23
    assert_eq!(p.est_pnl.at_sl, Some(-0.23));
}

#[test]
fn pending_orders_descriptions_and_commands() {
    let s = snap();
    let a = order(
        "EURUSD",
        Side::Buy,
        ExecType::Limit,
        0.1,
        Some(1.09),
        Some(1.085),
        None,
    );
    let p = ok(&a, &s);
    assert_eq!(
        p.description,
        "Buy limit 0.10 EURUSD at 1.09000, SL 1.08500"
    );
    let args = order_args(&p);
    assert_eq!(args.exec, ExecType::Limit);
    assert_eq!(args.price, Some(1.09));
    assert_eq!(args.sl, Some(1.085));

    let a = order(
        "XAUUSD",
        Side::Sell,
        ExecType::Stop,
        0.2,
        Some(3990.0),
        None,
        Some(3950.0),
    );
    let p = ok(&a, &s);
    assert_eq!(
        p.description,
        "Sell stop 0.20 XAUUSD at 3990.00, TP 3950.00"
    );
}

#[test]
fn description_without_sl_tp_has_no_trailing_clauses() {
    let a = order(
        "XAUUSD",
        Side::Sell,
        ExecType::Market,
        0.1,
        None,
        None,
        None,
    );
    assert_eq!(ok(&a, &snap()).description, "Sell 0.10 XAUUSD at market");
}

// ---- new orders: rounding ---------------------------------------------------------

#[test]
fn volume_is_rounded_down_to_step_floating_safe() {
    // (requested, expected after rounding down to 0.01)
    let cases = [
        (0.1, 0.1),
        (0.07, 0.07),
        (0.29, 0.29),
        (0.57, 0.57),
        (0.057, 0.05),
        (0.019999, 0.01),
        (0.1 + 0.2, 0.3),
        (0.999, 0.99),
        (1.0, 1.0),
        (0.58, 0.58),
    ];
    for (requested, expected) in cases {
        let p = ok(&buy_order(requested, None, None), &snap());
        let sent = order_args(&p).volume;
        assert_eq!(sent, expected, "requested {requested}");
        assert_eq!(p.total_volume, expected);
    }
}

#[test]
fn volume_rounding_down_adds_a_warning() {
    let p = ok(&buy_order(0.057, None, None), &snap());
    assert!(
        p.warnings.iter().any(|w| w.contains("rounded down")),
        "{:?}",
        p.warnings
    );
    let p = ok(&buy_order(0.05, None, None), &snap());
    assert!(
        !p.warnings.iter().any(|w| w.contains("rounded")),
        "{:?}",
        p.warnings
    );
}

#[test]
fn coarse_volume_step_is_respected() {
    let mut s = snap();
    {
        let i = s.symbols.get_mut("XAUUSD").unwrap();
        i.volume_step = 0.5;
        i.volume_min = 0.5;
    }
    let p = ok(&buy_order(0.99, None, None), &s);
    assert_eq!(order_args(&p).volume, 0.5);
    assert!(is_invalid(&err(&buy_order(0.4, None, None), &s)));
}

#[test]
fn prices_are_rounded_to_digits() {
    let a = order(
        "XAUUSD",
        Side::Buy,
        ExecType::Limit,
        0.1,
        Some(3990.004),
        Some(3979.996),
        Some(4019.999),
    );
    let p = ok(&a, &snap());
    let args = order_args(&p);
    assert_eq!(args.price, Some(3990.0));
    assert_eq!(args.sl, Some(3980.0));
    assert_eq!(args.tp, Some(4020.0));
    assert_eq!(
        p.description,
        "Buy limit 0.10 XAUUSD at 3990.00, SL 3980.00, TP 4020.00"
    );
}

// ---- new orders: volume rules -----------------------------------------------------

#[test]
fn volume_below_min_is_invalid() {
    for v in [0.009, 0.0, 0.001] {
        assert!(
            is_invalid(&err(&buy_order(v, None, None), &snap())),
            "volume {v}"
        );
    }
}

#[test]
fn volume_must_be_finite_and_positive() {
    for v in [-0.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            is_invalid(&err(&buy_order(v, None, None), &snap())),
            "volume {v}"
        );
    }
}

#[test]
fn volume_above_broker_max_is_invalid() {
    let mut s = snap();
    s.symbols.get_mut("XAUUSD").unwrap().volume_max = 0.5;
    let mut l = limits();
    l.max_lot = 10.0;
    let e = evaluate(&buy_order(0.6, None, None), &s, &l, false).unwrap_err();
    assert!(is_invalid(&e), "{e:?}");
    assert!(evaluate(&buy_order(0.5, None, None), &s, &l, false).is_ok());
}

#[test]
fn max_lot_boundary() {
    let s = snap();
    assert!(evaluate(&buy_order(1.0, None, None), &s, &limits(), false).is_ok());
    match err(&buy_order(1.01, None, None), &s) {
        GateError::LimitExceeded { rule, detail } => {
            assert_eq!(rule, "max_lot");
            assert!(
                detail.contains("1.01") && detail.contains("1.00"),
                "{detail}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn max_lot_uses_the_rounded_volume() {
    // 1.004 rounds down to 1.00 which is within max_lot 1.0
    assert!(evaluate(&buy_order(1.004, None, None), &snap(), &limits(), false).is_ok());
}

// ---- new orders: allowed symbols --------------------------------------------------

#[test]
fn symbol_not_in_allowed_list_is_refused() {
    let mut s = snap();
    s.symbols.insert(
        "US30".into(),
        SymbolInfo {
            symbol: "US30".into(),
            ..gold_info()
        },
    );
    s.prices
        .insert("US30".into(), price("US30", 40000.0, 40001.0, 100, 2));
    let a = order("US30", Side::Buy, ExecType::Market, 0.1, None, None, None);
    match err(&a, &s) {
        GateError::LimitExceeded { rule, detail } => {
            assert_eq!(rule, "allowed_symbols");
            assert!(
                detail.contains("US30") && detail.contains("XAUUSD"),
                "{detail}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn allowed_symbols_match_is_case_sensitive() {
    let a = order("xauusd", Side::Buy, ExecType::Market, 0.1, None, None, None);
    assert!(matches!(
        err(&a, &snap()),
        GateError::LimitExceeded {
            rule: "allowed_symbols",
            ..
        }
    ));
}

#[test]
fn empty_allowed_symbols_fails_closed() {
    let mut l = limits();
    l.allowed_symbols.clear();
    let e = evaluate(&buy_order(0.1, None, None), &snap(), &l, false).unwrap_err();
    assert!(matches!(
        e,
        GateError::LimitExceeded {
            rule: "allowed_symbols",
            ..
        }
    ));
}

// ---- new orders: snapshot completeness / tradability ------------------------------

#[test]
fn missing_symbol_info_or_price_is_invalid() {
    let mut no_info = snap();
    no_info.symbols.remove("XAUUSD");
    assert!(is_invalid(&err(&buy_order(0.1, None, None), &no_info)));
    let mut no_price = snap();
    no_price.prices.remove("XAUUSD");
    assert!(is_invalid(&err(&buy_order(0.1, None, None), &no_price)));
}

#[test]
fn symbol_not_open_for_trading_is_invalid() {
    let mut s = snap();
    s.symbols.get_mut("XAUUSD").unwrap().trade_allowed = false;
    let e = err(&buy_order(0.1, None, None), &s);
    assert!(is_invalid(&e));
    assert!(e.to_string().contains("not open for trading"), "{e}");
}

#[test]
fn zero_tick_size_is_invalid() {
    let mut s = snap();
    s.symbols.get_mut("XAUUSD").unwrap().tick_size = 0.0;
    assert!(is_invalid(&err(&buy_order(0.1, Some(3990.0), None), &s)));
}

// ---- new orders: max_positions ----------------------------------------------------

#[test]
fn max_positions_counts_positions_and_orders_plus_new() {
    // limit is 5: 4 existing + 1 new = 5 is fine, 5 existing + 1 = 6 is not
    let mut s = snap();
    s.positions = (1..=3)
        .map(|t| pos(t, "XAUUSD", Side::Buy, 0.1, 3990.0, 0.0))
        .collect();
    s.orders = vec![pending(10, "EURUSD", OrderType::BuyLimit, 0.1, 1.09)];
    assert!(evaluate(&buy_order(0.1, None, None), &s, &limits(), false).is_ok());

    s.orders
        .push(pending(11, "EURUSD", OrderType::SellLimit, 0.1, 1.12));
    match err(&buy_order(0.1, None, None), &s) {
        GateError::LimitExceeded { rule, detail } => {
            assert_eq!(rule, "max_positions");
            assert!(detail.contains('5'), "{detail}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn max_positions_of_one_allows_first_order_only() {
    let mut l = limits();
    l.max_positions = 1;
    let mut s = snap();
    assert!(evaluate(&buy_order(0.1, None, None), &s, &l, false).is_ok());
    s.orders
        .push(pending(10, "EURUSD", OrderType::BuyLimit, 0.1, 1.09));
    assert!(evaluate(&buy_order(0.1, None, None), &s, &l, false).is_err());
}

// ---- new orders: trading hours ----------------------------------------------------

#[test]
fn trading_hours_gate_new_orders() {
    // (server time, allowed)
    let cases = [
        (MON + 12 * 3600, true),
        (MON + 5 * 60, true),                  // Mon 00:05 start inclusive
        (MON + 4 * 60, false),                 // Mon 00:04
        (MON + 23 * 3600 + 54 * 60, true),     // 23:54
        (MON + 23 * 3600 + 55 * 60, false),    // 23:55 end exclusive
        (MON + 5 * 86_400 + 12 * 3600, false), // Saturday noon
        (MON + 6 * 86_400 + 12 * 3600, false), // Sunday noon
    ];
    for (t, allowed) in cases {
        let mut s = snap();
        s.server_time = t;
        let r = evaluate(&buy_order(0.1, None, None), &s, &limits(), false);
        if allowed {
            assert!(r.is_ok(), "t={t}: {r:?}");
        } else {
            assert!(
                matches!(
                    r,
                    Err(GateError::LimitExceeded {
                        rule: "trading_hours",
                        ..
                    })
                ),
                "t={t}: {r:?}"
            );
        }
    }
}

#[test]
fn no_trading_hours_means_always_open() {
    let mut l = limits();
    l.trading_hours = None;
    let mut s = snap();
    s.server_time = MON + 6 * 86_400 + 3 * 3600;
    assert!(evaluate(&buy_order(0.1, None, None), &s, &l, false).is_ok());
}

// ---- new orders: daily loss -------------------------------------------------------

#[test]
fn daily_loss_blocks_new_orders_at_or_below_the_limit() {
    // (realised today, floating profits, blocked)
    let cases: [(f64, &[f64], bool); 8] = [
        (0.0, &[], false),
        (-499.99, &[], false),
        (-500.0, &[], true),
        (-600.0, &[], true),
        (-300.0, &[-200.0], true),
        (-300.0, &[-100.0, -99.0], false),
        (-600.0, &[200.0], false),
        (100.0, &[-600.0], true),
    ];
    for (realised, floating, blocked) in cases {
        let mut s = snap();
        s.realized_pnl_today = realised;
        s.positions = floating
            .iter()
            .enumerate()
            .map(|(i, pl)| pos(i as u64 + 1, "XAUUSD", Side::Buy, 0.1, 3990.0, *pl))
            .collect();
        let r = evaluate(&buy_order(0.1, None, None), &s, &limits(), false);
        if blocked {
            match r {
                Err(GateError::LimitExceeded { rule, detail }) => {
                    assert_eq!(rule, "max_daily_loss");
                    assert!(detail.contains("USD"), "{detail}");
                }
                other => panic!("realised {realised} floating {floating:?}: {other:?}"),
            }
        } else {
            assert!(
                r.is_ok(),
                "realised {realised} floating {floating:?}: {r:?}"
            );
        }
    }
}

// ---- new orders: evaluation order -------------------------------------------------

#[test]
fn rule_precedence_symbol_then_snapshot_then_volume_then_positions_hours_loss() {
    // Everything wrong at once: the first rule in the documented order is reported.
    let mut s = snap();
    s.server_time = MON + 5 * 86_400; // Saturday
    s.realized_pnl_today = -1000.0;
    s.positions = (1..=5)
        .map(|t| pos(t, "XAUUSD", Side::Buy, 0.1, 3990.0, 0.0))
        .collect();

    let too_big = buy_order(5.0, None, None);
    assert!(matches!(
        err(&too_big, &s),
        GateError::LimitExceeded {
            rule: "max_lot",
            ..
        }
    ));

    let ok_volume = buy_order(0.5, None, None);
    assert!(matches!(
        err(&ok_volume, &s),
        GateError::LimitExceeded {
            rule: "max_positions",
            ..
        }
    ));

    s.positions.clear();
    assert!(matches!(
        err(&ok_volume, &s),
        GateError::LimitExceeded {
            rule: "trading_hours",
            ..
        }
    ));

    s.server_time = NOON;
    assert!(matches!(
        err(&ok_volume, &s),
        GateError::LimitExceeded {
            rule: "max_daily_loss",
            ..
        }
    ));

    let bad_symbol = order(
        "DOGEUSD",
        Side::Buy,
        ExecType::Market,
        5.0,
        None,
        None,
        None,
    );
    assert!(matches!(
        err(&bad_symbol, &s),
        GateError::LimitExceeded {
            rule: "allowed_symbols",
            ..
        }
    ));
}

#[test]
fn limits_are_checked_before_price_validation() {
    // The SL is on the wrong side AND the volume exceeds max_lot: the limit is reported.
    let e = err(&buy_order(5.0, Some(4100.0), None), &snap());
    assert!(
        matches!(
            e,
            GateError::LimitExceeded {
                rule: "max_lot",
                ..
            }
        ),
        "{e:?}"
    );
}

// ---- new orders: price / pending validation --------------------------------------

#[test]
fn market_orders_reject_a_price() {
    let a = order(
        "XAUUSD",
        Side::Buy,
        ExecType::Market,
        0.1,
        Some(4000.0),
        None,
        None,
    );
    assert!(is_invalid(&err(&a, &snap())));
}

#[test]
fn pending_orders_require_a_price() {
    for exec in [ExecType::Limit, ExecType::Stop] {
        let a = order("XAUUSD", Side::Buy, exec, 0.1, None, None, None);
        assert!(is_invalid(&err(&a, &snap())), "{exec:?}");
    }
}

#[test]
fn pending_price_side_table() {
    // gold: bid 4000.00, ask 4000.40, stops level 0.10
    // (side, exec, price, valid)
    let cases = [
        (Side::Buy, ExecType::Limit, 3990.0, true),
        (Side::Buy, ExecType::Limit, 4000.30, true), // exactly stops_level below ask
        (Side::Buy, ExecType::Limit, 4000.35, false), // closer than stops_level
        (Side::Buy, ExecType::Limit, 4000.40, false), // at the ask
        (Side::Buy, ExecType::Limit, 4005.0, false), // above the ask
        (Side::Sell, ExecType::Limit, 4010.0, true),
        (Side::Sell, ExecType::Limit, 4000.10, true),
        (Side::Sell, ExecType::Limit, 4000.0, false),
        (Side::Sell, ExecType::Limit, 3990.0, false),
        (Side::Buy, ExecType::Stop, 4010.0, true),
        (Side::Buy, ExecType::Stop, 4000.50, true),
        (Side::Buy, ExecType::Stop, 4000.40, false),
        (Side::Buy, ExecType::Stop, 3990.0, false),
        (Side::Sell, ExecType::Stop, 3990.0, true),
        (Side::Sell, ExecType::Stop, 3999.90, true),
        (Side::Sell, ExecType::Stop, 4000.0, false),
        (Side::Sell, ExecType::Stop, 4010.0, false),
    ];
    for (side, exec, p, valid) in cases {
        let a = order("XAUUSD", side, exec, 0.1, Some(p), None, None);
        let r = evaluate(&a, &snap(), &limits(), false);
        if valid {
            assert!(r.is_ok(), "{side:?} {exec:?} {p}: {r:?}");
        } else {
            assert!(
                matches!(r, Err(GateError::Invalid(_))),
                "{side:?} {exec:?} {p}: {r:?}"
            );
        }
    }
}

#[test]
fn sl_tp_side_and_distance_table_market() {
    // gold: bid 4000.00, ask 4000.40, stops level 0.10 (measured from the bid for a
    // buy, from the ask for a sell; the side check is against the entry price)
    // (side, sl, tp, valid)
    let cases = [
        (Side::Buy, Some(3990.0), Some(4020.0), true),
        (Side::Buy, Some(3999.90), None, true), // 0.10 below bid
        (Side::Buy, Some(3999.95), None, false), // inside stops level
        (Side::Buy, Some(4000.40), None, false), // SL at entry
        (Side::Buy, Some(4010.0), None, false), // SL above entry
        (Side::Buy, None, Some(4000.50), true),
        (Side::Buy, None, Some(4000.40), false), // TP at entry
        (Side::Buy, None, Some(3990.0), false),  // TP below entry
        (Side::Buy, Some(3990.0), Some(3980.0), false),
        (Side::Sell, Some(4010.0), Some(3980.0), true),
        (Side::Sell, Some(4000.50), None, true), // 0.10 above ask
        (Side::Sell, Some(4000.45), None, false),
        (Side::Sell, Some(4000.0), None, false), // SL at entry
        (Side::Sell, Some(3990.0), None, false), // SL below entry
        (Side::Sell, None, Some(4000.0), false), // TP at entry
        (Side::Sell, None, Some(4010.0), false), // TP above entry
        (Side::Sell, None, Some(3999.90), true),
    ];
    for (side, sl, tp, valid) in cases {
        let a = order("XAUUSD", side, ExecType::Market, 0.1, None, sl, tp);
        let r = evaluate(&a, &snap(), &limits(), false);
        if valid {
            assert!(r.is_ok(), "{side:?} sl={sl:?} tp={tp:?}: {r:?}");
        } else {
            assert!(
                matches!(r, Err(GateError::Invalid(_))),
                "{side:?} sl={sl:?} tp={tp:?}: {r:?}"
            );
        }
    }
}

#[test]
fn sl_tp_are_checked_against_the_pending_price() {
    // buy limit 3990: SL must be below 3990 even though it is below the market
    let bad = order(
        "XAUUSD",
        Side::Buy,
        ExecType::Limit,
        0.1,
        Some(3990.0),
        Some(3995.0),
        None,
    );
    assert!(is_invalid(&err(&bad, &snap())));
    let good = order(
        "XAUUSD",
        Side::Buy,
        ExecType::Limit,
        0.1,
        Some(3990.0),
        Some(3980.0),
        None,
    );
    assert!(evaluate(&good, &snap(), &limits(), false).is_ok());
    // sell stop 3990: TP must be below 3990
    let bad = order(
        "XAUUSD",
        Side::Sell,
        ExecType::Stop,
        0.1,
        Some(3990.0),
        None,
        Some(3995.0),
    );
    assert!(is_invalid(&err(&bad, &snap())));
}

#[test]
fn zero_sl_tp_mean_not_set_and_negative_or_nan_are_invalid() {
    let p = ok(&buy_order(0.1, Some(0.0), Some(0.0)), &snap());
    let a = order_args(&p);
    assert_eq!((a.sl, a.tp), (None, None));
    assert!(
        p.warnings.iter().any(|w| w.contains("stop loss")),
        "{:?}",
        p.warnings
    );
    for bad in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(
            is_invalid(&err(&buy_order(0.1, Some(bad), None), &snap())),
            "sl {bad}"
        );
        assert!(
            is_invalid(&err(&buy_order(0.1, None, Some(bad)), &snap())),
            "tp {bad}"
        );
    }
}

// ---- new orders: warnings ---------------------------------------------------------

#[test]
fn warns_when_there_is_no_stop_loss() {
    let p = ok(&buy_order(0.1, None, Some(4020.0)), &snap());
    assert!(
        p.warnings
            .iter()
            .any(|w| w.to_lowercase().contains("no stop loss")),
        "{:?}",
        p.warnings
    );
    let p = ok(&buy_order(0.1, Some(3990.0), None), &snap());
    assert!(
        !p.warnings
            .iter()
            .any(|w| w.to_lowercase().contains("no stop loss"))
    );
}

#[test]
fn warns_when_spread_is_wide() {
    let mut s = snap();
    s.prices.get_mut("XAUUSD").unwrap().spread_points = 51;
    let p = ok(&buy_order(0.1, Some(3990.0), None), &s);
    assert!(
        p.warnings.iter().any(|w| w.contains("51")),
        "{:?}",
        p.warnings
    );

    s.prices.get_mut("XAUUSD").unwrap().spread_points = 50;
    let p = ok(&buy_order(0.1, Some(3990.0), None), &s);
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
}

// ---- modify -----------------------------------------------------------------------

fn snap_with_buy() -> MarketSnapshot {
    let mut s = snap();
    s.positions
        .push(pos(101, "XAUUSD", Side::Buy, 0.1, 3990.0, 10.0));
    s
}

fn snap_with_sell() -> MarketSnapshot {
    let mut s = snap();
    s.positions
        .push(pos(202, "XAUUSD", Side::Sell, 0.2, 4010.0, 20.0));
    s
}

fn modify(ticket: u64, sl: Option<f64>, tp: Option<f64>) -> ProposedAction {
    ProposedAction::Modify(ModifyIntent { ticket, sl, tp })
}

#[test]
fn modify_buy_position_preview() {
    let p = ok(&modify(101, Some(3995.0), Some(4050.0)), &snap_with_buy());
    assert_eq!(
        p.description,
        "Set stop loss to 3995.00 and take profit to 4050.00 on your XAUUSD buy position"
    );
    assert_eq!(p.symbols, vec!["XAUUSD".to_owned()]);
    assert_eq!(p.tickets, vec![101]);
    assert_eq!(p.total_volume, 0.1);
    assert_eq!(p.count, 1);
    assert!(!p.is_batch);
    assert_eq!(
        p.commands,
        vec![Command::PositionModify(PositionModifyArgs {
            ticket: 101,
            sl: Some(3995.0),
            tp: Some(4050.0)
        })]
    );
    // entry 3990.00, 0.10 lots: SL +50, TP +600
    assert_eq!(p.est_pnl.at_sl, Some(50.0));
    assert_eq!(p.est_pnl.at_tp, Some(600.0));
}

#[test]
fn modify_sell_position_preview() {
    let p = ok(&modify(202, Some(4020.0), None), &snap_with_sell());
    assert_eq!(
        p.description,
        "Set stop loss to 4020.00 on your XAUUSD sell position"
    );
    // entry 4010, SL 4020 against a sell of 0.2 lots: -10 / 0.01 * 0.2 = -200
    assert_eq!(p.est_pnl.at_sl, Some(-200.0));
    assert_eq!(p.est_pnl.at_tp, None);
    assert_eq!(
        p.commands,
        vec![Command::PositionModify(PositionModifyArgs {
            ticket: 202,
            sl: Some(4020.0),
            tp: None
        })]
    );
}

#[test]
fn modify_unknown_ticket_is_invalid() {
    let e = err(&modify(999, Some(3995.0), None), &snap_with_buy());
    assert!(is_invalid(&e));
    assert!(e.to_string().contains("999"), "{e}");
}

#[test]
fn modify_needs_at_least_one_level() {
    assert!(is_invalid(&err(&modify(101, None, None), &snap_with_buy())));
}

#[test]
fn modify_validates_side_and_distance_for_buys() {
    // buy position closes at the bid 4000.00; stops level 0.10
    let cases = [
        (Some(3999.90), None, true),
        (Some(3999.95), None, false),
        (Some(4000.0), None, false),
        (Some(4010.0), None, false),
        (None, Some(4000.10), true),
        (None, Some(4000.05), false),
        (None, Some(4000.0), false),
        (None, Some(3990.0), false),
    ];
    for (sl, tp, valid) in cases {
        let r = evaluate(&modify(101, sl, tp), &snap_with_buy(), &limits(), false);
        assert_eq!(r.is_ok(), valid, "sl={sl:?} tp={tp:?}: {r:?}");
    }
}

#[test]
fn modify_validates_side_and_distance_for_sells() {
    // sell position closes at the ask 4000.40
    let cases = [
        (Some(4000.50), None, true),
        (Some(4000.45), None, false),
        (Some(4000.0), None, false),
        (Some(3990.0), None, false),
        (None, Some(4000.30), true),
        (None, Some(4000.35), false),
        (None, Some(4010.0), false),
    ];
    for (sl, tp, valid) in cases {
        let r = evaluate(&modify(202, sl, tp), &snap_with_sell(), &limits(), false);
        assert_eq!(r.is_ok(), valid, "sl={sl:?} tp={tp:?}: {r:?}");
    }
}

fn snap_with_buy_levels() -> MarketSnapshot {
    let mut s = snap_with_buy();
    s.positions[0].sl = Some(3980.0);
    s.positions[0].tp = Some(4100.0);
    s
}

#[test]
fn modify_zero_removes_levels() {
    let p = ok(&modify(101, Some(0.0), None), &snap_with_buy_levels());
    assert_eq!(
        p.description,
        "Remove stop loss on your XAUUSD buy position"
    );
    assert_eq!(p.est_pnl.at_sl, None);
    assert_eq!(
        p.commands,
        vec![Command::PositionModify(PositionModifyArgs {
            ticket: 101,
            sl: Some(0.0),
            tp: None
        })]
    );

    let p = ok(&modify(101, Some(0.0), Some(0.0)), &snap_with_buy_levels());
    assert_eq!(
        p.description,
        "Remove stop loss and take profit on your XAUUSD buy position"
    );

    let p = ok(
        &modify(101, Some(3995.0), Some(0.0)),
        &snap_with_buy_levels(),
    );
    assert_eq!(
        p.description,
        "Set stop loss to 3995.00 and remove take profit on your XAUUSD buy position"
    );
    assert_eq!(p.est_pnl.at_sl, Some(50.0));
    assert_eq!(p.est_pnl.at_tp, None);
}

#[test]
fn modify_rounds_levels_to_digits() {
    let p = ok(&modify(101, Some(3994.996), None), &snap_with_buy());
    assert_eq!(
        p.commands,
        vec![Command::PositionModify(PositionModifyArgs {
            ticket: 101,
            sl: Some(3995.0),
            tp: None
        })]
    );
}

#[test]
fn modify_rejects_negative_and_non_finite_levels() {
    for bad in [-5.0, f64::NAN, f64::INFINITY] {
        assert!(
            is_invalid(&err(&modify(101, Some(bad), None), &snap_with_buy())),
            "{bad}"
        );
    }
}

#[test]
fn modify_to_current_levels_is_nothing_to_do() {
    let mut s = snap_with_buy();
    s.positions[0].sl = Some(3995.0);
    s.positions[0].tp = Some(4050.0);
    let e = err(&modify(101, Some(3995.0), Some(4050.0)), &s);
    assert!(matches!(e, GateError::NothingToDo(_)), "{e:?}");
    // removing levels that are not set is a no-op too
    let e = err(&modify(101, None, Some(0.0)), &snap_with_buy());
    assert!(matches!(e, GateError::NothingToDo(_)), "{e:?}");
    // a real change on one of them is fine; the unchanged level is not re-sent
    let p = ok(&modify(101, Some(3996.0), Some(4050.0)), &s);
    assert_eq!(
        p.commands,
        vec![Command::PositionModify(PositionModifyArgs {
            ticket: 101,
            sl: Some(3996.0),
            tp: None
        })]
    );
    assert_eq!(
        p.description,
        "Set stop loss to 3996.00 on your XAUUSD buy position"
    );
}

#[test]
fn modify_does_not_revalidate_unchanged_levels() {
    // The existing SL is now closer to the market than stops_level allows (price moved),
    // but only the TP is being changed.
    let mut s = snap_with_buy();
    s.positions[0].sl = Some(3999.99);
    let p = ok(&modify(101, Some(3999.99), Some(4050.0)), &s);
    assert_eq!(
        p.commands,
        vec![Command::PositionModify(PositionModifyArgs {
            ticket: 101,
            sl: None,
            tp: Some(4050.0)
        })]
    );
}

#[test]
fn modify_needs_symbol_info_and_price() {
    let mut s = snap_with_buy();
    s.symbols.remove("XAUUSD");
    assert!(is_invalid(&err(&modify(101, Some(3995.0), None), &s)));
    let mut s = snap_with_buy();
    s.prices.remove("XAUUSD");
    assert!(is_invalid(&err(&modify(101, Some(3995.0), None), &s)));
}

#[test]
fn modify_is_not_blocked_by_daily_loss_hours_or_symbol_list() {
    let mut s = snap_with_buy();
    s.realized_pnl_today = -10_000.0;
    s.server_time = MON + 5 * 86_400 + 3600; // Saturday
    let mut l = limits();
    l.allowed_symbols = vec!["EURUSD".into()];
    l.max_positions = 1;
    assert!(evaluate(&modify(101, Some(3995.0), None), &s, &l, false).is_ok());
}

// ---- close ------------------------------------------------------------------------

fn close(tickets: &[u64], symbol: Option<&str>, side: Option<Side>, all: bool) -> ProposedAction {
    ProposedAction::Close(CloseSelector {
        tickets: tickets.to_vec(),
        symbol: symbol.map(str::to_owned),
        side,
        all,
        volume: None,
    })
}

fn close_cmds(p: &ActionPreview) -> Vec<u64> {
    p.commands
        .iter()
        .map(|c| match c {
            Command::PositionClose(a) => a.ticket,
            other => panic!("unexpected {other:?}"),
        })
        .collect()
}

fn book() -> MarketSnapshot {
    let mut s = snap();
    // deliberately not sorted by ticket
    s.positions = vec![
        pos(103, "XAUUSD", Side::Sell, 0.1, 4010.0, -12.5),
        pos(101, "XAUUSD", Side::Buy, 0.1, 3990.0, -20.0),
        pos(104, "EURUSD", Side::Buy, 0.2, 1.1, 7.0),
        pos(102, "XAUUSD", Side::Buy, 0.1, 3995.0, -9.6),
    ];
    s
}

#[test]
fn close_without_selector_is_ambiguous() {
    let e = err(&close(&[], None, None, false), &book());
    match e {
        GateError::Ambiguous { question } => {
            assert!(
                question.starts_with("Which positions should I close?"),
                "{question}"
            )
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn close_with_tickets_and_filters_is_invalid() {
    let s = book();
    assert!(is_invalid(&err(
        &close(&[101], Some("XAUUSD"), None, false),
        &s
    )));
    assert!(is_invalid(&err(
        &close(&[101], None, Some(Side::Buy), false),
        &s
    )));
    assert!(is_invalid(&err(&close(&[101], None, None, true), &s)));
}

#[test]
fn close_unknown_ticket_is_invalid() {
    let e = err(&close(&[101, 555], None, None, false), &book());
    assert!(is_invalid(&e));
    assert!(e.to_string().contains("555"), "{e}");
}

#[test]
fn close_single_ticket() {
    let p = ok(&close(&[101], None, None, false), &book());
    assert_eq!(
        p.description,
        "Close buy 0.10 XAUUSD, floating P/L -20.00 USD"
    );
    assert_eq!(p.tickets, vec![101]);
    assert_eq!(p.symbols, vec!["XAUUSD".to_owned()]);
    assert_eq!(p.count, 1);
    assert!(!p.is_batch);
    assert_eq!(p.total_volume, 0.1);
    assert_eq!(p.est_pnl.current, Some(-20.0));
    assert_eq!(p.est_pnl.at_sl, None);
    assert_eq!(
        p.commands,
        vec![Command::PositionClose(PositionCloseArgs {
            ticket: 101,
            deviation_points: None
        })]
    );
}

#[test]
fn close_by_symbol_is_a_batch_in_ticket_order() {
    let p = ok(&close(&[], Some("XAUUSD"), None, false), &book());
    assert_eq!(
        p.description,
        "Close 3 positions on XAUUSD (0.30 lots), floating P/L -42.10 USD"
    );
    assert!(p.is_batch);
    assert_eq!(p.count, 3);
    assert_eq!(p.tickets, vec![101, 102, 103]);
    assert_eq!(close_cmds(&p), vec![101, 102, 103]);
    assert_eq!(p.total_volume, 0.3);
    assert_eq!(p.est_pnl.current, Some(-42.1));
    assert_eq!(p.symbols, vec!["XAUUSD".to_owned()]);
}

#[test]
fn close_by_side_and_by_symbol_and_side() {
    let s = book();
    let p = ok(&close(&[], None, Some(Side::Buy), false), &s);
    assert_eq!(
        p.description,
        "Close 3 buy positions on EURUSD and XAUUSD (0.40 lots), floating P/L -22.60 USD"
    );
    assert_eq!(p.tickets, vec![101, 102, 104]);
    assert_eq!(p.symbols, vec!["EURUSD".to_owned(), "XAUUSD".to_owned()]);

    let p = ok(&close(&[], Some("XAUUSD"), Some(Side::Buy), false), &s);
    assert_eq!(
        p.description,
        "Close 2 buy positions on XAUUSD (0.20 lots), floating P/L -29.60 USD"
    );
    assert_eq!(p.tickets, vec![101, 102]);
}

#[test]
fn close_all() {
    let p = ok(&close(&[], None, None, true), &book());
    assert_eq!(
        p.description,
        "Close all 4 positions on EURUSD and XAUUSD (0.50 lots), floating P/L -35.10 USD"
    );
    assert!(p.is_batch);
    assert_eq!(p.count, 4);
    assert_eq!(close_cmds(&p), vec![101, 102, 103, 104]);
    assert_eq!(p.total_volume, 0.5);
}

#[test]
fn close_all_with_a_single_position_is_not_a_batch() {
    let mut s = snap();
    s.positions
        .push(pos(7, "EURUSD", Side::Sell, 0.5, 1.1, 3.25));
    let p = ok(&close(&[], None, None, true), &s);
    assert!(!p.is_batch);
    assert_eq!(p.count, 1);
    assert_eq!(
        p.description,
        "Close sell 0.50 EURUSD, floating P/L 3.25 USD"
    );
}

#[test]
fn close_multiple_tickets_is_a_batch_and_dedupes() {
    let p = ok(&close(&[103, 101, 103], None, None, false), &book());
    assert_eq!(p.tickets, vec![101, 103]);
    assert_eq!(p.count, 2);
    assert!(p.is_batch);
    assert_eq!(close_cmds(&p), vec![101, 103]);
}

#[test]
fn close_nothing_matched_is_nothing_to_do() {
    let s = book();
    assert!(matches!(
        err(&close(&[], Some("US30"), None, false), &s),
        GateError::NothingToDo(_)
    ));
    assert!(matches!(
        err(&close(&[], Some("EURUSD"), Some(Side::Sell), false), &s),
        GateError::NothingToDo(_)
    ));
    // symbol filters are exact broker names
    assert!(matches!(
        err(&close(&[], Some("xauusd"), None, false), &s),
        GateError::NothingToDo(_)
    ));
    // empty book
    assert!(matches!(
        err(&close(&[], None, None, true), &snap()),
        GateError::NothingToDo(_)
    ));
}

#[test]
fn close_all_flag_is_ignored_when_a_filter_is_present() {
    let p = ok(&close(&[], Some("EURUSD"), None, true), &book());
    assert_eq!(p.tickets, vec![104]);
}

#[test]
fn close_is_not_blocked_by_any_limit() {
    // Closing reduces risk: max_lot, daily loss, hours and the symbol list do not apply.
    let mut s = snap();
    s.positions = vec![pos(1, "XAUUSD", Side::Buy, 5.0, 3990.0, -400.0)];
    s.realized_pnl_today = -10_000.0;
    s.server_time = MON + 5 * 86_400 + 3600;
    let mut l = limits();
    l.allowed_symbols = vec!["EURUSD".into()];
    let p = evaluate(&close(&[1], None, None, false), &s, &l, false).unwrap();
    assert_eq!(p.total_volume, 5.0);
}

#[test]
fn close_does_not_need_prices_or_symbol_info() {
    let mut s = book();
    s.prices.clear();
    s.symbols.clear();
    assert!(evaluate(&close(&[101], None, None, false), &s, &limits(), false).is_ok());
}

#[test]
fn close_sums_floating_profit_without_float_noise() {
    let mut s = snap();
    s.positions = vec![
        pos(1, "XAUUSD", Side::Buy, 0.1, 3990.0, 0.1),
        pos(2, "XAUUSD", Side::Buy, 0.1, 3990.0, 0.2),
    ];
    let p = ok(&close(&[], Some("XAUUSD"), None, false), &s);
    assert_eq!(p.est_pnl.current, Some(0.3));
    assert_eq!(p.total_volume, 0.2);
}

// partial close

fn partial(ticket: u64, volume: f64) -> ProposedAction {
    ProposedAction::Close(CloseSelector {
        tickets: vec![ticket],
        volume: Some(volume),
        ..Default::default()
    })
}

#[test]
fn partial_close_builds_a_partial_command_and_prorates_pnl() {
    let mut s = snap();
    s.positions = vec![pos(1, "XAUUSD", Side::Buy, 0.5, 3990.0, -50.0)];
    let p = ok(&partial(1, 0.2), &s);
    assert_eq!(
        p.description,
        "Close 0.20 lots of buy 0.50 XAUUSD, floating P/L -20.00 USD"
    );
    assert_eq!(
        p.commands,
        vec![Command::PositionClosePartial(PositionClosePartialArgs {
            ticket: 1,
            volume: 0.2,
            deviation_points: None
        })]
    );
    assert_eq!(p.total_volume, 0.2);
    assert_eq!(p.est_pnl.current, Some(-20.0));
    assert!(!p.is_batch);
    assert_eq!(p.count, 1);
    assert_eq!(p.tickets, vec![1]);
}

#[test]
fn partial_close_rounds_down_to_step() {
    let mut s = snap();
    s.positions = vec![pos(1, "XAUUSD", Side::Buy, 0.5, 3990.0, -50.0)];
    let p = ok(&partial(1, 0.237), &s);
    match p.commands.as_slice() {
        [Command::PositionClosePartial(a)] => assert_eq!(a.volume, 0.23),
        other => panic!("{other:?}"),
    }
    assert!(
        p.warnings.iter().any(|w| w.contains("rounded down")),
        "{:?}",
        p.warnings
    );
}

#[test]
fn partial_close_validation() {
    let mut s = snap();
    s.positions = vec![
        pos(1, "XAUUSD", Side::Buy, 0.5, 3990.0, -50.0),
        pos(2, "XAUUSD", Side::Buy, 0.5, 3991.0, -40.0),
    ];
    // not smaller than the position
    assert!(is_invalid(&err(&partial(1, 0.5), &s)));
    assert!(is_invalid(&err(&partial(1, 0.6), &s)));
    // rounds below the minimum
    assert!(is_invalid(&err(&partial(1, 0.009), &s)));
    // junk volume
    for v in [0.0, -0.1, f64::NAN] {
        assert!(is_invalid(&err(&partial(1, v), &s)), "{v}");
    }
    // needs exactly one ticket
    let two = ProposedAction::Close(CloseSelector {
        tickets: vec![1, 2],
        volume: Some(0.1),
        ..Default::default()
    });
    assert!(is_invalid(&err(&two, &s)));
    let by_symbol = ProposedAction::Close(CloseSelector {
        symbol: Some("XAUUSD".into()),
        volume: Some(0.1),
        ..Default::default()
    });
    assert!(is_invalid(&err(&by_symbol, &s)));
    // needs symbol info for the step
    s.symbols.clear();
    assert!(is_invalid(&err(&partial(1, 0.1), &s)));
}

#[test]
fn partial_close_just_below_position_volume_is_allowed() {
    let mut s = snap();
    s.positions = vec![pos(1, "XAUUSD", Side::Buy, 0.5, 3990.0, -50.0)];
    let p = ok(&partial(1, 0.49), &s);
    assert_eq!(p.total_volume, 0.49);
}

// ---- cancel -----------------------------------------------------------------------

fn cancel(tickets: &[u64], symbol: Option<&str>, all: bool) -> ProposedAction {
    ProposedAction::Cancel(CancelSelector {
        tickets: tickets.to_vec(),
        symbol: symbol.map(str::to_owned),
        all,
    })
}

fn order_book() -> MarketSnapshot {
    let mut s = snap();
    s.orders = vec![
        pending(12, "EURUSD", OrderType::SellLimit, 0.2, 1.12),
        pending(11, "EURUSD", OrderType::BuyLimit, 0.1, 1.1),
        pending(13, "XAUUSD", OrderType::BuyStop, 0.3, 4100.0),
    ];
    s
}

fn cancel_cmds(p: &ActionPreview) -> Vec<u64> {
    p.commands
        .iter()
        .map(|c| match c {
            Command::OrderCancel(OrderCancelArgs { ticket }) => *ticket,
            other => panic!("unexpected {other:?}"),
        })
        .collect()
}

#[test]
fn cancel_single_order() {
    let p = ok(&cancel(&[11], None, false), &order_book());
    assert_eq!(p.description, "Cancel buy limit 0.10 EURUSD at 1.10000");
    assert_eq!(p.tickets, vec![11]);
    assert_eq!(p.symbols, vec!["EURUSD".to_owned()]);
    assert_eq!(p.total_volume, 0.1);
    assert_eq!(p.count, 1);
    assert!(!p.is_batch);
    assert_eq!(p.est_pnl, jarvis_gate::EstPnl::default());
    assert_eq!(
        p.commands,
        vec![Command::OrderCancel(OrderCancelArgs { ticket: 11 })]
    );
}

#[test]
fn cancel_by_symbol_is_a_batch_in_ticket_order() {
    let p = ok(&cancel(&[], Some("EURUSD"), false), &order_book());
    assert_eq!(
        p.description,
        "Cancel 2 pending orders on EURUSD (0.30 lots)"
    );
    assert!(p.is_batch);
    assert_eq!(p.count, 2);
    assert_eq!(cancel_cmds(&p), vec![11, 12]);
    assert_eq!(p.tickets, vec![11, 12]);
    assert_eq!(p.total_volume, 0.3);
}

#[test]
fn cancel_all() {
    let p = ok(&cancel(&[], None, true), &order_book());
    assert_eq!(
        p.description,
        "Cancel all 3 pending orders on EURUSD and XAUUSD (0.60 lots)"
    );
    assert_eq!(cancel_cmds(&p), vec![11, 12, 13]);
    assert_eq!(p.symbols, vec!["EURUSD".to_owned(), "XAUUSD".to_owned()]);
}

#[test]
fn cancel_errors() {
    let s = order_book();
    assert!(matches!(
        err(&cancel(&[], None, false), &s),
        GateError::Ambiguous { .. }
    ));
    assert!(is_invalid(&err(&cancel(&[11], Some("EURUSD"), false), &s)));
    assert!(is_invalid(&err(&cancel(&[11], None, true), &s)));
    assert!(is_invalid(&err(&cancel(&[99], None, false), &s)));
    assert!(matches!(
        err(&cancel(&[], Some("US30"), false), &s),
        GateError::NothingToDo(_)
    ));
    assert!(matches!(
        err(&cancel(&[], None, true), &snap()),
        GateError::NothingToDo(_)
    ));
}

#[test]
fn cancel_describes_every_order_type() {
    let cases = [
        (OrderType::BuyLimit, "buy limit"),
        (OrderType::SellLimit, "sell limit"),
        (OrderType::BuyStop, "buy stop"),
        (OrderType::SellStop, "sell stop"),
        (OrderType::BuyStopLimit, "buy stop limit"),
        (OrderType::SellStopLimit, "sell stop limit"),
    ];
    for (kind, text) in cases {
        let mut s = snap();
        s.orders = vec![pending(1, "EURUSD", kind, 0.1, 1.1)];
        let p = ok(&cancel(&[1], None, false), &s);
        assert_eq!(
            p.description,
            format!("Cancel {text} 0.10 EURUSD at 1.10000")
        );
    }
}

#[test]
fn cancel_without_symbol_info_still_describes_the_price() {
    let mut s = order_book();
    s.symbols.clear();
    let p = ok(&cancel(&[11], None, false), &s);
    assert_eq!(p.description, "Cancel buy limit 0.10 EURUSD at 1.1");
}

#[test]
fn cancel_is_not_blocked_by_any_limit() {
    let mut s = order_book();
    s.realized_pnl_today = -10_000.0;
    s.server_time = MON + 5 * 86_400 + 3600;
    let mut l = limits();
    l.allowed_symbols.clear();
    assert!(evaluate(&cancel(&[11], None, false), &s, &l, false).is_ok());
}

// ---- misc -------------------------------------------------------------------------

#[test]
fn previews_serialize_round_trip() {
    let p = ok(&buy_order(0.1, Some(3990.0), Some(4020.0)), &snap());
    let json = serde_json::to_string(&p).unwrap();
    let back: ActionPreview = serde_json::from_str(&json).unwrap();
    assert_eq!(back, p);
}

#[test]
fn evaluate_is_deterministic_apart_from_the_id() {
    let s = snap();
    let a = ok(&buy_order(0.1, Some(3990.0), None), &s);
    let b = ok(&buy_order(0.1, Some(3990.0), None), &s);
    assert_eq!(a.description, b.description);
    assert_eq!(a.est_pnl, b.est_pnl);
    assert_eq!(a.warnings, b.warnings);
}
