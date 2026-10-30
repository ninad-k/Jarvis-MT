//! Deterministic intent parser (English + Hinglish). It does not replace the LLM; it
//! (1) recognises "confirm"/"cancel" in speech or text so the UI can call
//! `/api/confirm/{token}` or `/api/cancel/{token}`, and (2) extracts the trading facts
//! the user actually said so [`cross_check`] can reject an LLM proposal that differs
//! (wrong side, symbol, volume) — SPEC rule 9, ADR-0006.
//!
//! # How an utterance is read
//!
//! 1. The text is normalised: lower case, punctuation removed (except `.` inside a
//!    number), `3,990` read as `3990`, `@` read as "at".
//! 2. A *short* utterance (at most four words once fillers such as "please" and "ji"
//!    are dropped and repeats collapsed) that is exactly a confirm or cancel phrase is
//!    `Confirm` or `Cancel`. Longer sentences never are, so "no stop loss" or "yes I
//!    think gold is going up" are not mistaken for answers.
//! 3. Otherwise words are canonicalised (`kharido` -> buy, `band karo` -> exit,
//!    `stop loss` -> sl, ...), numbers are read from digits and number words
//!    ("point one", "zero point five", "half a lot", "ek lot", "do lot"), symbols are
//!    resolved through the alias table, and the utterance is classified as cancelling
//!    orders, closing, modifying, opening, or `Other`.
//!
//! Facts the user did not say stay `None`; nothing is guessed.

use std::collections::HashMap;

use jarvis_bridge_client::protocol::{ExecType, Side};
use serde::{Deserialize, Serialize};

