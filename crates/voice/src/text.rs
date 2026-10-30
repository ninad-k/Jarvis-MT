//! Transcript clean-up: word-level corrections for common trading mishearings.

use crate::Corrections;

/// Common mishearings in trading speech: cold→gold, bold→gold, goal→gold,
/// nas daq→nasdaq, take profits→take profit, stop lost→stop loss, lot's→lots, ...
///
/// Context-dependent fixes (`by`/`bye`→`buy`, `cell`→`sell`) are not in this list
/// because they need to look at the following word; [`normalize_transcript`] always
/// applies them.
pub fn default_corrections() -> Corrections {
    const PAIRS: &[(&str, &str)] = &[
        // Instruments.
        ("cold", "gold"),
        ("bold", "gold"),
        ("goal", "gold"),
        ("gol", "gold"),
        ("gould", "gold"),
        ("nas daq", "nasdaq"),
        ("nas dack", "nasdaq"),
        ("nas dac", "nasdaq"),
        ("nasdack", "nasdaq"),
        ("nasdac", "nasdaq"),
        // Order fields.
        ("stop lost", "stop loss"),
        ("stoploss", "stop loss"),
        ("take profits", "take profit"),
        ("takeprofit", "take profit"),
        ("lot's", "lots"),
        ("lots'", "lots"),
        // Confirmation.
        ("conform", "confirm"),
        ("confirmed", "confirm"),
    ];
    PAIRS
        .iter()
        .map(|(from, to)| ((*from).to_owned(), (*to).to_owned()))
        .collect()
}

/// Words that, when they follow a misheard order verb, show the speaker meant a trade:
/// instruments and spelled-out numbers (digits are checked separately).
const SYMBOL_WORDS: &[&str] = &[
    "gold", "silver", "nasdaq", "dow", "bitcoin", "btc", "ethereum", "eth", "oil", "euro", "pound",
    "yen", "dollar", "xauusd", "xagusd", "ustec", "us30", "us500", "eurusd", "gbpusd", "usdjpy",
    "btcusd", "sona", "chandi",
];

const NUMBER_WORDS: &[&str] = &[
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "point",
    "half", "quarter",
];

/// Words before `by` that make it an ordinary preposition ("multiplied by 2").
const BY_BLOCKERS: &[&str] = &[
    "multiplied",
    "divided",
    "increase",
    "increased",
    "decrease",
    "decreased",
    "reduced",
    "raised",
    "up",
    "down",
    "followed",
    "rounded",
    "side",
    "step",
    "stop",
    "moved",
    "one",
];

/// (misheard word, replacement) applied only when the next word is a number or symbol.
const CONTEXT_RULES: &[(&str, &str)] = &[("by", "buy"), ("bye", "buy"), ("cell", "sell")];

/// Apply `corrections` to a transcript (whole-word, case-insensitive, keeps the rest).
///
/// Keys may contain several words (separated by whitespace in the text). The longest
/// matching key wins at each position and replaced text is not scanned again. A
/// capitalised match keeps its capital ("Cold" → "Gold"). After the list, the
/// context rules turn `by`/`bye` into `buy` and `cell` into `sell`, but only when the
/// next word is a number or an instrument name ("by 0.1 gold", "cell nasdaq").
pub fn normalize_transcript(text: &str, corrections: &Corrections) -> String {
    let rules = compile_rules(corrections);
    let first = apply_rules(text, &rules);
    apply_context_rules(&first)
}

// --- Tokeniser ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Word {
    /// Byte range of the whole run of letters, digits and apostrophes.
    start: usize,
    end: usize,
}

fn is_apostrophe(c: char) -> bool {
    c == '\'' || c == '\u{2019}' || c == '\u{2018}'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || is_apostrophe(c)
}

/// Runs of word characters that contain at least one letter or digit.
fn words(text: &str) -> Vec<Word> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        if is_word_char(c) {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            out.push(Word { start: s, end: i });
        }
    }
    if let Some(s) = start {
        out.push(Word {
            start: s,
            end: text.len(),
        });
    }
    out.retain(|w| text[w.start..w.end].chars().any(char::is_alphanumeric));
    out
}

/// Lower-case with typographic apostrophes folded to `'`.
fn fold(s: &str) -> String {
    s.chars()
        .map(|c| if is_apostrophe(c) { '\'' } else { c })
        .flat_map(char::to_lowercase)
        .collect()
}

/// The byte range of `w` without leading and trailing apostrophes (quotes).
fn core_range(text: &str, w: Word) -> (usize, usize) {
    let s = &text[w.start..w.end];
    let trimmed_start = s.trim_start_matches(is_apostrophe);
    let lead = s.len() - trimmed_start.len();
    let trimmed = trimmed_start.trim_end_matches(is_apostrophe);
    (w.start + lead, w.start + lead + trimmed.len())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    /// The whole token matched (apostrophes included).
    Full,
    /// Matched once surrounding quote marks were removed.
    Core,
}

