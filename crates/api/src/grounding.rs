//! Grounding check: every figure in the model's final reply must come from somewhere real.
//!
//! The prompt tells the model never to state a price, balance or any other number it did not
//! get from a tool. A small local model ignores that (it answered "gold is 2654.80" with no tool
//! call when the real price was near 3980), so the rule is enforced in code, for every persona
//! alike. After the model's last text reply of a turn, [`ungrounded_figures`] lists the figures
//! in it that appear in none of:
//!
//! - a tool result in the session history (this turn included),
//! - a message from the user in the session history,
//! - the two time strings of the system prompt.
//!
//! The agent loop gives the model one retry and then replaces the reply (see `agent.rs`).
//!
//! # What counts as a figure
//!
//! - any number with a decimal point (`0.01`, `3,980.29`, `.5`),
//! - any whole number of 100 or more (`2391`, `10,000`, `2026`),
//! - a whole number from 1 to 99 only with a money, percent or size context (`$12`, `12 USD`,
//!   `12%`, `12 lots`, `12 pips`), or with a magnitude (`5k`, `2 million`).
//!
//! # What is not
//!
//! Digits inside names (`US30`, `NAS100`, `MT5`, `M15`), clock times (`14:35`), the number in an
//! index name (`S&P 500`, `Nasdaq 100`), list markers (`1.` at the start of a line), dotted
//! chains such as dates and versions (`30.10.2026`, `v1.2.3`), and a plain zero.
//!
//! # How a figure is matched
//!
//! Signs are ignored (`loss of 12.40` matches `-12.4`). A figure matches a source number when it
//! equals that number rounded to the figure's own number of decimals, so `3980.29`, `3980.3`
//! and `3,980` all match `3980.285`; more precision than the source has does not match. A `k`
//! or `thousand`/`million` magnitude is applied first. Where a tool reports a bid and an ask,
//! their difference (the spread) counts as reported too.

use jarvis_llm::Message;
use serde_json::Value;

/// What the user is told when the model twice answered with figures no tool reported.
/// Persona-neutral on purpose: it is not model output.
pub const UNVERIFIED_REPLY: &str =
    "I couldn't verify that from MetaTrader just now, so I won't guess. Please ask again.";

/// The note the model gets after an ungrounded reply. It is an automatic server check, not
/// something the user said, so it says so, repeats the user's question, and asks for a
/// fresh answer without any mention of the check (the rejected reply is not kept).
pub fn correction_message(figures: &[String], question: &str) -> String {
    format!(
        "[Automatic check by the Arc MT server; this is not from the user.] A draft answer \
         contained figures that did not come from a tool in this conversation ({}). Do not \
         answer from memory. Call the appropriate read tool, then answer the user's question \
         using only tool results. Do not mention this check, do not apologise and do not \
         refer to any earlier draft. The user's question was: \"{}\"",
        figures.join(", "),
        question.trim()
    )
}

/// Currency words and symbols that make a small whole number a figure.
const CURRENCY_CODES: &[&str] = &["usd", "eur", "gbp", "jpy", "chf", "aud", "cad", "nzd"];
const CURRENCY_SYMBOLS: &[char] = &['$', '€', '£', '¥'];
/// Words after a number that make a small whole number a figure.
const UNIT_WORDS: &[&str] = &[
    "percent", "pct", "dollar", "dollars", "euro", "euros", "pound", "pounds", "cent", "cents",
    "lot", "lots", "pip", "pips", "point", "points", "pt", "pts",
];
/// Words in front of the number of an index name (`S&P 500`, `Nasdaq 100`).
const INDEX_WORDS: &[&str] = &[
    "nasdaq", "nas", "dow", "p", "sp", "spx", "ndx", "dax", "ftse", "nikkei", "russell", "cac",
    "stoxx", "street", "us", "uk", "ger", "de", "jp",
];
const INDEX_NUMBERS: &[f64] = &[30.0, 40.0, 50.0, 100.0, 225.0, 250.0, 500.0, 2000.0];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Find the figures a reply states.
    Reply,
    /// Find every number a source mentions.
    Source,
}