use crate::{CancelSelector, CloseSelector, ModifyIntent, OrderIntent, ProposedAction};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Intent {
    /// "confirm", "yes do it", "haan kar do", "go ahead".
    Confirm,
    /// "cancel", "no", "stop", "nahi", "ruk jao", "mat karo".
    Cancel,
    /// Open a new trade. Every field is what the user said, `None` if not said.
    Order(OrderFacts),
    /// Close positions: "close all", "sab band karo", "close my gold longs".
    Close(CloseFacts),
    /// Change SL/TP: "move my stop to 3990", "gold ka tp 4050 karo".
    Modify(ModifyFacts),
    /// Cancel pending orders: "cancel my pending orders".
    CancelOrders(CloseFacts),
    /// Anything else (questions, chit-chat): leave to the LLM.
    Other,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OrderFacts {
    pub side: Option<Side>,
    /// Broker symbol after alias resolution.
    pub symbol: Option<String>,
    pub volume: Option<f64>,
    /// "limit"/"stop" said explicitly; `None` = market or unspecified.
    pub pending: Option<jarvis_bridge_client::protocol::ExecType>,
    pub price: Option<f64>,
    pub sl: Option<f64>,
    pub tp: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CloseFacts {
    pub all: bool,
    pub symbol: Option<String>,
    pub side: Option<Side>,
    pub tickets: Vec<u64>,
    /// Partial: "close half", "close 0.05".
    pub volume: Option<f64>,
    pub half: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModifyFacts {
    pub symbol: Option<String>,
    pub ticket: Option<u64>,
    pub sl: Option<f64>,
    pub tp: Option<f64>,
}

// ---- vocabulary ------------------------------------------------------------------

/// Words dropped before matching a confirm/cancel phrase.
const FILLERS: &[&str] = &[
    "please", "pls", "plz", "ji", "jarvis", "bhai", "sir", "yaar", "abhi", "now",
];

const CONFIRM_PHRASES: &[&str] = &[
    "confirm",
    "confirmed",
    "confirm it",
    "i confirm",
    "yes",
    "yeah",
    "yep",
    "yup",
    "yes confirm",
    "yes confirmed",
    "yes do it",
    "yeah do it",
    "yes go ahead",
    "do it",
    "go ahead",
    "execute",
    "execute it",
    "place it",
    "place order",
    "place the order",
    "ok confirm",
    "okay confirm",
    "ok do it",
    "okay do it",
    "haan",
    "han",
    "ha",
    "haan confirm",
    "haan kar do",
    "haan kardo",
    "haan kar de",
    "ha kar do",
    "theek hai",
    "thik hai",
    "theek hai kar do",
    "thik hai kar do",
    "kar do",
    "kardo",
    "kar de",
    "confirm karo",
    "confirm kar do",
    "confirm kardo",
];

const CANCEL_PHRASES: &[&str] = &[
    "cancel",
    "cancel it",
    "cancel that",
    "cancel this",
    "cancel order",
    "cancel the order",
    "cancel my order",
    "cancel trade",
    "cancel the trade",
    "no cancel",
    "no",
    "nope",
    "no thanks",
    "no dont",
    "stop",
    "stop it",
    "abort",
    "dont",
    "do not",
    "dont do it",
    "never mind",
    "nevermind",
    "nahi",
    "nahin",
    "nahi karna",
    "mat karo",
    "mat kar",
    "ruk jao",
    "ruko",
    "rehne do",
    "rehne de",
    "chhod do",
    "cancel karo",
    "cancel kar do",
    "cancel kardo",
    "order cancel karo",
    "order cancel kar do",
];

/// Multi-word spellings rewritten to one canonical word. Longest patterns first.
const MULTI: &[(&[&str], &str)] = &[
    (&["band", "kar", "do"], "exit"),
    (&["bandh", "kar", "do"], "exit"),
    (&["band", "karo"], "exit"),
    (&["bandh", "karo"], "exit"),
    (&["band", "kardo"], "exit"),
    (&["bandh", "kardo"], "exit"),
    (&["nikal", "do"], "exit"),
    (&["nikaal", "do"], "exit"),
    (&["square", "off"], "exit"),
    (&["le", "lo"], "buy"),
    (&["le", "le"], "buy"),
    (&["kharid", "lo"], "buy"),
    (&["bech", "do"], "sell"),
    (&["bech", "de"], "sell"),
    (&["stop", "loss"], "sl"),
    (&["take", "profit"], "tp"),
    (&["kar", "do"], "karo"),
    (&["laga", "do"], "lagao"),
    (&["badal", "do"], "badlo"),
];

/// Single-word spellings rewritten to one canonical word.
const SYNONYMS: &[(&str, &str)] = &[
    ("kharido", "buy"),
    ("khareedo", "buy"),
    ("kharidna", "buy"),
    ("kharid", "buy"),
    ("khareed", "buy"),
    ("lena", "buy"),
    ("lelo", "buy"),
    ("buying", "buy"),
    ("buys", "buy"),
    ("longs", "buy"),
    ("becho", "sell"),
    ("bechna", "sell"),
    ("bechdo", "sell"),
    ("bech", "sell"),
    ("selling", "sell"),
    ("sells", "sell"),
    ("shorts", "sell"),
    ("stoploss", "sl"),
    ("takeprofit", "tp"),
    ("target", "tp"),
    ("tgt", "tp"),
    ("squareoff", "exit"),
    ("nikalo", "exit"),
    ("kardo", "karo"),
    ("cancle", "cancel"),
];

const ALL_WORDS: &[&str] = &[
    "all",
    "sab",
    "sabhi",
    "saare",
    "saari",
    "sabko",
    "everything",
    "poori",
];
const HALF_WORDS: &[&str] = &["half", "aadha", "adha", "aadhi"];
const EN_MODIFY_VERBS: &[&str] = &[
    "move", "set", "change", "update", "modify", "adjust", "shift", "raise", "lower", "trail",
    "edit",
];
const HI_MODIFY_VERBS: &[&str] = &["karo", "lagao", "rakho", "badlo"];
/// Words that may sit between "sl"/"tp" and its number ("sl at 3990", "tp ka ...").
const LEVEL_FILLERS: &[&str] = &[
    "at", "to", "is", "of", "ka", "ki", "ko", "pe", "par", "as", "around",
];
const PRICE_MARKERS: &[&str] = &["at", "price", "pe", "par", "around", "near"];
const VOLUME_WORDS: &[&str] = &["volume", "size", "qty", "quantity", "lots"];
const DISTANCE_UNITS: &[&str] = &[
    "pip", "pips", "point", "points", "pts", "pt", "dollar", "dollars",
];
const TICKET_WORDS: &[&str] = &["ticket", "tickets", "position", "order", "orders", "id"];
const PENDING_WORDS: &[&str] = &["pending", "pendings", "order", "orders", "limit", "limits"];
/// Words after "close" that make it a noun ("the close price").
const NOT_A_CLOSE_VERB: &[&str] = &["price", "prices", "above", "below", "value", "time", "to"];

/// Currencies/metals/crypto that form a plausible bare six-letter symbol ("eurusd").
const PAIR_PARTS: &[&str] = &[
    "usd", "eur", "gbp", "jpy", "chf", "aud", "nzd", "cad", "xau", "xag", "xpt", "xpd", "btc",
    "eth", "ltc", "xrp", "sgd", "hkd", "nok", "sek", "try", "zar", "mxn", "cnh", "pln", "czk",
    "huf", "dkk", "thb", "inr", "cny",
];
/// Index/commodity names recognised as bare symbols even without an alias.
const BARE_INDICES: &[&str] = &[
    "ustec", "us30", "us100", "us500", "nas100", "spx500", "ger40", "ger30", "uk100", "jp225",
    "usoil", "ukoil",
];
/// Trailing numbers of index names that are part of the name, not a price.
const INDEX_SUFFIXES: &[u32] = &[30, 40, 100, 225, 500];

// ---- normalisation ----------------------------------------------------------------

/// Lower-case, strip punctuation, keep `.` only inside numbers, drop thousands commas.
fn normalize(text: &str) -> String {
    let chars: Vec<char> = text.to_lowercase().chars().collect();
    let digit_at = |i: usize| chars.get(i).is_some_and(char::is_ascii_digit);
    let mut out = String::with_capacity(chars.len());
    for (i, &c) in chars.iter().enumerate() {
        match c {
            c if c.is_alphanumeric() => out.push(c),
            '.' if digit_at(i + 1) => out.push('.'),
            ',' if i > 0
                && digit_at(i - 1)
                && digit_at(i + 1)
                && digit_at(i + 2)
                && digit_at(i + 3)
                && !digit_at(i + 4) => {}
            '@' => out.push_str(" at "),
            '\'' | '\u{2019}' | '`' | '&' => {}
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn words(norm: &str) -> Vec<&str> {
    norm.split(' ').filter(|w| !w.is_empty()).collect()
}

// ---- confirm / cancel ---------------------------------------------------------------

fn confirm_or_cancel(raw: &[&str]) -> Option<Intent> {
    let mut kept: Vec<&str> = Vec::new();
    for w in raw.iter().filter(|w| !FILLERS.contains(w)) {
        if kept.last() != Some(w) {
            kept.push(w);
        }
    }
    if kept.is_empty() || kept.len() > 4 {
        return None;
    }
    let phrase = kept.join(" ");
    if CONFIRM_PHRASES.contains(&phrase.as_str()) {
        Some(Intent::Confirm)
    } else if CANCEL_PHRASES.contains(&phrase.as_str()) {
        Some(Intent::Cancel)
    } else {
        None
    }
}

// ---- lexing -------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct Num {
    v: f64,
    /// Digit count when written as a plain integer ("123456"), `None` otherwise.
    int_digits: Option<usize>,
    /// Written with a decimal point or as "point five".
    decimal: bool,
    /// Followed by "lot"/"lots" (or built from "half a lot").
    lotted: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Tk {
    W(String),
    N(Num),
    /// A resolved broker symbol.
    S(String),
}

impl Tk {
    fn is(&self, word: &str) -> bool {
        matches!(self, Tk::W(w) if w == word)
    }

    fn is_any(&self, set: &[&str]) -> bool {
        matches!(self, Tk::W(w) if set.contains(&w.as_str()))
    }

    fn num(&self) -> Option<&Num> {
        match self {
            Tk::N(n) => Some(n),
            _ => None,
        }
    }
}

struct AliasTable {
    /// (normalised key words, broker symbol), longest keys first.
    keys: Vec<(Vec<String>, String)>,
    /// lower-case broker symbol -> broker symbol as configured.
    values: HashMap<String, String>,
}

impl AliasTable {
    fn new(aliases: &HashMap<String, String>) -> Self {
        let mut keys: Vec<(Vec<String>, String)> = aliases
            .iter()
            .filter_map(|(k, v)| {
                let key: Vec<String> = words(&normalize(k))
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
                (!key.is_empty()).then(|| (key, v.clone()))
            })
            .collect();
        keys.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.cmp(b)));
        let values = aliases
            .values()
            .map(|v| (v.to_lowercase(), v.clone()))
            .collect();
        Self { keys, values }
    }

    /// Alias match at `i`: (number of words, broker symbol, joined key).
    fn match_at(&self, ws: &[String], i: usize) -> Option<(usize, String, String)> {
        self.keys.iter().find_map(|(key, value)| {
            let hit = ws.len() >= i + key.len() && ws[i..i + key.len()] == key[..];
            hit.then(|| (key.len(), value.clone(), key.join(" ")))
        })
    }

    fn bare(&self, w: &str) -> Option<String> {
        if let Some(v) = self.values.get(w) {
            return Some(v.clone());
        }
        if BARE_INDICES.contains(&w) {
            return Some(w.to_uppercase());
        }
        let is_pair = w.len() == 6
            && w.is_ascii()
            && PAIR_PARTS.contains(&&w[..3])
            && PAIR_PARTS.contains(&&w[3..]);
        is_pair.then(|| w.to_uppercase())
    }
}

/// `"sl3990"` -> `("sl", "3990")`, `"0.1lot"` -> `("0.1", "lot")`, `"buylimit"` -> ...
fn split_glued(w: &str) -> Option<(String, String)> {
    for (glued, a, b) in [
        ("buylimit", "buy", "limit"),
        ("selllimit", "sell", "limit"),
        ("buystop", "buy", "stop"),
        ("sellstop", "sell", "stop"),
    ] {
        if w == glued {
            return Some((a.to_owned(), b.to_owned()));
        }
    }
    for prefix in ["sl", "tp"] {
        if let Some(rest) = w.strip_prefix(prefix)
            && numeric(rest).is_some()
        {
            return Some((prefix.to_owned(), rest.to_owned()));
        }
    }
    for suffix in ["lots", "lot"] {
        if let Some(rest) = w.strip_suffix(suffix)
            && numeric(rest).is_some()
        {
            return Some((rest.to_owned(), suffix.to_owned()));
        }
    }
    None
}

fn canonicalize(raw: &[&str]) -> Vec<String> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    'next: while i < raw.len() {
        for (pattern, replacement) in MULTI {
            if raw[i..].starts_with(pattern) {
                out.push((*replacement).to_owned());
                i += pattern.len();
                continue 'next;
            }
        }
        if let Some((a, b)) = split_glued(raw[i]) {
            out.push(a);
            out.push(b);
        } else {
            let w = SYNONYMS
                .iter()
                .find(|(from, _)| *from == raw[i])
                .map_or(raw[i], |(_, to)| to);
            out.push(w.to_owned());
        }
        i += 1;
    }
    out
}

/// A plain numeric word: digits with at most one `.`.
fn numeric(w: &str) -> Option<Num> {
    let mut dots = 0;
    let mut digits = 0;
    for c in w.chars() {
        match c {
            '0'..='9' => digits += 1,
            '.' => dots += 1,
            _ => return None,
        }
    }
    if digits == 0 || dots > 1 {
        return None;
    }
    let v: f64 = w.parse().ok()?;
    Some(Num {
        v,
        int_digits: (dots == 0).then_some(digits),
        decimal: dots == 1,
        lotted: false,
    })
}

/// Spoken digit words. `do` is deliberately missing: it only means 2 before "lot(s)".
fn digit_word(w: &str) -> Option<u32> {
    Some(match w {
        "zero" | "shunya" | "sifar" => 0,
        "one" | "ek" => 1,
        "two" => 2,
        "three" | "teen" => 3,
        "four" | "char" => 4,
        "five" | "paanch" | "panch" => 5,
        "six" | "chhe" | "che" | "chhah" => 6,
        "seven" | "saat" => 7,
        "eight" | "aath" => 8,
        "nine" | "nau" => 9,
        "ten" | "das" => 10,
        _ => return None,
    })
}

fn is_lot(w: Option<&String>) -> bool {
    w.is_some_and(|w| w == "lot" || w == "lots")
}

/// One fractional digit: "five", "5" (but not "10").
fn fraction_digits(w: &str) -> Option<String> {
    if let Some(d) = digit_word(w) {
        return (d < 10).then(|| d.to_string());
    }
    (!w.is_empty() && w.bytes().all(|b| b.is_ascii_digit())).then(|| w.to_owned())
}

/// Integer part of a decimal sequence: "zero", "1", "ten".
fn integer_part(w: &str) -> Option<String> {
    if let Some(d) = digit_word(w) {
        return Some(d.to_string());
    }
    numeric(w)
        .filter(|n| n.int_digits.is_some())
        .map(|_| w.to_owned())
}

/// Try to read a spoken or mixed decimal ("point one", "zero point five", "1 point 5")
/// starting at `i`. Returns the number and how many words it used.
fn spoken_decimal(ws: &[String], i: usize) -> Option<(Num, usize)> {
    let is_point = |w: &String| w == "point" || w == "dot";
    let (int_part, point_at) = if is_point(&ws[i]) {
        ("0".to_owned(), i)
    } else if ws.get(i + 1).is_some_and(is_point) {
        (integer_part(&ws[i])?, i + 1)
    } else {
        return None;
    };
    let mut frac = String::new();
    let mut k = point_at + 1;
    while let Some(d) = ws.get(k).and_then(|w| fraction_digits(w)) {
        frac.push_str(&d);
        k += 1;
    }
    if frac.is_empty() {
        return None;
    }
    let v: f64 = format!("{int_part}.{frac}").parse().ok()?;
    Some((
        Num {
            v,
            int_digits: None,
            decimal: true,
            lotted: false,
        },
        k - i,
    ))
}

fn lex(ws: &[String], table: &AliasTable) -> Vec<Tk> {
    let mut out: Vec<Tk> = Vec::with_capacity(ws.len());
    let mut i = 0;
    while i < ws.len() {
        // 1. symbols through the alias table (longest key first)
        if let Some((len, symbol, key)) = table.match_at(ws, i) {
            i += len;
            // "s&p 500", "nasdaq 100", "dow 30": the trailing number belongs to the name
            if let Some(n) = ws
                .get(i)
                .and_then(|w| numeric(w))
                .filter(|n| n.int_digits.is_some())
            {
                let suffix = INDEX_SUFFIXES.iter().find(|s| f64::from(**s) == n.v);
                let belongs = suffix.is_some_and(|s| {
                    symbol.to_lowercase().ends_with(&s.to_string())
                        || (key.starts_with("nas") && *s == 100)
                });
                if belongs {
                    i += 1;
                }
            }
            out.push(Tk::S(symbol));
            continue;
        }
        // 2. bare broker symbols
        if let Some(symbol) = table.bare(&ws[i]) {
            out.push(Tk::S(symbol));
            i += 1;
            continue;
        }
        // 3. spoken / mixed decimals
        if let Some((mut num, used)) = spoken_decimal(ws, i) {
            i += used;
            if is_lot(ws.get(i)) {
                num.lotted = true;
                i += 1;
            }
            out.push(Tk::N(num));
            continue;
        }
        // 4. "half a lot", "aadha lot"
        if HALF_WORDS.contains(&ws[i].as_str()) {
            let mut k = i + 1;
            if ws.get(k).is_some_and(|w| w == "a" || w == "ek") {
                k += 1;
            }
            if is_lot(ws.get(k)) {
                out.push(Tk::N(Num {
                    v: 0.5,
                    int_digits: None,
                    decimal: true,
                    lotted: true,
                }));
                i = k + 1;
                continue;
            }
        }
        // 5. digit words before "lot(s)": "one lot", "ek lot", "do lot"
        if is_lot(ws.get(i + 1)) {
            let value = if ws[i] == "do" {
                Some(2)
            } else {
                digit_word(&ws[i])
            };
            if let Some(v) = value {
                out.push(Tk::N(Num {
                    v: f64::from(v),
                    int_digits: Some(1),
                    decimal: false,
                    lotted: true,
                }));
                i += 2;
                continue;
            }
        }
        // 6. plain numbers
        if let Some(mut num) = numeric(&ws[i]) {
            i += 1;
            if is_lot(ws.get(i)) {
                num.lotted = true;
                i += 1;
            }
            out.push(Tk::N(num));
            continue;
        }
        out.push(Tk::W(ws[i].clone()));
        i += 1;
    }
    resolve_ambiguous_words(&mut out);
    out
}

/// Speech-to-text slips and words that are only trading words in context:
/// "by 0.1 gold" (buy), "cell gold" (sell), "go long gold" but not "long term".
fn resolve_ambiguous_words(tk: &mut [Tk]) {
    for i in 0..tk.len() {
        let Tk::W(w) = &tk[i] else { continue };
        let before_a_fact = matches!(tk.get(i + 1), Some(Tk::N(_) | Tk::S(_)));
        let replacement = match w.as_str() {
            "by" | "bye" if before_a_fact => Some("buy"),
            "cell" | "sel" | "sale" if before_a_fact && !(i > 0 && tk[i - 1].is("for")) => {
                Some("sell")
            }
            "long" | "short" => {
                let not_a_side = tk
                    .get(i + 1)
                    .is_some_and(|n| n.is_any(&["term", "time", "run", "side"]));
                (!not_a_side).then_some(if w == "long" { "buy" } else { "sell" })
            }
            _ => None,
        };
        if let Some(r) = replacement {
            tk[i] = Tk::W(r.to_owned());
        }
    }
}

// ---- classification -------------------------------------------------------------------

/// Parse one utterance. `aliases` maps lowercase spoken names to broker symbols
/// (same table as `[symbols.aliases]`); bare broker symbols (e.g. "xauusd") are also
/// recognised. Numbers may be digits ("0.1"), words ("point one", "zero point five",
/// "half a lot", "ek lot", "aadha lot") and Hindi number words.
pub fn parse(text: &str, aliases: &HashMap<String, String>) -> Intent {
    let norm = normalize(text);
    let raw = words(&norm);
    if raw.is_empty() {
        return Intent::Other;
    }
    if let Some(answer) = confirm_or_cancel(&raw) {
        return answer;
    }
    let table = AliasTable::new(aliases);
    let mut tk = lex(&canonicalize(&raw), &table);
    let pending = settle_stop_words(&mut tk);
    classify(&tk, pending)
}

/// Decide what each "limit"/"stop" means. "buy stop" and "stop order" are pending
/// order types; "stop 3990", "stop to 3990" are the stop loss and become "sl".
/// Returns the pending order type and where it was said.
fn settle_stop_words(tk: &mut [Tk]) -> Option<(usize, ExecType)> {
    let mut pending = None;
    for i in 0..tk.len() {
        if tk[i].is("limit") && pending.is_none() {
            pending = Some((i, ExecType::Limit));
        } else if tk[i].is("stop") {
            let after_side = i > 0 && tk[i - 1].is_any(&["buy", "sell"]);
            let before_side = tk
                .get(i + 1)
                .is_some_and(|n| n.is_any(&["buy", "sell", "order", "orders"]));
            let level_follows = tk
                .get(i + 1)
                .is_some_and(|n| n.num().is_some() || n.is_any(&["to", "at"]));
            if after_side || before_side {
                if pending.is_none() {
                    pending = Some((i, ExecType::Stop));
                }
            } else if level_follows {
                tk[i] = Tk::W("sl".to_owned());
            }
        }
    }
    pending
}

fn classify(tk: &[Tk], pending: Option<(usize, ExecType)>) -> Intent {
    let has = |set: &[&str]| tk.iter().any(|t| t.is_any(set));
    let first = |set: &[&str]| tk.iter().position(|t| t.is_any(set));

    if has(&["cancel", "delete"]) && has(PENDING_WORDS) {
        return Intent::CancelOrders(close_facts(tk));
    }
    let close_at = tk.iter().enumerate().position(|(i, t)| {
        t.is_any(&["close", "exit"]) && !tk.get(i + 1).is_some_and(|n| n.is_any(NOT_A_CLOSE_VERB))
    });
    if close_at.is_some() {
        // "close all pending orders" means cancel them
        if has(&["pending", "pendings", "orders"])
            && !has(&["position", "positions", "trade", "trades"])
        {
            return Intent::CancelOrders(close_facts(tk));
        }
        return Intent::Close(close_facts(tk));
    }

    let side_at = first(&["buy", "sell"]);
    let modify_verb_at = first(EN_MODIFY_VERBS);
    let has_level_word = has(&["sl", "tp"]);
    let wants_modify = has_level_word
        && (modify_verb_at.is_some() || has(HI_MODIFY_VERBS))
        && match side_at {
            None => true,
            Some(side) => modify_verb_at.is_some_and(|verb| verb < side),
        };
    if wants_modify {
        return Intent::Modify(modify_facts(tk));
    }

    if let Some(side_idx) = side_at {
        let facts = order_facts(tk, side_idx, pending);
        if facts.symbol.is_some() || facts.volume.is_some() || facts.pending.is_some() {
            return Intent::Order(facts);
        }
    }
    Intent::Other
}

fn side_of(t: &Tk) -> Option<Side> {
    if t.is("buy") {
        Some(Side::Buy)
    } else if t.is("sell") {
        Some(Side::Sell)
    } else {
        None
    }
}

/// Tracks which numbers are already explained by something else.
struct Used(Vec<bool>);

impl Used {
    /// Numbers that are distances ("50 pips") are never prices or volumes.
    fn new(tk: &[Tk]) -> Self {
        Used(
            tk.iter()
                .enumerate()
                .map(|(i, t)| {
                    t.num().is_some() && tk.get(i + 1).is_some_and(|n| n.is_any(DISTANCE_UNITS))
                })
                .collect(),
        )
    }

    fn free(&self, tk: &[Tk], i: usize) -> Option<Num> {
        match tk.get(i) {
            Some(Tk::N(n)) if !self.0[i] => Some(n.clone()),
            _ => None,
        }
    }

    fn take(&mut self, tk: &[Tk], i: usize) -> Option<f64> {
        let n = self.free(tk, i)?;
        self.0[i] = true;
        Some(n.v)
    }
}

/// First free number after a keyword, skipping filler words ("sl at 3990").
fn number_after(tk: &[Tk], used: &mut Used, keyword_at: usize) -> Option<f64> {
    let mut j = keyword_at + 1;
    while tk.get(j).is_some_and(|t| t.is_any(LEVEL_FILLERS)) {
        j += 1;
    }
    if tk.get(j).and_then(Tk::num).is_some_and(|n| !n.lotted) {
        return used.take(tk, j);
    }
    None
}

/// Stop loss and take profit levels: the number after "sl"/"tp", or right before it.
fn levels(tk: &[Tk], used: &mut Used) -> (Option<f64>, Option<f64>) {
    let mut find = |keyword: &str| {
        let at = tk.iter().position(|t| t.is(keyword))?;
        number_after(tk, used, at).or_else(|| {
            let before = at.checked_sub(1)?;
            tk.get(before).and_then(Tk::num).filter(|n| !n.lotted)?;
            used.take(tk, before)
        })
    };
    let sl = find("sl");
    let tp = find("tp");
    (sl, tp)
}

fn symbol_of(tk: &[Tk]) -> Option<String> {
    tk.iter().find_map(|t| match t {
        Tk::S(s) => Some(s.clone()),
        _ => None,
    })
}

fn order_facts(tk: &[Tk], side_idx: usize, pending: Option<(usize, ExecType)>) -> OrderFacts {
    let mut used = Used::new(tk);
    let (sl, tp) = levels(tk, &mut used);

    // volume: "0.1 lots", "volume 0.1"
    let mut volume = (0..tk.len())
        .find(|&i| used.free(tk, i).is_some_and(|n| n.lotted))
        .and_then(|i| used.take(tk, i));
    if volume.is_none() {
        volume = (0..tk.len())
            .filter(|&i| tk[i].is_any(VOLUME_WORDS))
            .find_map(|i| number_after(tk, &mut used, i));
    }

    // explicit price: "at 2350", "price 1.1"
    let mut price = (0..tk.len())
        .filter(|&i| tk[i].is_any(PRICE_MARKERS))
        .find_map(|i| match tk.get(i + 1) {
            Some(Tk::N(n)) if !n.lotted => used.take(tk, i + 1),
            _ => None,
        });

    // volume by position: "buy 0.1 gold", "gold kharido 0.1", "0.1 gold becho"
    if volume.is_none() {
        let free_numbers = (0..tk.len())
            .filter(|&i| used.free(tk, i).is_some_and(|n| !n.lotted))
            .count();
        // a pending order needs a price, so a lone unmarked number is the price
        let lone_pending_number = pending.is_some() && price.is_none() && free_numbers == 1;
        if !lone_pending_number {
            let pending_at = pending.map(|(i, _)| i);
            let candidate = (0..tk.len()).find(|&i| {
                let Some(n) = used.free(tk, i) else {
                    return false;
                };
                if n.lotted || n.v <= 0.0 || n.v >= 100.0 {
                    return false;
                }
                let after_verb =
                    i > 0 && (side_of(&tk[i - 1]).is_some() || Some(i - 1) == pending_at);
                let before_symbol = matches!(tk.get(i + 1), Some(Tk::S(_)));
                let after_symbol = i > 0 && matches!(tk[i - 1], Tk::S(_)) && n.decimal;
                after_verb || before_symbol || after_symbol
            });
            volume = candidate.and_then(|i| used.take(tk, i));
        }
    }

    // pending order price without a marker: the first number after "limit"/"stop"
    if price.is_none()
        && let Some((at, _)) = pending
    {
        let after = (at + 1..tk.len()).find(|&i| used.free(tk, i).is_some_and(|n| !n.lotted));
        let any = (0..tk.len()).find(|&i| used.free(tk, i).is_some_and(|n| !n.lotted));
        price = after.or(any).and_then(|i| used.take(tk, i));
    }

    OrderFacts {
        side: side_of(&tk[side_idx]),
        symbol: symbol_of(tk),
        volume: volume.filter(|v| *v > 0.0),
        pending: pending.map(|(_, e)| e),
        price,
        sl,
        tp,
    }
}

/// Ticket numbers: integers after "ticket"/"position"/"order" (4+ digits) or any
/// free integer with 5+ digits.
fn tickets(tk: &[Tk], used: &mut Used) -> Vec<u64> {
    let mut found = Vec::new();
    for i in 0..tk.len() {
        let Some(n) = used.free(tk, i) else { continue };
        let Some(digits) = n.int_digits else { continue };
        let after_keyword = i > 0 && tk[i - 1].is_any(TICKET_WORDS);
        let after_marker = i > 0 && tk[i - 1].is_any(PRICE_MARKERS);
        if (after_keyword && digits >= 4) || (digits >= 5 && !after_marker) {
            // `n.v` is an exact integer read from at most 15-ish digits
            if let Some(ticket) = float_to_ticket(n.v) {
                used.0[i] = true;
                found.push(ticket);
            }
        }
    }
    found
}

fn float_to_ticket(v: f64) -> Option<u64> {
    (v.is_finite() && v >= 0.0 && v.fract() == 0.0 && v < 1.8e19).then_some(v as u64)
}

fn close_facts(tk: &[Tk]) -> CloseFacts {
    let mut used = Used::new(tk);
    let tickets = tickets(tk, &mut used);
    let all = tk.iter().any(|t| t.is_any(ALL_WORDS));
    let side = tk.iter().find_map(side_of);
    // volume: "0.05 lots", or a decimal such as "close 0.05 gold" (a bare "3" is a count)
    let volume = (0..tk.len())
        .find(|&i| {
            used.free(tk, i)
                .is_some_and(|n| n.v > 0.0 && n.v < 100.0 && (n.lotted || n.decimal))
        })
        .and_then(|i| used.take(tk, i));
    let half = volume.is_none() && tk.iter().any(|t| t.is_any(HALF_WORDS));
    CloseFacts {
        all,
        symbol: symbol_of(tk),
        side,
        tickets,
        volume,
        half,
    }
}

fn modify_facts(tk: &[Tk]) -> ModifyFacts {
    let mut used = Used::new(tk);
    let (sl, tp) = levels(tk, &mut used);
    let ticket = tickets(tk, &mut used).first().copied();
    ModifyFacts {
        symbol: symbol_of(tk),
        ticket,
        sl,
        tp,
    }
}

// ---- cross check ------------------------------------------------------------------------

/// A disagreement between what the user said and what the LLM proposed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{field}: you said {said}, but the proposal has {proposed}")]
pub struct Mismatch {
    /// One of `side`, `symbol`, `type`, `volume`, `price`, `sl`, `tp`, `selector`
    /// or `action` (the proposal is a different kind of action than the one asked for).
    pub field: &'static str,
    pub said: String,
    pub proposed: String,
}

fn mismatch(
    field: &'static str,
    said: impl ToString,
    proposed: impl ToString,
) -> Result<(), Mismatch> {
    Err(Mismatch {
        field,
        said: said.to_string(),
        proposed: proposed.to_string(),
    })
}

fn side_text(s: Side) -> &'static str {
    match s {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

fn exec_text(e: ExecType) -> &'static str {
    match e {
        ExecType::Market => "market",
        ExecType::Limit => "limit",
        ExecType::Stop => "stop",
    }
}

fn opt_text(v: Option<f64>) -> String {
    v.map_or_else(|| "not set".to_owned(), |v| v.to_string())
}

fn same_volume(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9
}

fn same_price(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs())
}

/// The user's symbol (an alias target such as "XAUUSD") against the proposed broker
/// name, which may carry the broker's suffix ("XAUUSD.m", "XAUUSDm").
fn same_symbol(said: &str, proposed: &str) -> bool {
    if said.eq_ignore_ascii_case(proposed) {
        return true;
    }
    let (s, p) = (said.to_lowercase(), proposed.to_lowercase());
    match p.strip_prefix(&s) {
        Some(rest) => {
            // Judge the suffix as the broker wrote it (case matters: "m" vs "X").
            let n = rest.chars().count();
            let suffix: Vec<char> = proposed.chars().rev().take(n).collect();
            suffix.last().is_some_and(|c| !c.is_alphanumeric())
                || (n <= 3 && suffix.iter().all(char::is_ascii_lowercase))
        }
        None => false,
    }
}

fn check_level(
    field: &'static str,
    said: Option<f64>,
    proposed: Option<f64>,
) -> Result<(), Mismatch> {
    match (said, proposed) {
        (None, _) => Ok(()),
        (Some(s), Some(p)) if same_price(s, p) => Ok(()),
        (Some(s), p) => mismatch(field, s, opt_text(p)),
    }
}

fn action_name(a: &ProposedAction) -> &'static str {
    match a {
        ProposedAction::Order(_) => "open a new trade",
        ProposedAction::Modify(_) => "change a stop loss or take profit",
        ProposedAction::Close(_) => "close positions",
        ProposedAction::Cancel(_) => "cancel pending orders",
    }
}