fn word_hit(text: &str, w: Word, key_word: &str) -> Option<Hit> {
    if fold(&text[w.start..w.end]) == key_word {
        return Some(Hit::Full);
    }
    if !key_word.contains('\'') {
        let (s, e) = core_range(text, w);
        if fold(&text[s..e]) == key_word {
            return Some(Hit::Core);
        }
    }
    None
}

// --- List corrections --------------------------------------------------------------

struct Rule {
    key: Vec<String>,
    replacement: String,
}

fn compile_rules(corrections: &Corrections) -> Vec<Rule> {
    corrections
        .iter()
        .filter_map(|(from, to)| {
            let key: Vec<String> = from.split_whitespace().map(fold).collect();
            if key.is_empty() {
                None
            } else {
                Some(Rule {
                    key,
                    replacement: to.clone(),
                })
            }
        })
        .collect()
}

fn only_whitespace_between(text: &str, a: Word, b: Word) -> bool {
    text[a.end..b.start].chars().all(char::is_whitespace)
}

/// Match `rule` at word index `i`; returns the byte range to replace.
fn match_rule(text: &str, ws: &[Word], i: usize, rule: &Rule) -> Option<(usize, usize)> {
    let n = rule.key.len();
    if i + n > ws.len() {
        return None;
    }
    let mut first_hit = Hit::Full;
    let mut last_hit = Hit::Full;
    for (k, key_word) in rule.key.iter().enumerate() {
        let w = ws[i + k];
        if k > 0 && !only_whitespace_between(text, ws[i + k - 1], w) {
            return None;
        }
        let hit = word_hit(text, w, key_word)?;
        if k == 0 {
            first_hit = hit;
        }
        if k == n - 1 {
            last_hit = hit;
        }
    }
    let first = ws[i];
    let last = ws[i + n - 1];
    let start = if first_hit == Hit::Core {
        core_range(text, first).0
    } else {
        first.start
    };
    let end = if last_hit == Hit::Core {
        core_range(text, last).1
    } else {
        last.end
    };
    Some((start, end))
}

/// Give `replacement` the capital of the text it replaces.
fn with_case_of(original: &str, replacement: &str) -> String {
    let capitalised = original.chars().next().is_some_and(char::is_uppercase);
    let mut chars = replacement.chars();
    match chars.next() {
        Some(first) if capitalised && first.is_lowercase() => {
            first.to_uppercase().chain(chars).collect()
        }
        _ => replacement.to_owned(),
    }
}