/// One number as written.
struct Token {
    /// As written (`3,980.29`, `5k`).
    text: String,
    /// The digits only, unscaled and unsigned.
    value: f64,
    /// 1000 for `5k` and `5 thousand`, and so on.
    scale: f64,
    decimals: usize,
    /// The groups of a number with thousands separators (`1,234` is `[1.0, 234.0]`).
    groups: Vec<f64>,
}

/// A figure stated in a reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Figure {
    /// As written in the reply.
    pub text: String,
    value: f64,
    tolerance: f64,
    groups: Vec<f64>,
}

/// Every number the model may legitimately repeat.
#[derive(Debug, Default, Clone)]
pub struct Sources {
    values: Vec<f64>,
}

impl Sources {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add every number in `text`.
    pub fn add_text(&mut self, text: &str) {
        for t in scan(text, Mode::Source) {
            self.values.push(t.value);
            if (t.scale - 1.0).abs() > f64::EPSILON {
                self.values.push(t.value * t.scale);
            }
            self.values.extend(t.groups);
        }
    }

    /// Add what a tool result reports and what follows from it directly: the spread of a
    /// price (ask minus bid).
    pub fn add_tool_result(&mut self, content: &str) {
        self.add_text(content);
        if let Ok(json) = serde_json::from_str::<Value>(content) {
            self.add_spreads(&json);
        }
    }

    fn add_spreads(&mut self, value: &Value) {
        match value {
            Value::Object(map) => {
                if let (Some(bid), Some(ask)) = (
                    map.get("bid").and_then(Value::as_f64),
                    map.get("ask").and_then(Value::as_f64),
                ) {
                    self.values.push((ask - bid).abs());
                }
                map.values().for_each(|v| self.add_spreads(v));
            }
            Value::Array(items) => items.iter().for_each(|v| self.add_spreads(v)),
            _ => {}
        }
    }

    fn sort(&mut self) {
        self.values.sort_by(f64::total_cmp);
        self.values.dedup();
    }

    /// Whether a source number lies within `tolerance` of `value`. Needs [`Self::sort`].
    fn near(&self, value: f64, tolerance: f64) -> bool {
        let first = self.values.partition_point(|v| *v < value - tolerance);
        self.values
            .get(first)
            .is_some_and(|v| *v <= value + tolerance)
    }
}

impl Figure {
    fn grounded(&self, sources: &Sources) -> bool {
        sources.near(self.value, self.tolerance)
            // `123,456` may be a list of two numbers rather than one with a separator.
            || (!self.groups.is_empty()
                && self
                    .groups
                    .iter()
                    .all(|g| sources.near(*g, 0.5 + 1e-7)))
    }
}

/// The sources for one turn: tool results and user messages of `history`, skipping the
/// indices in `skip` (the correction exchange, whose text repeats the offending figures), and
/// the time strings of the system prompt.
pub fn collect_sources(history: &[Message], skip: &[usize], time_text: &str) -> Sources {
    let mut sources = Sources::new();
    sources.add_text(time_text);
    for (i, message) in history.iter().enumerate() {
        if skip.contains(&i) {
            continue;
        }
        match message {
            Message::User { text } => sources.add_text(text),
            Message::ToolResult { content, .. } => sources.add_tool_result(content),
            Message::Assistant { .. } => {}
        }
    }
    sources.sort();
    sources
}

/// The figures stated in `reply`.
pub fn figures(reply: &str) -> Vec<Figure> {
    scan(reply, Mode::Reply)
        .into_iter()
        .map(|t| {
            let value = t.value * t.scale;
            let tolerance =
                0.5 * 10f64.powi(-i32::try_from(t.decimals).unwrap_or(0)) * t.scale + 1e-7;
            Figure {
                text: t.text,
                value,
                tolerance,
                groups: t.groups,
            }
        })
        .collect()
}

/// The figures of `reply` that no source reports, as written, without repeats.
/// `sources` must come from [`collect_sources`].
pub fn ungrounded_figures(reply: &str, sources: &Sources) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for figure in figures(reply) {
        if !figure.grounded(sources) && !out.contains(&figure.text) {
            out.push(figure.text);
        }
    }
    out
}

// ------------------------------------------------------------------------ scanner