/// Reject a proposal that contradicts facts the user stated in `utterance_intent`.
/// Facts the user did not state are not checked (the LLM may fill them from context,
/// e.g. "close it" after discussing one position). `Other`/`Confirm`/`Cancel` intents
/// never block.
///
/// Notes:
/// - The proposal must be the same *kind* of action the user asked for (field
///   `action`): "buy gold" never justifies a close.
/// - A symbol matches when equal ignoring case, or when the proposal is the same name
///   plus a broker suffix (`XAUUSD.m`).
/// - A close/cancel proposal that lists explicit tickets cannot be checked against the
///   user's symbol, side or "all" (the gate resolves those from the account), so only
///   the tickets the user named are compared.
pub fn cross_check(utterance_intent: &Intent, proposal: &ProposedAction) -> Result<(), Mismatch> {
    match (utterance_intent, proposal) {
        (Intent::Other | Intent::Confirm | Intent::Cancel, _) => Ok(()),
        (Intent::Order(f), ProposedAction::Order(o)) => check_order(f, o),
        (Intent::Close(f), ProposedAction::Close(c)) => check_close(f, c),
        (Intent::CancelOrders(f), ProposedAction::Cancel(c)) => check_cancel(f, c),
        (Intent::Modify(f), ProposedAction::Modify(m)) => check_modify(f, m),
        (said, other) => {
            let said_name = match said {
                Intent::Order(_) => "open a new trade",
                Intent::Close(_) => "close positions",
                Intent::CancelOrders(_) => "cancel pending orders",
                _ => "change a stop loss or take profit",
            };
            mismatch("action", said_name, action_name(other))
        }
    }
}