fn apply_rules(text: &str, rules: &[Rule]) -> String {
    if rules.is_empty() {
        return text.to_owned();
    }
    let ws = words(text);
    let mut out = String::with_capacity(text.len());
    let mut copied = 0usize;
    let mut i = 0usize;
    while i < ws.len() {
        // Longest key wins; the earlier entry wins a tie.
        let mut best: Option<(usize, usize, usize, &Rule)> = None;
        for rule in rules {
            if let Some((s, e)) = match_rule(text, &ws, i, rule) {
                let better = best.is_none_or(|(_, _, n, _)| rule.key.len() > n);
                if better {
                    best = Some((s, e, rule.key.len(), rule));
                }
            }
        }
        match best {
            Some((s, e, n, rule)) => {
                out.push_str(&text[copied..s]);
                out.push_str(&with_case_of(&text[s..e], &rule.replacement));
                copied = e;
                i += n;
            }
            None => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

// --- Context rules -----------------------------------------------------------------

fn looks_like_number(word: &str) -> bool {
    word.chars().next().is_some_and(|c| c.is_ascii_digit()) || NUMBER_WORDS.contains(&word)
}

fn is_order_operand(word: &str) -> bool {
    looks_like_number(word) || SYMBOL_WORDS.contains(&word)
}

fn apply_context_rules(text: &str) -> String {
    let ws = words(text);
    let mut out = String::with_capacity(text.len());
    let mut copied = 0usize;
    for (i, &w) in ws.iter().enumerate() {
        let (cs, ce) = core_range(text, w);
        let folded = fold(&text[cs..ce]);
        let Some(&(_, replacement)) = CONTEXT_RULES.iter().find(|(from, _)| *from == folded) else {
            continue;
        };
        let Some(&next) = ws.get(i + 1) else { continue };
        if !only_whitespace_between(text, w, next) {
            continue;
        }
        let (ns, ne) = core_range(text, next);
        if !is_order_operand(&fold(&text[ns..ne])) {
            continue;
        }
        if let Some(&prev) = i.checked_sub(1).and_then(|p| ws.get(p)) {
            let (ps, pe) = core_range(text, prev);
            if BY_BLOCKERS.contains(&fold(&text[ps..pe]).as_str()) {
                continue;
            }
        }
        out.push_str(&text[copied..cs]);
        out.push_str(&with_case_of(&text[cs..ce], replacement));
        copied = ce;
    }
    out.push_str(&text[copied..]);
    out
}

// --- Raw output clean-up -----------------------------------------------------------

/// Remove non-speech annotations such as `[BLANK_AUDIO]`, `[MUSIC]` and `(music)` and
/// collapse whitespace. The raw text is kept separately for the audit log.
#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
pub(crate) fn strip_annotations(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut depth_square = 0usize;
    let mut depth_round = 0usize;
    for c in raw.chars() {
        match c {
            '[' => depth_square += 1,
            ']' if depth_square > 0 => depth_square -= 1,
            '(' => depth_round += 1,
            ')' if depth_round > 0 => depth_round -= 1,
            _ if depth_square == 0 && depth_round == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(text: &str) -> String {
        normalize_transcript(text, &default_corrections())
    }

    // --- list corrections ---

    #[test]
    fn cold_becomes_gold() {
        assert_eq!(norm("buy 0.1 cold"), "buy 0.1 gold");
    }

    #[test]
    fn bold_becomes_gold() {
        assert_eq!(norm("sell bold now"), "sell gold now");
    }

    #[test]
    fn goal_becomes_gold() {
        assert_eq!(norm("close all goal positions"), "close all gold positions");
    }

    #[test]
    fn gol_becomes_gold() {
        assert_eq!(norm("show gol"), "show gold");
    }

    #[test]
    fn nas_daq_two_words() {
        assert_eq!(norm("buy nas daq"), "buy nasdaq");
    }

    #[test]
    fn nasdack_one_word() {
        assert_eq!(norm("sell nasdack"), "sell nasdaq");
    }

    #[test]
    fn multi_word_key_tolerates_extra_whitespace() {
        assert_eq!(norm("buy nas   daq"), "buy nasdaq");
    }

    #[test]
    fn stop_lost() {
        assert_eq!(norm("with stop lost 2300"), "with stop loss 2300");
    }

    #[test]
    fn take_profits() {
        assert_eq!(norm("take profits at 2400"), "take profit at 2400");
    }

    #[test]
    fn lot_apostrophe_s() {
        assert_eq!(norm("two lot's of gold"), "two lots of gold");
    }

    #[test]
    fn lots_trailing_apostrophe() {
        assert_eq!(norm("two lots' of gold"), "two lots of gold");
    }

    #[test]
    fn curly_apostrophe_matches() {
        assert_eq!(norm("two lot\u{2019}s of gold"), "two lots of gold");
    }

    #[test]
    fn conform_becomes_confirm() {
        assert_eq!(norm("conform"), "confirm");
    }

    #[test]
    fn confirmed_with_period_keeps_punctuation() {
        assert_eq!(norm("Confirmed."), "Confirm.");
    }

    #[test]
    fn case_insensitive_match_keeps_leading_capital() {
        assert_eq!(norm("Cold is up"), "Gold is up");
        assert_eq!(norm("COLD"), "Gold");
    }

    #[test]
    fn whole_words_only() {
        assert_eq!(norm("golden goalpost bolder"), "golden goalpost bolder");
        assert_eq!(norm("scold"), "scold");
    }

    #[test]
    fn untouched_text_is_preserved_exactly() {
        let s = "  Show my positions, please!  Equity: 1,234.50 ";
        assert_eq!(norm(s), s);
    }

    #[test]
    fn punctuation_around_match_is_kept() {
        assert_eq!(norm("(cold), cold. \"cold\"!"), "(gold), gold. \"gold\"!");
    }

    #[test]
    fn quotes_around_match_are_kept() {
        assert_eq!(norm("'cold'"), "'gold'");
    }

    #[test]
    fn multiple_matches_in_one_text() {
        assert_eq!(
            norm("cold and bold and nas daq"),
            "gold and gold and nasdaq"
        );
    }

    #[test]
    fn adjacent_multi_word_keys() {
        assert_eq!(norm("stop lost take profits"), "stop loss take profit");
    }

    #[test]
    fn multi_word_key_does_not_span_punctuation() {
        assert_eq!(norm("nas, daq"), "nas, daq");
        assert_eq!(norm("stop. lost"), "stop. lost");
    }

    #[test]
    fn replacement_is_not_rescanned() {
        let c: Corrections = vec![
            ("a".to_owned(), "b".to_owned()),
            ("b".to_owned(), "c".to_owned()),
        ];
        assert_eq!(normalize_transcript("a b", &c), "b c");
    }

    #[test]
    fn longest_key_wins() {
        let c: Corrections = vec![
            ("stop".to_owned(), "halt".to_owned()),
            ("stop lost".to_owned(), "stop loss".to_owned()),
        ];
        assert_eq!(normalize_transcript("stop lost", &c), "stop loss");
        assert_eq!(normalize_transcript("stop it", &c), "halt it");
    }

    #[test]
    fn custom_list_and_empty_list() {
        let c: Corrections = vec![("sona".to_owned(), "gold".to_owned())];
        assert_eq!(normalize_transcript("sona kharido", &c), "gold kharido");
        assert_eq!(normalize_transcript("cold", &Vec::new()), "cold");
    }

    #[test]
    fn empty_keys_are_ignored() {
        let c: Corrections = vec![
            (String::new(), "x".to_owned()),
            ("  ".to_owned(), "y".to_owned()),
        ];
        assert_eq!(normalize_transcript("hello", &c), "hello");
    }

    #[test]
    fn empty_text() {
        assert_eq!(norm(""), "");
    }

    #[test]
    fn unicode_text_survives() {
        assert_eq!(norm("sona \u{20b9}500 cold"), "sona \u{20b9}500 gold");
    }

    // --- context rules ---

    #[test]
    fn by_before_number_is_buy() {
        assert_eq!(norm("by 0.1 gold"), "buy 0.1 gold");
        assert_eq!(norm("please by 2 lots"), "please buy 2 lots");
    }

    #[test]
    fn by_before_spelled_number_is_buy() {
        assert_eq!(norm("by point one lot"), "buy point one lot");
        assert_eq!(norm("by zero point five"), "buy zero point five");
    }

    #[test]
    fn by_before_symbol_is_buy() {
        assert_eq!(norm("by gold"), "buy gold");
        assert_eq!(norm("by xauusd"), "buy xauusd");
        assert_eq!(norm("by nasdaq at market"), "buy nasdaq at market");
    }

    #[test]
    fn by_before_misheard_symbol_is_buy() {
        assert_eq!(norm("by cold"), "buy gold");
        assert_eq!(norm("by nas daq"), "buy nasdaq");
    }

    #[test]
    fn by_in_ordinary_use_is_untouched() {
        assert_eq!(
            norm("the order was placed by me"),
            "the order was placed by me"
        );
        assert_eq!(norm("by the way"), "by the way");
        assert_eq!(norm("by"), "by");
        assert_eq!(norm("sorted by"), "sorted by");
    }

    #[test]
    fn by_after_blockers_is_untouched() {
        assert_eq!(norm("multiplied by 2"), "multiplied by 2");
        assert_eq!(norm("increase by 10 percent"), "increase by 10 percent");
    }

    #[test]
    fn bye_before_number_or_symbol_is_buy() {
        assert_eq!(norm("bye 0.2 gold"), "buy 0.2 gold");
        assert_eq!(norm("bye nasdaq"), "buy nasdaq");
        assert_eq!(norm("okay bye"), "okay bye");
        assert_eq!(norm("bye jarvis"), "bye jarvis");
    }

    #[test]
    fn cell_before_number_or_symbol_is_sell() {
        assert_eq!(norm("cell 0.1 gold"), "sell 0.1 gold");
        assert_eq!(norm("cell gold"), "sell gold");
        assert_eq!(norm("Cell silver"), "Sell silver");
        assert_eq!(norm("my cell phone"), "my cell phone");
    }

    #[test]
    fn context_rule_needs_adjacent_word() {
        assert_eq!(norm("by, 2 lots"), "by, 2 lots");
    }

    #[test]
    fn context_rules_apply_with_empty_list() {
        assert_eq!(normalize_transcript("by 1 gold", &Vec::new()), "buy 1 gold");
    }

    #[test]
    fn full_phrase() {
        assert_eq!(
            norm("By point one lot of cold with stop lost 3990 and take profits 4010"),
            "Buy point one lot of gold with stop loss 3990 and take profit 4010"
        );
    }

    // --- annotations ---

    #[test]
    fn strips_blank_audio_and_music() {
        assert_eq!(strip_annotations("[BLANK_AUDIO]"), "");
        assert_eq!(
            strip_annotations(" buy gold [MUSIC] now (music) "),
            "buy gold now"
        );
        assert_eq!(strip_annotations("a  b\n c"), "a b c");
        assert_eq!(
            strip_annotations("stray ] and ) marks"),
            "stray ] and ) marks"
        );
    }
}