fn take_digits(c: &[char], i: &mut usize) -> String {
    let start = *i;
    while *i < c.len() && c[*i].is_ascii_digit() {
        *i += 1;
    }
    c[start..*i].iter().collect()
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Skip a name that contains digits (`US30`, `NAS100`, `M15`, `v1.2.3`).
fn skip_identifier(c: &[char], mut i: usize) -> usize {
    let n = c.len();
    while i < n && is_word_char(c[i]) {
        i += 1;
    }
    while i + 1 < n && c[i] == '.' && c[i + 1].is_alphanumeric() {
        i += 1;
        while i < n && is_word_char(c[i]) {
            i += 1;
        }
    }
    i
}

/// The lowercase word that ends just before `start`, allowing one space between.
fn word_before(c: &[char], start: usize) -> String {
    let mut end = start;
    if end > 0 && c[end - 1].is_whitespace() {
        end -= 1;
    }
    let mut begin = end;
    while begin > 0 && c[begin - 1].is_alphabetic() {
        begin -= 1;
    }
    c[begin..end]
        .iter()
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

/// The symbol that ends just before `start`, allowing one space between.
fn symbol_before(c: &[char], start: usize) -> Option<char> {
    let mut end = start;
    if end > 0 && c[end - 1].is_whitespace() {
        end -= 1;
    }
    (end > 0).then(|| c[end - 1])
}

/// The lowercase word that starts at `from`, allowing one space first.
fn word_after(c: &[char], from: usize) -> (String, bool) {
    let mut i = from;
    if i < c.len() && c[i].is_whitespace() && c[i] != '\n' {
        i += 1;
    }
    if i < c.len() && c[i] == '%' {
        return ("%".to_owned(), true);
    }
    let begin = i;
    while i < c.len() && c[i].is_alphabetic() {
        i += 1;
    }
    (
        c[begin..i]
            .iter()
            .flat_map(|ch| ch.to_lowercase())
            .collect(),
        false,
    )
}

fn scan(text: &str, mode: Mode) -> Vec<Token> {
    let c: Vec<char> = text.chars().collect();
    let n = c.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let ch = c[i];
        let leading_dot = ch == '.'
            && i + 1 < n
            && c[i + 1].is_ascii_digit()
            && (i == 0 || !(is_word_char(c[i - 1]) || c[i - 1] == '.'));
        if !(ch.is_ascii_digit() || leading_dot) {
            i += 1;
            continue;
        }
        if mode == Mode::Reply
            && ch.is_ascii_digit()
            && i > 0
            && (c[i - 1].is_alphabetic() || c[i - 1] == '_')
        {
            i = skip_identifier(&c, i);
            continue;
        }

        let start = i;
        let mut int_part = String::new();
        let mut groups: Vec<f64> = Vec::new();
        if ch.is_ascii_digit() {
            let first = take_digits(&c, &mut i);
            let mut pieces = vec![first];
            // Thousands separators: 1-3 digits, then groups of exactly three.
            if pieces[0].len() <= 3 {
                while i + 3 < n
                    && c[i] == ','
                    && c[i + 1..=i + 3].iter().all(char::is_ascii_digit)
                    && !(i + 4 < n && c[i + 4].is_ascii_digit())
                {
                    pieces.push(c[i + 1..=i + 3].iter().collect());
                    i += 4;
                }
            }
            if pieces.len() > 1 {
                groups = pieces.iter().filter_map(|p| p.parse().ok()).collect();
            }
            int_part = pieces.concat();
        }
        let mut frac = String::new();
        if i + 1 < n && c[i] == '.' && c[i + 1].is_ascii_digit() {
            i += 1;
            frac = take_digits(&c, &mut i);
        }
        // A dotted chain is a date or a version, not a quantity: 30.10.2026, 1.2.3.
        if mode == Mode::Reply && i + 1 < n && c[i] == '.' && c[i + 1].is_ascii_digit() {
            while i < n
                && (c[i].is_ascii_digit()
                    || (c[i] == '.' && i + 1 < n && c[i + 1].is_ascii_digit()))
            {
                i += 1;
            }
            continue;
        }
        let end_number = i;
        // Only a run of whole numbers of three digits or more can be read as a list.
        if !frac.is_empty() || groups.iter().any(|g| *g < 100.0) {
            groups.clear();
        }
        let value: f64 = format!(
            "{}.{}",
            if int_part.is_empty() { "0" } else { &int_part },
            if frac.is_empty() { "0" } else { &frac }
        )
        .parse()
        .unwrap_or(0.0);

        // A magnitude: 5k, 5 thousand, 2 million.
        let mut scale = 1.0;
        let mut shown_end = end_number;
        let attached: String = c[end_number..]
            .iter()
            .take_while(|ch| ch.is_alphabetic())
            .flat_map(|ch| ch.to_lowercase())
            .collect();
        let (after, percent) = if attached.is_empty() {
            word_after(&c, end_number)
        } else {
            (attached.clone(), false)
        };
        if attached == "k" {
            scale = 1e3;
            shown_end = end_number + 1;
            i = shown_end;
        } else if !attached.is_empty() {
            i = end_number + attached.chars().count();
        } else {
            match after.as_str() {
                "thousand" => scale = 1e3,
                "million" => scale = 1e6,
                "billion" => scale = 1e9,
                _ => {}
            }
        }

        if mode == Mode::Source {
            out.push(Token {
                text: c[start..shown_end].iter().collect(),
                value,
                scale,
                decimals: frac.len(),
                groups,
            });
            continue;
        }

        // ---- Reply mode: decide whether this is a figure.
        let is_integer = frac.is_empty() && !leading_dot;
        // Clock times and ratios: 14:35.
        let after_colon = start >= 2 && c[start - 1] == ':' && c[start - 2].is_ascii_digit();
        let before_colon =
            end_number + 1 < n && c[end_number] == ':' && c[end_number + 1].is_ascii_digit();
        if after_colon || before_colon {
            continue;
        }
        // The number in an index name.
        if is_integer
            && groups.is_empty()
            && INDEX_NUMBERS.contains(&value)
            && INDEX_WORDS.contains(&word_before(&c, start).as_str())
        {
            continue;
        }
        // A list marker: `1.` or `2)` at the start of a line.
        if is_integer
            && groups.is_empty()
            && int_part.len() <= 2
            && end_number < n
            && matches!(c[end_number], '.' | ')')
            && (end_number + 1 == n || c[end_number + 1].is_whitespace())
        {
            let line_start = c[..start]
                .iter()
                .rposition(|ch| *ch == '\n')
                .map_or(0, |p| p + 1);
            if c[line_start..start]
                .iter()
                .all(|ch| ch.is_whitespace() || "-*•>#".contains(*ch))
            {
                continue;
            }
        }

        let money_before = symbol_before(&c, start).is_some_and(|s| CURRENCY_SYMBOLS.contains(&s))
            || CURRENCY_CODES.contains(&word_before(&c, start).as_str());
        let context = money_before
            || percent
            || CURRENCY_CODES.contains(&after.as_str())
            || UNIT_WORDS.contains(&after.as_str());
        let figure = !is_integer || scale > 1.0 || value >= 100.0 || (context && value >= 1.0);
        if !figure {
            continue;
        }
        out.push(Token {
            text: c[start..shown_end].iter().collect(),
            value,
            scale,
            decimals: frac.len(),
            groups,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(texts: &[&str]) -> Sources {
        let mut s = Sources::new();
        for t in texts {
            s.add_tool_result(t);
        }
        s.sort();
        s
    }

    /// `(reply, source texts, figures that must be reported as ungrounded)`.
    #[allow(clippy::type_complexity)]
    fn table() -> Vec<(&'static str, Vec<&'static str>, Vec<&'static str>)> {
        let price = r#"{"symbol":"XAUUSD","bid":3980.29,"ask":3980.57,"last":0.0,"spread_points":28,"digits":2,"time":1793341200}"#;
        let rounding = r#"{"bid":3980.285,"profit":-12.4,"balance":10000.0,"equity":10070.55}"#;
        vec![
            // 1-4: the failure that was found: a price with no source at all
            (
                "Gold is currently trading at 2654.80 USD per ounce.",
                vec![],
                vec!["2654.80"],
            ),
            ("Gold is trading at 2654.80.", vec![price], vec!["2654.80"]),
            ("Gold is 3980.29 bid.", vec![price], vec![]),
            ("Gold is 3980.29 bid and 3980.57 ask.", vec![price], vec![]),
            // 5-8: rounding to fewer decimals
            ("It is 3980.29.", vec![rounding], vec![]),
            ("It is 3980.3.", vec![rounding], vec![]),
            ("It is 3,980.", vec![rounding], vec![]),
            ("It is 3980.", vec![rounding], vec![]),
            // 9-10: more precision than the source, or a different number
            ("It is 3980.2851.", vec![rounding], vec!["3980.2851"]),
            ("It is 3981.", vec![rounding], vec!["3981"]),
            // 11-14: thousands separators
            ("Your balance is 10,000.00 dollars.", vec![rounding], vec![]),
            ("Your balance is 10000.", vec![rounding], vec![]),
            ("Your equity is 10,070.55.", vec![rounding], vec![]),
            (
                "Your equity is 10,070.56.",
                vec![rounding],
                vec!["10,070.56"],
            ),
            // 15-17: signs
            ("You lost 12.40 on it.", vec![rounding], vec![]),
            ("Profit of -12.4.", vec![rounding], vec![]),
            ("Profit of +12.40.", vec![rounding], vec![]),
            // 18-21: percent and currency
            ("That is 12% of the balance.", vec![], vec!["12"]),
            ("That is 12% of the balance.", vec!["12"], vec![]),
            ("That costs $12.", vec![], vec!["12"]),
            ("That costs 12 USD.", vec![], vec!["12"]),
            // 22-24: lots and pips
            ("You hold 12 lots.", vec![], vec!["12"]),
            ("Stop loss is 50 pips away.", vec![], vec!["50"]),
            ("Volume is 0.30 lots.", vec![], vec!["0.30"]),
            // 25-27: small whole numbers without context are not figures
            (
                "You have 2 open positions and 1 pending order.",
                vec![],
                vec![],
            ),
            ("Give me 15 seconds, it is the 30th.", vec![], vec![]),
            ("Three of the 12 symbols are up.", vec![], vec![]),
            // 28-31: names with digits
            ("US30 and US500 are in your watchlist.", vec![], vec![]),
            ("XAUUSD, EURUSD_ and NAS100 are open.", vec![], vec![]),
            (
                "I run on MT5 with the M15 chart and US100.cash.",
                vec![],
                vec![],
            ),
            (
                "The S&P 500, the Nasdaq 100 and the Dow 30 are indices.",
                vec![],
                vec![],
            ),
            // 32-34: list markers
            ("1. Gold\n2. Nasdaq\n3. Dow", vec![], vec![]),
            ("Open:\n  - 1. XAUUSD\n  - 2. US100", vec![], vec![]),
            ("1) first\n2) second", vec![], vec![]),
            // 35-38: dates and times, with the system prompt's time strings as a source
            (
                "It is 14:35 on 2026-10-30.",
                vec![
                    "Current time: 2026-10-30T10:00:00Z UTC. MT5 broker server time: 2026-10-30 13:00:00",
                ],
                vec![],
            ),
            ("The year is 2026.", vec!["2026-10-30T10:00:00Z"], vec![]),
            (
                "The year is 2031.",
                vec!["2026-10-30T10:00:00Z"],
                vec!["2031"],
            ),
            ("Next review on 30.10.2026 with v1.2.3.", vec![], vec![]),
            // 39-41: the user's own numbers
            ("Buying 0.01 lots of gold.", vec!["buy 0.01 gold"], vec![]),
            (
                "Stop loss at 2300 and take profit at 2350.5.",
                vec!["buy 0.1 gold sl 2300 tp 2350.5"],
                vec![],
            ),
            ("Stop loss at 2300.", vec!["buy 0.1 gold"], vec!["2300"]),
            // 42-44: magnitudes
            ("About 10k in the account.", vec![rounding], vec![]),
            ("About 10.1k in the account.", vec![rounding], vec![]),
            (
                "About 11 thousand in the account.",
                vec![rounding],
                vec!["11"],
            ),
            // 45-47: a derived spread, then a made-up one
            ("The spread is 0.28.", vec![price], vec![]),
            ("The spread is 28 points.", vec![price], vec![]),
            ("The spread is 0.31.", vec![price], vec!["0.31"]),
            // 48-50: lists of numbers, repeats, leading dot, zero
            (
                "Tickets 100,200 are open.",
                vec![r#"{"tickets":[100,200]}"#],
                vec![],
            ),
            (
                "Prices 3980.1 and 3980.1 and 4000.1.",
                vec![],
                vec!["3980.1", "4000.1"],
            ),
            ("Up .5 percent, with 0 lots open.", vec!["0.5"], vec![]),
            // 51-52: no figures at all
            ("Which symbol do you mean?", vec![], vec![]),
            ("I couldn't verify that, so I won't guess.", vec![], vec![]),
        ]
    }

    #[test]
    fn extractor_and_matcher_table() {
        let cases = table();
        assert!(cases.len() >= 30, "{}", cases.len());
        for (reply, source_texts, expected) in cases {
            let s = sources(&source_texts);
            let got = ungrounded_figures(reply, &s);
            assert_eq!(got, expected, "reply: {reply:?} sources: {source_texts:?}");
        }
    }

    fn texts(reply: &str) -> Vec<String> {
        figures(reply).into_iter().map(|f| f.text).collect()
    }

    #[test]
    fn extractor_reports_figures_as_written() {
        assert_eq!(texts("Bid 3,980.29, ask 3980.57."), ["3,980.29", "3980.57"]);
        assert_eq!(texts("Loss of -12.40 USD."), ["12.40"]);
        assert_eq!(texts("It rose 1.5% to 2391."), ["1.5", "2391"]);
        assert_eq!(texts("Up .5"), [".5"]);
        assert_eq!(texts("US30 at 41000.5"), ["41000.5"]);
        assert_eq!(texts("EURUSD_ 1.08123"), ["1.08123"]);
        assert!(texts("Only 2 of 3, at 14:35, on the 1st.").is_empty());
    }

    #[test]
    fn a_list_of_numbers_is_not_one_big_number() {
        // 123,456 reads as one number with a separator, and as two numbers.
        let s = sources(&[r#"{"tickets":[123,456]}"#]);
        assert!(ungrounded_figures("Tickets 123,456.", &s).is_empty());
        let s = sources(&[r#"{"tickets":[123,457]}"#]);
        assert_eq!(ungrounded_figures("Tickets 123,456.", &s), ["123,456"]);
    }

    #[test]
    fn the_correction_exchange_is_not_a_source() {
        let history = vec![
            Message::User {
                text: "gold?".into(),
            },
            Message::Assistant {
                text: Some("Gold is 2654.80.".into()),
                tool_calls: vec![],
            },
            Message::User {
                text: correction_message(&["2654.80".to_owned()], "gold?"),
            },
        ];
        let with_all = collect_sources(&history, &[], "");
        assert!(ungrounded_figures("Gold is 2654.80.", &with_all).is_empty());
        let without = collect_sources(&history, &[2], "");
        assert_eq!(
            ungrounded_figures("Gold is 2654.80.", &without),
            ["2654.80"]
        );
    }

    #[test]
    fn assistant_text_is_never_a_source() {
        let history = vec![
            Message::User {
                text: "gold?".into(),
            },
            Message::Assistant {
                text: Some("Gold is 2654.80.".into()),
                tool_calls: vec![],
            },
        ];
        let s = collect_sources(&history, &[], "");
        assert_eq!(ungrounded_figures("Gold is 2654.80.", &s), ["2654.80"]);
    }

    #[test]
    fn tool_results_and_user_messages_are_sources() {
        let history = vec![
            Message::User {
                text: "buy 0.01 gold".into(),
            },
            Message::ToolResult {
                call_id: "1".into(),
                name: "get_price".into(),
                content: r#"{"bid":3980.29,"ask":3980.57}"#.into(),
                is_error: false,
            },
        ];
        let s = collect_sources(&history, &[], "Current time: 2026-10-30T10:00:00Z");
        assert!(ungrounded_figures("0.01 lots at 3980.57, spread 0.28, in 2026.", &s).is_empty());
    }

    #[test]
    fn the_correction_text_lists_the_figures() {
        let m = correction_message(&["2654.80".into(), "12".into()], "  what's gold at? ");
        assert!(m.contains("(2654.80, 12)"));
        assert!(m.contains("Do not answer from memory"));
        assert!(m.contains("using only tool results"));
        assert!(m.contains("not from the user"));
        assert!(m.contains("do not apologise"));
        assert!(m.ends_with("The user's question was: \"what's gold at?\""));
    }
}
