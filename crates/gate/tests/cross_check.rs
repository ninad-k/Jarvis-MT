//! `cross_check`: a proposal must not contradict what the user actually said
//! (SPEC rule 9, ADR-0006). Facts the user did not state are never checked.

use std::collections::HashMap;

use jarvis_bridge_client::protocol::{ExecType, Side};
use jarvis_gate::intent::{CloseFacts, Intent, ModifyFacts, OrderFacts, cross_check, parse};
use jarvis_gate::{CancelSelector, CloseSelector, ModifyIntent, OrderIntent, ProposedAction};

fn aliases() -> HashMap<String, String> {
    [
        ("gold", "XAUUSD"),
        ("nasdaq", "USTEC"),
        ("euro", "EURUSD"),
        ("dow jones", "US30"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

fn order(side: Side, symbol: &str, volume: f64) -> OrderIntent {
    OrderIntent {
        symbol: symbol.to_owned(),
        side,
        volume,
        exec: ExecType::Market,
        price: None,
        sl: None,
        tp: None,
    }
}

fn say(text: &str) -> Intent {
    parse(text, &aliases())
}

fn field_of(r: Result<(), jarvis_gate::intent::Mismatch>) -> Option<&'static str> {
    r.err().map(|m| m.field)
}

// ---- orders ---------------------------------------------------------------------------

#[test]
fn matching_order_passes() {
    let said = say("buy 0.1 gold");
    let proposal = ProposedAction::Order(order(Side::Buy, "XAUUSD", 0.1));
    assert_eq!(cross_check(&said, &proposal), Ok(()));
}

#[test]
fn order_mismatch_table() {
    let with = |f: fn(&mut OrderIntent)| {
        let mut o = order(Side::Buy, "XAUUSD", 0.1);
        f(&mut o);
        ProposedAction::Order(o)
    };
    let cases: Vec<(&str, ProposedAction, Option<&str>)> = vec![
        ("buy 0.1 gold", with(|_| {}), None),
        ("buy 0.1 gold", with(|o| o.side = Side::Sell), Some("side")),
        ("sell 0.1 gold", with(|_| {}), Some("side")),
        (
            "buy 0.1 gold",
            with(|o| o.symbol = "USTEC".into()),
            Some("symbol"),
        ),
        ("buy 0.1 nasdaq", with(|_| {}), Some("symbol")),
        ("buy 0.1 gold", with(|o| o.volume = 0.2), Some("volume")),
        ("buy 0.1 gold", with(|o| o.volume = 0.1 + 1e-12), None),
        (
            "buy 0.1 gold",
            with(|o| o.volume = 0.1 + 1e-6),
            Some("volume"),
        ),
        // facts not said are not checked
        ("buy gold", with(|o| o.volume = 5.0), None),
        ("buy 0.1", with(|o| o.symbol = "EURUSD".into()), None),
        (
            "buy 0.1 gold",
            with(|o| {
                o.sl = Some(3990.0);
                o.tp = Some(4020.0);
            }),
            None,
        ),
        // SL / TP / price said
        ("buy 0.1 gold sl 3990", with(|o| o.sl = Some(3990.0)), None),
        (
            "buy 0.1 gold sl 3990",
            with(|o| o.sl = Some(3980.0)),
            Some("sl"),
        ),
        ("buy 0.1 gold sl 3990", with(|_| {}), Some("sl")),
        ("buy 0.1 gold tp 4020", with(|o| o.tp = Some(4020.0)), None),
        (
            "buy 0.1 gold tp 4020",
            with(|o| o.tp = Some(4200.0)),
            Some("tp"),
        ),
        ("buy 0.1 gold tp 4020", with(|_| {}), Some("tp")),
        (
            "buy 0.1 gold sl 3990",
            with(|o| o.sl = Some(3990.0 * (1.0 + 1e-8))),
            None,
        ),
        (
            "buy 0.1 gold sl 3990",
            with(|o| o.sl = Some(3990.0 * (1.0 + 1e-4))),
            Some("sl"),
        ),
        // user said a price: the proposal must carry it
        ("buy 0.1 gold at 2350", with(|_| {}), Some("price")),
        (
            "buy 0.1 gold at 2350",
            with(|o| {
                o.exec = ExecType::Limit;
                o.price = Some(2350.0);
            }),
            None,
        ),
        (
            "buy limit 0.1 gold at 2350",
            with(|o| {
                o.exec = ExecType::Limit;
                o.price = Some(2350.0);
            }),
            None,
        ),
        // pending type said
        (
            "buy limit 0.1 gold at 2350",
            with(|o| {
                o.exec = ExecType::Stop;
                o.price = Some(2350.0);
            }),
            Some("type"),
        ),
        ("buy stop 0.1 gold at 2350", with(|_| {}), Some("type")),
    ];
    for (text, proposal, expected) in cases {
        let said = say(text);
        let got = field_of(cross_check(&said, &proposal));
        assert_eq!(got, expected, "{text:?} vs {proposal:?}");
    }
}

#[test]
fn symbol_with_broker_suffix_matches() {
    let said = say("buy 0.1 gold");
    for broker in [
        "XAUUSD",
        "xauusd",
        "XAUUSD.m",
        "XAUUSDm",
        "XAUUSD_i",
        "XAUUSD#",
        "XAUUSD.pro",
    ] {
        let proposal = ProposedAction::Order(order(Side::Buy, broker, 0.1));
        assert_eq!(cross_check(&said, &proposal), Ok(()), "{broker}");
    }
}

#[test]
fn different_symbols_that_share_a_prefix_do_not_match() {
    let said = Intent::Order(OrderFacts {
        side: Some(Side::Buy),
        symbol: Some("US30".into()),
        ..Default::default()
    });
    for broker in ["US300", "US30X1", "XAUUSD", "US3"] {
        let proposal = ProposedAction::Order(order(Side::Buy, broker, 0.1));
        assert_eq!(
            field_of(cross_check(&said, &proposal)),
            Some("symbol"),
            "{broker}"
        );
    }
}

#[test]
fn mismatch_message_names_both_values() {
    let said = say("buy 0.1 gold");
    let proposal = ProposedAction::Order(order(Side::Sell, "XAUUSD", 0.1));
    let m = cross_check(&said, &proposal).unwrap_err();
    assert_eq!(m.field, "side");
    assert_eq!(m.said, "buy");
    assert_eq!(m.proposed, "sell");
    assert_eq!(
        m.to_string(),
        "side: you said buy, but the proposal has sell"
    );
}

#[test]
fn first_mismatch_follows_field_order() {
    // side is wrong and so is the volume: side is reported
    let said = say("buy 0.1 gold");
    let proposal = ProposedAction::Order(order(Side::Sell, "XAUUSD", 5.0));
    assert_eq!(field_of(cross_check(&said, &proposal)), Some("side"));
}

// ---- non-blocking intents ----------------------------------------------------------------

#[test]
fn other_confirm_cancel_never_block() {
    let proposals = [
        ProposedAction::Order(order(Side::Sell, "EURUSD", 3.0)),
        ProposedAction::Close(CloseSelector::default()),
        ProposedAction::Cancel(CancelSelector::default()),
        ProposedAction::Modify(ModifyIntent {
            ticket: 1,
            sl: Some(1.0),
            tp: None,
        }),
    ];
    for said in [Intent::Other, Intent::Confirm, Intent::Cancel] {
        for p in &proposals {
            assert_eq!(cross_check(&said, p), Ok(()), "{said:?} vs {p:?}");
        }
    }
}

#[test]
fn proposal_of_a_different_kind_is_rejected() {
    let close_all = ProposedAction::Close(CloseSelector {
        all: true,
        ..Default::default()
    });
    assert_eq!(
        field_of(cross_check(&say("buy 0.1 gold"), &close_all)),
        Some("action")
    );
    let buy = ProposedAction::Order(order(Side::Buy, "XAUUSD", 0.1));
    assert_eq!(
        field_of(cross_check(&say("close all"), &buy)),
        Some("action")
    );
    assert_eq!(
        field_of(cross_check(&say("cancel my pending orders"), &buy)),
        Some("action")
    );
    assert_eq!(
        field_of(cross_check(&say("set sl 3990"), &buy)),
        Some("action")
    );
}

// ---- close ----------------------------------------------------------------------------------

fn close_sel(f: impl FnOnce(&mut CloseSelector)) -> ProposedAction {
    let mut c = CloseSelector::default();
    f(&mut c);
    ProposedAction::Close(c)
}

#[test]
fn close_selector_table() {
    let cases: Vec<(&str, ProposedAction, Option<&str>)> = vec![
        ("close all", close_sel(|c| c.all = true), None),
        (
            "close all",
            close_sel(|c| c.symbol = Some("XAUUSD".into())),
            Some("selector"),
        ),
        (
            "close all",
            close_sel(|c| c.side = Some(Side::Buy)),
            Some("selector"),
        ),
        // the gate resolves tickets from the account, so they cannot be compared to "all"
        ("close all", close_sel(|c| c.tickets = vec![1, 2]), None),
        (
            "close gold",
            close_sel(|c| c.symbol = Some("XAUUSD".into())),
            None,
        ),
        (
            "close gold",
            close_sel(|c| c.symbol = Some("USTEC".into())),
            Some("selector"),
        ),
        ("close gold", close_sel(|c| c.all = true), Some("selector")),
        ("close gold", close_sel(|c| c.tickets = vec![5]), None),
        (
            "close my gold longs",
            close_sel(|c| {
                c.symbol = Some("XAUUSD".into());
                c.side = Some(Side::Buy);
            }),
            None,
        ),
        (
            "close my gold longs",
            close_sel(|c| {
                c.symbol = Some("XAUUSD".into());
                c.side = Some(Side::Sell);
            }),
            Some("selector"),
        ),
        (
            "close my gold longs",
            close_sel(|c| c.symbol = Some("XAUUSD".into())),
            Some("selector"),
        ),
        (
            "close all nasdaq longs",
            close_sel(|c| {
                c.symbol = Some("USTEC".into());
                c.side = Some(Side::Buy);
                c.all = true;
            }),
            None,
        ),
        (
            "close all shorts",
            close_sel(|c| c.side = Some(Side::Sell)),
            None,
        ),
        (
            "close all shorts",
            close_sel(|c| c.side = Some(Side::Buy)),
            Some("selector"),
        ),
        // nothing said: nothing checked
        (
            "close it",
            close_sel(|c| c.symbol = Some("XAUUSD".into())),
            None,
        ),
        ("close it", close_sel(|c| c.tickets = vec![9]), None),
        // tickets said must be the tickets proposed
        (
            "close ticket 123456",
            close_sel(|c| c.tickets = vec![123456]),
            None,
        ),
        (
            "close ticket 123456",
            close_sel(|c| c.tickets = vec![123457]),
            Some("selector"),
        ),
        (
            "close ticket 123456",
            close_sel(|c| c.symbol = Some("XAUUSD".into())),
            Some("selector"),
        ),
        // volume
        (
            "close 0.05 gold",
            close_sel(|c| {
                c.symbol = Some("XAUUSD".into());
                c.volume = Some(0.05);
                c.tickets = vec![7];
                c.symbol = None;
            }),
            None,
        ),
        (
            "close 0.05 gold",
            close_sel(|c| c.symbol = Some("XAUUSD".into())),
            Some("volume"),
        ),
        (
            "close half of gold",
            close_sel(|c| c.symbol = Some("XAUUSD".into())),
            Some("volume"),
        ),
        (
            "close half of gold",
            close_sel(|c| {
                c.tickets = vec![7];
                c.volume = Some(0.05);
            }),
            None,
        ),
    ];
    for (text, proposal, expected) in cases {
        let got = field_of(cross_check(&say(text), &proposal));
        assert_eq!(got, expected, "{text:?} vs {proposal:?}");
    }
}

#[test]
fn close_all_with_a_different_proposal_names_the_selector() {
    let m = cross_check(
        &say("close all"),
        &close_sel(|c| c.symbol = Some("XAUUSD".into())),
    )
    .unwrap_err();
    assert_eq!(m.field, "selector");
    assert_eq!(m.said, "all");
}

// ---- cancel orders ---------------------------------------------------------------------------

#[test]
fn cancel_orders_selector_table() {
    let cancel = |f: fn(&mut CancelSelector)| {
        let mut c = CancelSelector::default();
        f(&mut c);
        ProposedAction::Cancel(c)
    };
    let cases: Vec<(&str, ProposedAction, Option<&str>)> = vec![
        ("cancel all pending orders", cancel(|c| c.all = true), None),
        (
            "cancel all pending orders",
            cancel(|c| c.symbol = Some("EURUSD".into())),
            Some("selector"),
        ),
        (
            "cancel pending euro orders",
            cancel(|c| c.symbol = Some("EURUSD".into())),
            None,
        ),
        (
            "cancel pending euro orders",
            cancel(|c| c.symbol = Some("XAUUSD".into())),
            Some("selector"),
        ),
        (
            "cancel pending euro orders",
            cancel(|c| c.all = true),
            Some("selector"),
        ),
        ("cancel my pending orders", cancel(|c| c.all = true), None),
        (
            "cancel my pending orders",
            cancel(|c| c.tickets = vec![5]),
            None,
        ),
        (
            "cancel order 123456",
            cancel(|c| c.tickets = vec![123456]),
            None,
        ),
        (
            "cancel order 123456",
            cancel(|c| c.tickets = vec![999999]),
            Some("selector"),
        ),
    ];
    for (text, proposal, expected) in cases {
        let got = field_of(cross_check(&say(text), &proposal));
        assert_eq!(got, expected, "{text:?} vs {proposal:?}");
    }
}

// ---- modify -----------------------------------------------------------------------------------

#[test]
fn modify_table() {
    let modify = |ticket: u64, sl: Option<f64>, tp: Option<f64>| {
        ProposedAction::Modify(ModifyIntent { ticket, sl, tp })
    };
    let cases: Vec<(&str, ProposedAction, Option<&str>)> = vec![
        (
            "move my stop loss to 3990",
            modify(1, Some(3990.0), None),
            None,
        ),
        (
            "move my stop loss to 3990",
            modify(1, Some(3995.0), None),
            Some("sl"),
        ),
        (
            "move my stop loss to 3990",
            modify(1, None, Some(4000.0)),
            Some("sl"),
        ),
        ("set tp 4050", modify(1, None, Some(4050.0)), None),
        ("set tp 4050", modify(1, None, Some(4060.0)), Some("tp")),
        (
            "set sl 3990 and tp 4050",
            modify(1, Some(3990.0), Some(4050.0)),
            None,
        ),
        (
            "set sl 3990 and tp 4050",
            modify(1, Some(3990.0), Some(4000.0)),
            Some("tp"),
        ),
        ("change my stop loss", modify(1, Some(1.0), None), None),
        (
            "move sl to 3990 for ticket 123456",
            modify(123456, Some(3990.0), None),
            None,
        ),
        (
            "move sl to 3990 for ticket 123456",
            modify(654321, Some(3990.0), None),
            Some("selector"),
        ),
    ];
    for (text, proposal, expected) in cases {
        let got = field_of(cross_check(&say(text), &proposal));
        assert_eq!(got, expected, "{text:?} vs {proposal:?}");
    }
}

// ---- manual facts ---------------------------------------------------------------------------------

#[test]
fn facts_built_by_hand_are_checked_the_same_way() {
    let said = Intent::Close(CloseFacts {
        all: false,
        symbol: Some("XAUUSD".into()),
        side: None,
        tickets: vec![],
        volume: None,
        half: false,
    });
    assert_eq!(
        cross_check(&said, &close_sel(|c| c.symbol = Some("XAUUSD".into()))),
        Ok(())
    );
    let said = Intent::Modify(ModifyFacts {
        symbol: None,
        ticket: None,
        sl: None,
        tp: None,
    });
    assert_eq!(
        cross_check(
            &said,
            &ProposedAction::Modify(ModifyIntent {
                ticket: 1,
                sl: Some(2.0),
                tp: None
            })
        ),
        Ok(())
    );
}
