//! "Risking 1%" in the user's words: the parser reads it, and the cross-check accepts a
//! risk-sized proposal instead of a volume (and still refuses a volume the user never said).

use std::collections::HashMap;

use jarvis_bridge_client::protocol::{ExecType, Side};
use jarvis_gate::intent::{Intent, Mismatch, OrderFacts, cross_check, cross_check_sized, parse};
use jarvis_gate::{OrderIntent, ProposedAction};

fn aliases() -> HashMap<String, String> {
    [("gold", "XAUUSD"), ("nasdaq", "USTEC"), ("euro", "EURUSD")]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

fn facts(text: &str) -> OrderFacts {
    match parse(text, &aliases()) {
        Intent::Order(f) => f,
        other => panic!("'{text}' was read as {other:?}, not an order"),
    }
}

#[test]
fn risk_phrases_are_read() {
    // (utterance, risk, sl)
    let cases = [
        ("buy gold risking 1% with a stop at 3900", 1.0, Some(3900.0)),
        ("buy gold risking 1 % with a stop at 3900", 1.0, Some(3900.0)),
        ("buy gold 1% risk stop loss 3900", 1.0, Some(3900.0)),
        ("buy gold with 1 percent risk and a stop at 3900", 1.0, Some(3900.0)),
        ("buy gold, risk 0.5 percent, sl 3900", 0.5, Some(3900.0)),
        ("buy gold 0.5% risk sl 3900", 0.5, Some(3900.0)),
        ("sell nasdaq risking one percent stop at 19900", 1.0, Some(19900.0)),
        ("sell nasdaq risk of 1.5 percent", 1.5, None),
        ("buy euro risking zero point five percent", 0.5, None),
        ("buy gold risking 2 per cent sl 3900", 2.0, Some(3900.0)),
        ("BUY GOLD RISKING 1 PCT SL 3900", 1.0, Some(3900.0)),
    ];
    for (text, risk, sl) in cases {
        let f = facts(text);
        assert_eq!(f.risk_percent, Some(risk), "{text}: {f:?}");
        assert_eq!(f.sl, sl, "{text}: {f:?}");
        // The percentage is never mistaken for lots or a price.
        assert_eq!(f.volume, None, "{text}: {f:?}");
        assert_eq!(f.price, None, "{text}: {f:?}");
    }
}

#[test]
fn a_risk_is_not_read_without_a_percentage() {
    for text in [
        "buy gold risking 50 dollars",
        "buy gold risk 50",
        "buy 0.1 gold, it is a risky one",
        "buy gold 0.1",
    ] {
        assert_eq!(facts(text).risk_percent, None, "{text}");
    }
    assert_eq!(facts("buy gold risking 50 dollars").volume, None);
}

#[test]
fn lots_and_risk_are_told_apart() {
    let f = facts("buy 0.1 gold");
    assert_eq!((f.volume, f.risk_percent), (Some(0.1), None));
    let f = facts("buy 0.1 gold risking 1%");
    assert_eq!((f.volume, f.risk_percent), (Some(0.1), Some(1.0)));
    // A decimal percentage right after the symbol is not a volume.
    let f = facts("buy gold 0.5% risk");
    assert_eq!((f.volume, f.risk_percent), (None, Some(0.5)));
}

#[test]
fn a_percent_sign_elsewhere_changes_nothing() {
    let f = facts("buy 0.1 gold at 2350");
    assert_eq!((f.volume, f.price, f.risk_percent), (Some(0.1), Some(2350.0), None));
    assert_eq!(parse("what is 5% of 200", &aliases()), Intent::Other);
}

fn proposal(volume: f64) -> ProposedAction {
    ProposedAction::Order(OrderIntent {
        symbol: "XAUUSD".to_owned(),
        side: Side::Buy,
        volume,
        exec: ExecType::Market,
        price: None,
        sl: Some(3900.0),
        tp: None,
    })
}

fn field_of(r: Result<(), Mismatch>) -> Option<&'static str> {
    r.err().map(|m| m.field)
}

#[test]
fn a_risk_sized_proposal_is_accepted_for_a_risk_request() {
    let said = parse("buy gold risking 1% with a stop at 3900", &aliases());
    // The volume is computed from the risk: it is not compared with anything.
    assert_eq!(cross_check_sized(&said, &proposal(0.19), Some(1.0)), Ok(()));
    assert_eq!(cross_check_sized(&said, &proposal(0.02), Some(1.0)), Ok(()));
}

#[test]
fn the_wrong_risk_is_refused() {
    let said = parse("buy gold risking 1% with a stop at 3900", &aliases());
    assert_eq!(
        field_of(cross_check_sized(&said, &proposal(0.19), Some(2.0))),
        Some("risk_percent")
    );
}

#[test]
fn a_volume_the_user_never_said_is_refused_when_they_said_a_risk() {
    let said = parse("buy gold risking 1% with a stop at 3900", &aliases());
    let err = cross_check(&said, &proposal(0.1)).unwrap_err();
    assert_eq!(err.field, "volume");
    assert!(err.said.contains("risk of 1%"), "{err}");
    assert_eq!(
        field_of(cross_check_sized(&said, &proposal(0.1), None)),
        Some("volume")
    );
}

#[test]
fn a_risk_the_user_never_said_is_refused_when_they_said_lots() {
    let said = parse("buy 0.1 gold with a stop at 3900", &aliases());
    assert_eq!(
        field_of(cross_check_sized(&said, &proposal(0.19), Some(1.0))),
        Some("volume")
    );
    // The plain volume still works.
    assert_eq!(cross_check(&said, &proposal(0.1)), Ok(()));
    assert_eq!(
        field_of(cross_check(&said, &proposal(0.2))),
        Some("volume")
    );
}

#[test]
fn a_risk_proposal_after_a_bare_follow_up_still_needs_an_explicit_order() {
    // "yes" is not a request to open a trade, whatever size the model computed.
    let said = parse("yes", &aliases());
    let err = cross_check_sized(&said, &proposal(0.19), Some(1.0)).unwrap_err();
    assert!(err.explicit_request_needed);
}

#[test]
fn a_proposal_may_fill_the_size_from_context_when_the_user_gave_none() {
    // Same leniency as for lots: "buy gold" then "1%" arrives as a separate message.
    let said = parse("buy gold stop at 3900", &aliases());
    assert_eq!(cross_check_sized(&said, &proposal(0.19), Some(1.0)), Ok(()));
}
