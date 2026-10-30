//! Intent parser corpus (SPEC testing strategy: "100+ phrases incl. Hinglish").
//! Each row is an utterance (English, Hinglish, mixed, with speech-to-text slips) and
//! the `Intent` the parser must produce.

use std::collections::HashMap;

use jarvis_bridge_client::protocol::{ExecType, Side};
use jarvis_gate::intent::{CloseFacts, Intent, ModifyFacts, OrderFacts, parse};

fn aliases() -> HashMap<String, String> {
    [
        ("gold", "XAUUSD"),
        ("cold", "XAUUSD"),
        ("sona", "XAUUSD"),
        ("silver", "XAGUSD"),
        ("nasdaq", "USTEC"),
        ("nas", "USTEC"),
        ("dow", "US30"),
        ("dow jones", "US30"),
        ("s&p", "US500"),
        ("spx", "US500"),
        ("euro", "EURUSD"),
        ("fiber", "EURUSD"),
        ("cable", "GBPUSD"),
        ("pound", "GBPUSD"),
        ("bitcoin", "BTCUSD"),
        ("btc", "BTCUSD"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

// ---- expected-value builders -----------------------------------------------------

const GOLD: &str = "XAUUSD";

#[derive(Default)]
struct O {
    side: Option<Side>,
    symbol: Option<&'static str>,
    volume: Option<f64>,
    pending: Option<ExecType>,
    price: Option<f64>,
    sl: Option<f64>,
    tp: Option<f64>,
}

impl O {
    fn build(self) -> Intent {
        Intent::Order(OrderFacts {
            side: self.side,
            symbol: self.symbol.map(str::to_owned),
            volume: self.volume,
            pending: self.pending,
            price: self.price,
            sl: self.sl,
            tp: self.tp,
        })
    }
}

fn buy(symbol: &'static str, volume: Option<f64>) -> O {
    O {
        side: Some(Side::Buy),
        symbol: Some(symbol),
        volume,
        ..Default::default()
    }
}

fn sell(symbol: &'static str, volume: Option<f64>) -> O {
    O {
        side: Some(Side::Sell),
        symbol: Some(symbol),
        volume,
        ..Default::default()
    }
}

fn close(all: bool, symbol: Option<&str>, side: Option<Side>) -> CloseFacts {
    CloseFacts {
        all,
        symbol: symbol.map(str::to_owned),
        side,
        ..Default::default()
    }
}

fn modify(symbol: Option<&str>, sl: Option<f64>, tp: Option<f64>) -> Intent {
    Intent::Modify(ModifyFacts {
        symbol: symbol.map(str::to_owned),
        ticket: None,
        sl,
        tp,
    })
}

fn run(rows: Vec<(&str, Intent)>) {
    let aliases = aliases();
    let mut failures = Vec::new();
    for (text, expected) in &rows {
        let got = parse(text, &aliases);
        if &got != expected {
            failures.push(format!(
                "{text:?}\n    expected {expected:?}\n    got      {got:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} phrases failed:\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n")
    );
}

// ---- confirm ------------------------------------------------------------------------

fn confirm_phrases_rows() -> Vec<(&'static str, Intent)> {
    let c = || Intent::Confirm;
    vec![
        ("confirm", c()),
        ("Confirm.", c()),
        ("CONFIRM!", c()),
        ("confirmed", c()),
        ("yes", c()),
        ("Yes, confirm", c()),
        ("yes confirm please", c()),
        ("do it", c()),
        ("Do it!", c()),
        ("go ahead", c()),
        ("go ahead please", c()),
        ("execute", c()),
        ("place it", c()),
        ("haan", c()),
        ("haan ji", c()),
        ("ha", c()),
        ("theek hai", c()),
        ("theek hai kar do", c()),
        ("kar do", c()),
        ("haan kar do", c()),
        ("haan haan kar do", c()),
        ("confirm karo", c()),
        ("confirm kar do", c()),
        ("ok confirm", c()),
        ("Jarvis confirm", c()),
        ("yes yes confirm", c()),
        ("YES!!!", c()),
        ("yeah do it", c()),
        ("haan confirm", c()),
        ("  yes   ", c()),
    ]
}

#[test]
fn confirm_phrases() {
    run(confirm_phrases_rows());
}

// ---- cancel ---------------------------------------------------------------------------

fn cancel_phrases_rows() -> Vec<(&'static str, Intent)> {
    let x = || Intent::Cancel;
    vec![
        ("cancel", x()),
        ("Cancel!", x()),
        ("no", x()),
        ("No.", x()),
        ("stop", x()),
        ("Stop!", x()),
        ("abort", x()),
        ("don't", x()),
        ("dont", x()),
        ("nahi", x()),
        ("nahin", x()),
        ("mat karo", x()),
        ("ruk jao", x()),
        ("rehne do", x()),
        ("cancel karo", x()),
        ("no no no", x()),
        ("nahi nahi", x()),
        ("no cancel", x()),
        ("cancel it", x()),
        ("dont do it", x()),
        ("please cancel", x()),
        ("stop it", x()),
        ("never mind", x()),
        ("nope", x()),
        ("cancel the order", x()),
        ("order cancel karo", x()),
    ]
}

#[test]
fn cancel_phrases() {
    run(cancel_phrases_rows());
}

// ---- things that must NOT be confirm or cancel -------------------------------------

fn trading_words_are_not_answers_rows() -> Vec<(&'static str, Intent)> {
    vec![
        ("no stop loss", Intent::Other),
        ("stop loss", Intent::Other),
        ("stop loss 3990", Intent::Other),
        ("what's the stop loss", Intent::Other),
        ("yes I think gold is going up", Intent::Other),
        ("no I meant sell", Intent::Other),
        ("ok", Intent::Other),
        ("okay", Intent::Other),
        ("hello jarvis", Intent::Other),
        ("do you think gold goes up", Intent::Other),
        ("I do not want to buy", Intent::Other),
        ("tell me about the close price of gold", Intent::Other),
        ("is gold long term bullish", Intent::Other),
        ("what is gold at", Intent::Other),
        ("show my positions", Intent::Other),
        ("how many positions do I have", Intent::Other),
        ("what is my balance", Intent::Other),
        ("thank you", Intent::Other),
        ("gold", Intent::Other),
        ("buy", Intent::Other),
        ("sell", Intent::Other),
        ("1 2 3", Intent::Other),
        ("", Intent::Other),
        ("   ", Intent::Other),
        ("?!", Intent::Other),
    ]
}

#[test]
fn trading_words_are_not_answers() {
    run(trading_words_are_not_answers_rows());
}

// ---- new orders: English --------------------------------------------------------------

fn market_orders_english_rows() -> Vec<(&'static str, Intent)> {
    vec![
        ("buy 0.1 gold", buy(GOLD, Some(0.1)).build()),
        ("Buy point one gold", buy(GOLD, Some(0.1)).build()),
        ("buy zero point five gold", buy(GOLD, Some(0.5)).build()),
        ("buy point zero five gold", buy(GOLD, Some(0.05)).build()),
        (
            "sell zero point zero one gold",
            sell(GOLD, Some(0.01)).build(),
        ),
        ("buy one point five gold", buy(GOLD, Some(1.5)).build()),
        ("sell 0.2 xauusd", sell(GOLD, Some(0.2)).build()),
        ("buy half a lot of gold", buy(GOLD, Some(0.5)).build()),
        ("buy one lot gold", buy(GOLD, Some(1.0)).build()),
        ("buy 1 lot of gold", buy(GOLD, Some(1.0)).build()),
        ("buy 2 gold", buy(GOLD, Some(2.0)).build()),
        ("buy 0.01 gold", buy(GOLD, Some(0.01)).build()),
        ("BUY 0.1 GOLD", buy(GOLD, Some(0.1)).build()),
        ("buy 0.1 gold.", buy(GOLD, Some(0.1)).build()),
        ("buy   0.1    gold", buy(GOLD, Some(0.1)).build()),
        ("buy gold", buy(GOLD, None).build()),
        (
            "I want to buy 0.1 gold please",
            buy(GOLD, Some(0.1)).build(),
        ),
        (
            "can you sell 0.2 gold at market",
            sell(GOLD, Some(0.2)).build(),
        ),
        ("buy 0.1lot gold", buy(GOLD, Some(0.1)).build()),
        ("buy 0.1 lots of gold", buy(GOLD, Some(0.1)).build()),
        ("sell 0.3 dow jones", sell("US30", Some(0.3)).build()),
        ("buy 0.1 bitcoin", buy("BTCUSD", Some(0.1)).build()),
        ("buy 0.1 s&p", buy("US500", Some(0.1)).build()),
        ("buy 0.1 S&P 500", buy("US500", Some(0.1)).build()),
        ("buy 0.1 nasdaq 100", buy("USTEC", Some(0.1)).build()),
        ("go long gold 0.1", buy(GOLD, Some(0.1)).build()),
        ("short nasdaq 0.5", sell("USTEC", Some(0.5)).build()),
        ("go short on dow", sell("US30", None).build()),
        ("sell 0.1 silver", sell("XAGUSD", Some(0.1)).build()),
        ("buy 0.1 pound", buy("GBPUSD", Some(0.1)).build()),
    ]
}

#[test]
fn market_orders_english() {
    run(market_orders_english_rows());
}

fn orders_with_sl_and_tp_rows() -> Vec<(&'static str, Intent)> {
    let with = |mut o: O, sl: Option<f64>, tp: Option<f64>| {
        o.sl = sl;
        o.tp = tp;
        o.build()
    };
    vec![
        (
            "buy 0.1 gold with stop loss 3990 and take profit 4020",
            with(buy(GOLD, Some(0.1)), Some(3990.0), Some(4020.0)),
        ),
        (
            "buy 0.1 lots of gold with stop loss 3990 and take profit 4020",
            with(buy(GOLD, Some(0.1)), Some(3990.0), Some(4020.0)),
        ),
        (
            "buy 0.1 gold sl 3990 tp 4020",
            with(buy(GOLD, Some(0.1)), Some(3990.0), Some(4020.0)),
        ),
        (
            "sell 0.1 gold stop loss at 4010",
            with(sell(GOLD, Some(0.1)), Some(4010.0), None),
        ),
        (
            "buy 0.1 gold target 4020",
            with(buy(GOLD, Some(0.1)), None, Some(4020.0)),
        ),
        (
            "buy 0.1 gold take profit 4020",
            with(buy(GOLD, Some(0.1)), None, Some(4020.0)),
        ),
        (
            "buy 0.01 gold sl 3990",
            with(buy(GOLD, Some(0.01)), Some(3990.0), None),
        ),
        (
            "buy 0.1 gold, stop loss 3,990",
            with(buy(GOLD, Some(0.1)), Some(3990.0), None),
        ),
        (
            "buy 0.1 gold sl3990 tp4020",
            with(buy(GOLD, Some(0.1)), Some(3990.0), Some(4020.0)),
        ),
        (
            "sell 0.10 lots eurusd tp 1.0950 sl 1.1050",
            with(sell("EURUSD", Some(0.1)), Some(1.105), Some(1.095)),
        ),
        (
            "buy 0.1 gold stop loss 50 pips",
            buy(GOLD, Some(0.1)).build(),
        ),
        (
            "buy 0.1 gold with no stop loss",
            buy(GOLD, Some(0.1)).build(),
        ),
        (
            "buy 0.1 gold and set stop loss 3990",
            with(buy(GOLD, Some(0.1)), Some(3990.0), None),
        ),
        (
            "buy 0.1 gold stop 3990",
            with(buy(GOLD, Some(0.1)), Some(3990.0), None),
        ),
    ]
}

#[test]
fn orders_with_sl_and_tp() {
    run(orders_with_sl_and_tp_rows());
}

fn pending_orders_rows() -> Vec<(&'static str, Intent)> {
    let pend = |mut o: O, kind: ExecType, price: Option<f64>| {
        o.pending = Some(kind);
        o.price = price;
        o
    };
    vec![
        (
            "buy limit 0.1 eurusd at 1.1",
            pend(buy("EURUSD", Some(0.1)), ExecType::Limit, Some(1.1)).build(),
        ),
        (
            "buy limit at 1.1 euro",
            pend(buy("EURUSD", None), ExecType::Limit, Some(1.1)).build(),
        ),
        (
            "buy limit 1.1 euro",
            pend(buy("EURUSD", None), ExecType::Limit, Some(1.1)).build(),
        ),
        (
            "sell stop 3950 gold",
            pend(sell(GOLD, None), ExecType::Stop, Some(3950.0)).build(),
        ),
        (
            "sell stop 0.1 gold at 3950",
            pend(sell(GOLD, Some(0.1)), ExecType::Stop, Some(3950.0)).build(),
        ),
        (
            "buy stop 0.1 gold at 4010",
            pend(buy(GOLD, Some(0.1)), ExecType::Stop, Some(4010.0)).build(),
        ),
        (
            "sell 0.1 gold limit 4050",
            pend(sell(GOLD, Some(0.1)), ExecType::Limit, Some(4050.0)).build(),
        ),
        ("buy limit 0.1 gold at 3950 sl 3940 tp 3980", {
            let mut o = pend(buy(GOLD, Some(0.1)), ExecType::Limit, Some(3950.0));
            o.sl = Some(3940.0);
            o.tp = Some(3980.0);
            o.build()
        }),
        (
            "buylimit 0.1 gold at 3950",
            pend(buy(GOLD, Some(0.1)), ExecType::Limit, Some(3950.0)).build(),
        ),
        (
            "place a sell limit on gold at 4100 for 0.2 lots",
            pend(sell(GOLD, Some(0.2)), ExecType::Limit, Some(4100.0)).build(),
        ),
        (
            "buy stop",
            pend(
                O {
                    side: Some(Side::Buy),
                    ..Default::default()
                },
                ExecType::Stop,
                None,
            )
            .build(),
        ),
        ("buy 0.1 gold at 2350", {
            let mut o = buy(GOLD, Some(0.1));
            o.price = Some(2350.0);
            o.build()
        }),
        ("buy 0.1 gold @ 2350", {
            let mut o = buy(GOLD, Some(0.1));
            o.price = Some(2350.0);
            o.build()
        }),
    ]
}

#[test]
fn pending_orders() {
    run(pending_orders_rows());
}

// ---- new orders: Hinglish and speech-to-text slips ---------------------------------

fn orders_hinglish_rows() -> Vec<(&'static str, Intent)> {
    vec![
        ("gold kharido", buy(GOLD, None).build()),
        ("gold kharido 0.1", buy(GOLD, Some(0.1)).build()),
        ("khareedo 0.1 gold", buy(GOLD, Some(0.1)).build()),
        ("0.1 gold becho", sell(GOLD, Some(0.1)).build()),
        ("sona le lo 0.1", buy(GOLD, Some(0.1)).build()),
        ("mujhe gold bech do", sell(GOLD, None).build()),
        ("nasdaq kharid lo", buy("USTEC", None).build()),
        ("ek lot gold le lo", buy(GOLD, Some(1.0)).build()),
        ("aadha lot nasdaq becho", sell("USTEC", Some(0.5)).build()),
        ("sell ek lot gold", sell(GOLD, Some(1.0)).build()),
        ("buy do lot gold", buy(GOLD, Some(2.0)).build()),
        ("teen lot gold kharido", buy(GOLD, Some(3.0)).build()),
        ("paanch lot nasdaq becho", sell("USTEC", Some(5.0)).build()),
        ("gold 0.2 bechna hai", sell(GOLD, Some(0.2)).build()),
        ("bitcoin kharidna hai", buy("BTCUSD", None).build()),
        ("zero point one gold kharido", buy(GOLD, Some(0.1)).build()),
    ]
}

#[test]
fn orders_hinglish() {
    run(orders_hinglish_rows());
}

fn orders_with_speech_to_text_artefacts_rows() -> Vec<(&'static str, Intent)> {
    vec![
        ("by 0.1 gold", buy(GOLD, Some(0.1)).build()),
        ("by gold", buy(GOLD, None).build()),
        ("cell gold", sell(GOLD, None).build()),
        ("cell 0.2 gold", sell(GOLD, Some(0.2)).build()),
        ("buy 0.1 cold", buy(GOLD, Some(0.1)).build()),
        ("sell point one cold", sell(GOLD, Some(0.1)).build()),
        ("by zero point five nasdaq", buy("USTEC", Some(0.5)).build()),
        ("buy 0.1 gold , stop loss , 3990", {
            let mut o = buy(GOLD, Some(0.1));
            o.sl = Some(3990.0);
            o.build()
        }),
        // "by"/"cell" without a following fact are just English words
        ("stand by", Intent::Other),
        ("cell phone", Intent::Other),
    ]
}

#[test]
fn orders_with_speech_to_text_artefacts() {
    run(orders_with_speech_to_text_artefacts_rows());
}

// ---- close ----------------------------------------------------------------------------

fn close_phrases_rows() -> Vec<(&'static str, Intent)> {
    let all = |symbol: Option<&str>, side: Option<Side>| Intent::Close(close(true, symbol, side));
    let some = |symbol: Option<&str>, side: Option<Side>| Intent::Close(close(false, symbol, side));
    vec![
        ("close all", all(None, None)),
        ("close everything", all(None, None)),
        ("close all positions", all(None, None)),
        ("sab band karo", all(None, None)),
        ("nikal do sab", all(None, None)),
        ("exit everything", all(None, None)),
        ("close my gold longs", some(Some(GOLD), Some(Side::Buy))),
        (
            "close all nasdaq longs",
            all(Some("USTEC"), Some(Side::Buy)),
        ),
        ("close all shorts", all(None, Some(Side::Sell))),
        ("close gold", some(Some(GOLD), None)),
        ("close my sell positions", some(None, Some(Side::Sell))),
        ("exit gold", some(Some(GOLD), None)),
        ("square off gold", some(Some(GOLD), None)),
        ("gold band karo", some(Some(GOLD), None)),
        ("gold band kar do", some(Some(GOLD), None)),
        ("saare gold buy band karo", all(Some(GOLD), Some(Side::Buy))),
        ("close all gold positions", all(Some(GOLD), None)),
        ("close the position on euro", some(Some("EURUSD"), None)),
        ("close it", some(None, None)),
        ("band karo", some(None, None)),
        ("close 3 positions", some(None, None)),
        ("CLOSE ALL.", all(None, None)),
        (
            "please close all my gold buys",
            all(Some(GOLD), Some(Side::Buy)),
        ),
        (
            "close all short positions on dow",
            all(Some("US30"), Some(Side::Sell)),
        ),
    ]
}

#[test]
fn close_phrases() {
    run(close_phrases_rows());
}

fn close_partial_and_ticket_phrases_rows() -> Vec<(&'static str, Intent)> {
    let facts = |f: CloseFacts| Intent::Close(f);
    vec![
        (
            "close ticket 123456",
            facts(CloseFacts {
                tickets: vec![123456],
                ..Default::default()
            }),
        ),
        (
            "close position 123456",
            facts(CloseFacts {
                tickets: vec![123456],
                ..Default::default()
            }),
        ),
        (
            "close 123456 and 123457",
            facts(CloseFacts {
                tickets: vec![123456, 123457],
                ..Default::default()
            }),
        ),
        (
            "close half of gold",
            facts(CloseFacts {
                symbol: Some(GOLD.into()),
                half: true,
                ..Default::default()
            }),
        ),
        (
            "close half my gold position",
            facts(CloseFacts {
                symbol: Some(GOLD.into()),
                half: true,
                ..Default::default()
            }),
        ),
        (
            "aadha gold band karo",
            facts(CloseFacts {
                symbol: Some(GOLD.into()),
                half: true,
                ..Default::default()
            }),
        ),
        (
            "close 0.05 gold",
            facts(CloseFacts {
                symbol: Some(GOLD.into()),
                volume: Some(0.05),
                ..Default::default()
            }),
        ),
        (
            "close 0.05 lots of my gold buy",
            facts(CloseFacts {
                symbol: Some(GOLD.into()),
                side: Some(Side::Buy),
                volume: Some(0.05),
                ..Default::default()
            }),
        ),
        (
            "close one lot of nasdaq",
            facts(CloseFacts {
                symbol: Some("USTEC".into()),
                volume: Some(1.0),
                ..Default::default()
            }),
        ),
    ]
}

#[test]
fn close_partial_and_ticket_phrases() {
    run(close_partial_and_ticket_phrases_rows());
}

// ---- cancel pending orders ---------------------------------------------------------------

fn cancel_order_phrases_rows() -> Vec<(&'static str, Intent)> {
    let orders = |f: CloseFacts| Intent::CancelOrders(f);
    vec![
        ("cancel my pending orders", orders(CloseFacts::default())),
        ("cancel all pending orders", orders(close(true, None, None))),
        (
            "cancel pending gold orders",
            orders(close(false, Some(GOLD), None)),
        ),
        ("cancel orders", orders(CloseFacts::default())),
        (
            "sab pending orders cancel karo",
            orders(close(true, None, None)),
        ),
        ("delete pending orders", orders(CloseFacts::default())),
        (
            "cancel order 123456",
            orders(CloseFacts {
                tickets: vec![123456],
                ..Default::default()
            }),
        ),
        (
            "cancel my limit orders on euro",
            orders(close(false, Some("EURUSD"), None)),
        ),
        ("close all pending orders", orders(close(true, None, None))),
        (
            "cancel pending orders on nasdaq",
            orders(close(false, Some("USTEC"), None)),
        ),
    ]
}

#[test]
fn cancel_order_phrases() {
    run(cancel_order_phrases_rows());
}

// ---- modify ---------------------------------------------------------------------------------

fn modify_phrases_rows() -> Vec<(&'static str, Intent)> {
    vec![
        (
            "move my stop loss to 3990",
            modify(None, Some(3990.0), None),
        ),
        ("set sl 3990", modify(None, Some(3990.0), None)),
        ("change tp to 4050", modify(None, None, Some(4050.0))),
        ("move stop to 3990", modify(None, Some(3990.0), None)),
        (
            "set stop loss 3990 and take profit 4020",
            modify(None, Some(3990.0), Some(4020.0)),
        ),
        ("sl 3990 karo", modify(None, Some(3990.0), None)),
        (
            "gold ka tp 4050 karo",
            modify(Some(GOLD), None, Some(4050.0)),
        ),
        (
            "update take profit to 4050 on gold",
            modify(Some(GOLD), None, Some(4050.0)),
        ),
        (
            "move sl to 3990 for ticket 123456",
            Intent::Modify(ModifyFacts {
                symbol: None,
                ticket: Some(123456),
                sl: Some(3990.0),
                tp: None,
            }),
        ),
        ("set target 4050", modify(None, None, Some(4050.0))),
        ("change my stop loss", modify(None, None, None)),
        ("raise stop loss to 3995", modify(None, Some(3995.0), None)),
        (
            "gold ka stop loss 3990 rakho",
            modify(Some(GOLD), Some(3990.0), None),
        ),
        ("move my stop to break even", modify(None, None, None)),
        ("tp 4050 laga do", modify(None, None, Some(4050.0))),
        (
            "nasdaq ka sl 18000 kar do",
            modify(Some("USTEC"), Some(18000.0), None),
        ),
        (
            "lower my take profit to 4030",
            modify(None, None, Some(4030.0)),
        ),
    ]
}

#[test]
fn modify_phrases() {
    run(modify_phrases_rows());
}

// ---- corpus size guard -------------------------------------------------------------------

/// Keeps the corpus honest: the SPEC asks for 100+ phrases and this file must not
/// silently shrink below that.
#[test]
fn corpus_has_at_least_one_hundred_twenty_phrases() {
    let mut n = 0;
    n += confirm_phrases_rows().len();
    n += cancel_phrases_rows().len();
    n += trading_words_are_not_answers_rows().len();
    n += market_orders_english_rows().len();
    n += orders_with_sl_and_tp_rows().len();
    n += pending_orders_rows().len();
    n += orders_hinglish_rows().len();
    n += orders_with_speech_to_text_artefacts_rows().len();
    n += close_phrases_rows().len();
    n += close_partial_and_ticket_phrases_rows().len();
    n += cancel_order_phrases_rows().len();
    n += modify_phrases_rows().len();
    assert!(n >= 120, "only {n} phrases in the corpus");
}
