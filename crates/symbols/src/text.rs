//! Small text helpers shared by the catalog, registry and auto-mapper.

use std::sync::OnceLock;

use regex::Regex;

/// Lower-case `s` and reduce it to space-separated alphanumeric tokens, padded with one
/// space on each side so a phrase can be found with a plain `contains`:
///
/// - `&` becomes the word `and` (`S&P 500` and `Standard & Poor's` both normalise);
/// - letter/digit boundaries split (`US30` → `us 30`, `Nasdaq100` → `nasdaq 100`);
/// - everything else that is not a letter or digit is a separator.
pub(crate) fn normalize_phrase(s: &str) -> String {
    #[derive(PartialEq, Clone, Copy)]
    enum Kind {
        Alpha,
        Digit,
        Other,
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push(' ');
    let mut last = Kind::Other;
    for ch in s.chars() {
        if ch == '&' {
            if !out.ends_with(' ') {
                out.push(' ');
            }
            out.push_str("and ");
            last = Kind::Other;
            continue;
        }
        let kind = if ch.is_alphabetic() {
            Kind::Alpha
        } else if ch.is_ascii_digit() {
            Kind::Digit
        } else {
            Kind::Other
        };
        match kind {
            Kind::Other => {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            _ => {
                if last != kind && last != Kind::Other && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.extend(ch.to_lowercase());
            }
        }
        last = kind;
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

/// The tokens of a string normalised by [`normalize_phrase`].
pub(crate) fn tokens(normalized: &str) -> impl Iterator<Item = &str> {
    normalized.split(' ').filter(|t| !t.is_empty())
}

fn month_token(t: &str) -> bool {
    matches!(
        t,
        "january"
            | "february"
            | "march"
            | "april"
            | "may"
            | "june"
            | "july"
            | "august"
            | "september"
            | "october"
            | "november"
            | "december"
            | "jan"
            | "feb"
            | "mar"
            | "apr"
            | "jun"
            | "jul"
            | "aug"
            | "sep"
            | "sept"
            | "oct"
            | "nov"
            | "dec"
    )
}

fn year_token(t: &str) -> bool {
    t.len() == 4
        && t.starts_with("20")
        && t.as_bytes()[2] >= b'2'
        && t.as_bytes()[2] <= b'3'
        && t.bytes().all(|b| b.is_ascii_digit())
}

/// True when a normalised description reads like a dated futures contract: it says
/// "futures"/"expiry" **and** names a month or a year ("Gold Futures - December 2026",
/// "Index Futures - Expiry: September 2026"). A bare "Futures" is not dated.
pub(crate) fn describes_dated_future(norm_desc: &str) -> bool {
    let mut futures = false;
    let mut dated = false;
    for t in tokens(norm_desc) {
        if matches!(
            t,
            "future" | "futures" | "expiry" | "expiration" | "expires" | "forward"
        ) {
            futures = true;
        }
        if month_token(t) || year_token(t) {
            dated = true;
        }
    }
    futures && dated
}

/// True when the description mentions a futures contract at all.
pub(crate) fn mentions_futures(norm_desc: &str) -> bool {
    tokens(norm_desc).any(|t| matches!(t, "future" | "futures"))
}

fn dated_name_regexes() -> &'static [Regex; 3] {
    static RES: OnceLock<[Regex; 3]> = OnceLock::new();
    RES.get_or_init(|| {
        [
            // NAS1226, GC0826, DOWS1226, SI0926: letter + MMYY (month 01-12, year 20-39).
            Regex::new(r"[A-Za-z](?:0[1-9]|1[0-2])(?:2[0-9]|3[0-9])$").expect("static regex"),
            // NatGas_X6, UT100_U26: separator + futures month code + 1-2 digit year.
            Regex::new(r"[_.\-][FGHJKMNQUVXZ][0-9]{1,2}$").expect("static regex"),
            // UT100U6, US30Z6: alphanumeric + futures month code + one digit.
            Regex::new(r"[A-Za-z0-9][FGHJKMNQUVXZ][0-9]$").expect("static regex"),
        ]
    })
}

/// True when the symbol name itself looks like a dated futures contract (month/year
/// code): `NAS1226`, `GC0826`, `UT100U6`, `US30Z6`, `NatGas_X6`.
pub(crate) fn name_looks_dated(name: &str) -> bool {
    dated_name_regexes().iter().any(|re| re.is_match(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_cases() {
        let cases = [
            ("S&P 500 - Cash", " s and p 500 cash "),
            ("Standard & Poor's 500", " standard and poor s 500 "),
            ("US30 CASH", " us 30 cash "),
            ("Nasdaq100", " nasdaq 100 "),
            ("XAU/USD", " xau usd "),
            ("", " "),
            ("  Gold   vs  US Dollar ", " gold vs us dollar "),
        ];
        for (input, want) in cases {
            assert_eq!(normalize_phrase(input), want, "{input}");
        }
    }

    #[test]
    fn dated_names() {
        for n in [
            "NAS1226",
            "GC0826",
            "DOWS1226",
            "SI0926",
            "UT100U6",
            "US30Z6",
            "NatGas_X6",
            "NatGas_U6",
            "NatGas_X5",
        ] {
            assert!(name_looks_dated(n), "{n}");
        }
        for n in [
            "US30",
            "US100",
            "UK100",
            "HK50",
            "JP225",
            "US2000",
            "US1000",
            "US100_Spot",
            "XAUUSD_",
            "EURUSDm",
            "NatGas_SPOT",
            "XAUUSD.pr",
            "UT100Roll",
            "US30Roll",
            "BTCUSD.lv",
            "GOLD_CASH",
            "SP500m",
        ] {
            assert!(!name_looks_dated(n), "{n}");
        }
    }

    #[test]
    fn dated_descriptions() {
        let yes = [
            "Gold Futures - December 2026",
            "Natural Gas Futures - November 2026 EX:23/10/2026",
            "US Tech 100 Index Futures - Expiry: September 2026",
            "Nasdaq 100 Futures 2026",
        ];
        let no = [
            "Gold vs US Dollar",
            "US Crude Oil - Cash",
            "Gold Futures",
            "Brent Crude Oil Cash",
        ];
        for d in yes {
            assert!(describes_dated_future(&normalize_phrase(d)), "{d}");
        }
        for d in no {
            assert!(!describes_dated_future(&normalize_phrase(d)), "{d}");
        }
    }
}