fn check_order(f: &OrderFacts, o: &OrderIntent) -> Result<(), Mismatch> {
    if let Some(side) = f.side
        && side != o.side
    {
        return mismatch("side", side_text(side), side_text(o.side));
    }
    if let Some(symbol) = &f.symbol
        && !same_symbol(symbol, &o.symbol)
    {
        return mismatch("symbol", symbol, &o.symbol);
    }
    if let Some(kind) = f.pending
        && kind != o.exec
    {
        return mismatch("type", exec_text(kind), exec_text(o.exec));
    }
    if let Some(volume) = f.volume
        && !same_volume(volume, o.volume)
    {
        return mismatch("volume", volume, o.volume);
    }
    check_level("price", f.price, o.price)?;
    check_level("sl", f.sl, o.sl)?;
    check_level("tp", f.tp, o.tp)
}

/// Shared by close and cancel: compare what the user said they want to target with the
/// proposal's selector. `side` is `None` for cancels.
fn check_selector(
    f: &CloseFacts,
    tickets: &[u64],
    symbol: Option<&String>,
    side: Option<Side>,
    all: bool,
) -> Result<(), Mismatch> {
    if !f.tickets.is_empty() || !tickets.is_empty() {
        if !f.tickets.is_empty() {
            let mut said = f.tickets.clone();
            said.sort_unstable();
            said.dedup();
            let mut got = tickets.to_vec();
            got.sort_unstable();
            got.dedup();
            if said != got {
                return mismatch(
                    "selector",
                    format!("tickets {said:?}"),
                    format!("tickets {got:?}"),
                );
            }
        }
        return Ok(());
    }
    if let Some(said) = &f.symbol {
        match symbol {
            Some(p) if same_symbol(said, p) => {}
            other => {
                return mismatch(
                    "selector",
                    format!("symbol {said}"),
                    other.map_or_else(|| "no symbol filter".to_owned(), |p| format!("symbol {p}")),
                );
            }
        }
    }
    if let Some(said) = f.side
        && side != Some(said)
    {
        return mismatch(
            "selector",
            format!("{} positions", side_text(said)),
            side.map_or_else(
                || "both sides".to_owned(),
                |s| format!("{} positions", side_text(s)),
            ),
        );
    }
    if f.all && f.symbol.is_none() && f.side.is_none() && !all {
        return mismatch("selector", "all", "a narrower selection");
    }
    Ok(())
}

