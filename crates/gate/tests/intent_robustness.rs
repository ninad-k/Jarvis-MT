//! Parser edge cases: no panics on junk, symbol recognition limits, determinism.

use std::collections::HashMap;

use jarvis_bridge_client::protocol::Side;
use jarvis_gate::intent::{Intent, OrderFacts, parse};

fn aliases() -> HashMap<String, String> {
    [
        ("gold", "XAUUSD"),
        ("nasdaq", "USTEC"),
        ("dow jones", "US30"),
        ("s&p", "US500"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

fn order(text: &str) -> OrderFacts {
    match parse(text, &aliases()) {
        Intent::Order(f) => f,
        other => panic!("{text:?} parsed as {other:?}"),
    }
}

/// Small deterministic generator (xorshift) so the test needs no extra crates.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[(self.next() % items.len() as u64) as usize]
    }
}

#[test]
fn parse_never_panics_on_random_word_soup() {
    let vocabulary = [
        "buy",
        "sell",
        "close",
        "cancel",
        "stop",
        "loss",
        "sl",
        "tp",
        "limit",
        "gold",
        "nasdaq",
        "dow",
        "jones",
        "s&p",
        "500",
        "100",
        "0.1",
        ".5",
        "5.",
        "..",
        "0.1.2",
        "point",
        "dot",
        "zero",
        "one",
        "ek",
        "do",
        "lot",
        "lots",
        "half",
        "aadha",
        "a",
        "all",
        "sab",
        "pips",
        "at",
        "@",
        "to",
        "ka",
        "karo",
        "kar",
        "band",
        "le",
        "lo",
        "by",
        "cell",
        "long",
        "short",
        "term",
        "ticket",
        "123456",
        "99999999999999999999999999",
        "-",
        "—",
        "\"",
        "'",
        "#",
        ",",
        "3,990",
        "1,2,3",
        "xauusd",
        "eurusd",
        "market",
        "😀",
        "सोना",
        "ǅ",
        "İ",
        "ß",
        "\u{0}",
        "pending",
        "orders",
        "yes",
        "no",
        "confirm",
        "move",
        "set",
        "change",
    ];
    let separators = [" ", "  ", ", ", ". ", "\t", "\n", ""];
    let aliases = aliases();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..4000 {
        let len = 1 + (rng.next() % 12) as usize;
        let mut text = String::new();
        for _ in 0..len {
            text.push_str(rng.pick(&vocabulary));
            text.push_str(rng.pick(&separators));
        }
        let first = parse(&text, &aliases);
        let second = parse(&text, &aliases);
        assert_eq!(first, second, "non-deterministic parse of {text:?}");
    }
}

#[test]
fn parse_handles_very_long_and_degenerate_input() {
    let aliases = aliases();
    let long = "buy 0.1 gold ".repeat(5_000);
    let _ = parse(&long, &aliases);
    let digits = "9".repeat(2_000);
    assert_eq!(parse(&format!("buy {digits} gold"), &aliases), {
        Intent::Order(OrderFacts {
            side: Some(Side::Buy),
            symbol: Some("XAUUSD".into()),
            ..Default::default()
        })
    });
    for junk in [
        "\u{0}",
        "....",
        ",,,,",
        "@@@",
        "'''",
        "\u{2019}",
        "   \t\n",
        "😀😀😀",
    ] {
        assert_eq!(parse(junk, &aliases), Intent::Other, "{junk:?}");
    }
}

#[test]
fn unknown_words_are_not_symbols() {
    // six-letter words must not be mistaken for broker symbols
    for text in [
        "buy 0.1 market",
        "buy 0.1 target",
        "sell 0.2 profit",
        "buy 0.1 please",
    ] {
        let f = parse(text, &aliases());
        match f {
            Intent::Order(f) => assert_eq!(f.symbol, None, "{text:?}"),
            Intent::Confirm | Intent::Other => {}
            other => panic!("{text:?} parsed as {other:?}"),
        }
    }
}

#[test]
fn bare_broker_symbols_are_recognised() {
    for (text, symbol) in [
        ("buy 0.1 eurusd", "EURUSD"),
        ("sell 0.1 GBPJPY", "GBPJPY"),
        ("buy 0.1 xauusd", "XAUUSD"),
        ("buy 0.1 btcusd", "BTCUSD"),
        ("sell 0.1 us30", "US30"),
        ("buy 0.1 ustec", "USTEC"),
        ("buy 0.1 nas100", "NAS100"),
    ] {
        assert_eq!(order(text).symbol.as_deref(), Some(symbol), "{text:?}");
    }
}

#[test]
fn alias_values_are_recognised_even_without_a_matching_key() {
    // "us500" is only known as the value of the "s&p" alias
    assert_eq!(order("buy 0.1 us500").symbol.as_deref(), Some("US500"));
}

#[test]
fn empty_alias_table_still_extracts_side_and_volume() {
    let f = match parse("buy 0.1 gold", &HashMap::new()) {
        Intent::Order(f) => f,
        other => panic!("{other:?}"),
    };
    assert_eq!(f.side, Some(Side::Buy));
    assert_eq!(f.volume, Some(0.1));
    assert_eq!(f.symbol, None);
}

#[test]
fn longest_alias_wins() {
    assert_eq!(order("sell 0.1 dow jones").symbol.as_deref(), Some("US30"));
}

#[test]
fn index_suffix_numbers_are_not_prices_or_volumes() {
    let f = order("sell limit s&p 500 at 5400 0.2 lots");
    assert_eq!(f.symbol.as_deref(), Some("US500"));
    assert_eq!(f.price, Some(5400.0));
    assert_eq!(f.volume, Some(0.2));
}

#[test]
fn distances_in_pips_are_not_levels() {
    let f = order("buy 0.1 gold stop loss 50 pips take profit 100 pips");
    assert_eq!(f.sl, None);
    assert_eq!(f.tp, None);
    assert_eq!(f.volume, Some(0.1));
}

#[test]
fn volume_is_never_taken_from_a_price_sized_number() {
    let f = order("buy gold at 4000");
    assert_eq!(f.volume, None);
    assert_eq!(f.price, Some(4000.0));
}

#[test]
fn zero_volume_is_not_a_fact() {
    assert_eq!(order("buy 0 lots gold").volume, None);
}

#[test]
fn confirm_and_cancel_only_when_the_utterance_is_short() {
    let aliases = aliases();
    for text in [
        "yes please go ahead and confirm it now",
        "no stop loss please",
        "cancel the plan to buy gold tomorrow",
        "stop loss at 3990",
    ] {
        let got = parse(text, &aliases);
        assert!(
            !matches!(got, Intent::Confirm | Intent::Cancel),
            "{text:?} -> {got:?}"
        );
    }
}

#[test]
fn thousands_separators_and_symbols_in_numbers() {
    let f = order("buy 0.1 gold sl 3,990.50 tp $4,020");
    assert_eq!(f.sl, Some(3990.5));
    assert_eq!(f.tp, Some(4020.0));
}