fn check_close(f: &CloseFacts, c: &CloseSelector) -> Result<(), Mismatch> {
    check_selector(f, &c.tickets, c.symbol.as_ref(), c.side, c.all)?;
    if let Some(said) = f.volume {
        match c.volume {
            Some(p) if same_volume(said, p) => {}
            other => {
                return mismatch(
                    "volume",
                    said,
                    opt_text(other).replace("not set", "the whole position"),
                );
            }
        }
    }
    if f.half && c.volume.is_none() {
        return mismatch("volume", "half", "the whole position");
    }
    Ok(())
}

fn check_cancel(f: &CloseFacts, c: &CancelSelector) -> Result<(), Mismatch> {
    // pending orders have no side; a side the user said cannot be compared
    let facts = CloseFacts {
        side: None,
        ..f.clone()
    };
    check_selector(&facts, &c.tickets, c.symbol.as_ref(), None, c.all)
}

fn check_modify(f: &ModifyFacts, m: &ModifyIntent) -> Result<(), Mismatch> {
    if let Some(ticket) = f.ticket
        && ticket != m.ticket
    {
        return mismatch(
            "selector",
            format!("ticket {ticket}"),
            format!("ticket {}", m.ticket),
        );
    }
    check_level("sl", f.sl, m.sl)?;
    check_level("tp", f.tp, m.tp)
}
