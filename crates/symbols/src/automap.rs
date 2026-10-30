//! Infer a broker's symbol map from its full MT5 catalog.
//!
//! The approach (ported from the maintainer's earlier mapping work, then extended):
//!
//! 1. **Affix detection.** The FX majors drive it. A blanket suffix/prefix pattern for
//!    six-letter canonicals is emitted only when at least four majors agree on one affix
//!    of at most three characters on one side. When a broker lists two sets (`EURUSD` and
//!    `EURUSD_`), the set whose members are tradable and in Market Watch wins; a tie goes
//!    to the set the metals use.
//! 2. **Per-instrument candidates.** A symbol is a candidate for an instrument when its
//!    name contains the canonical name or an alias as a contiguous core with a short
//!    affix, or when its description contains one of the instrument's keywords. Dated
//!    futures, disabled symbols, stocks/ETFs and look-alikes with a long tail are never
//!    candidates. Candidates are scored (tradability, Market Watch, name fit, keyword,
//!    cash/spot/roll, mini/micro, ...) and the best becomes the mapping.
//! 3. The best candidate is written as an explicit entry only when the detected affix
//!    pattern would not already produce it.
//!
//! Everything is deterministic: the input is de-duplicated and sorted by name, ties are
//! broken by affix length and name, and no hash-ordered collection is iterated.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};

use jarvis_bridge_client::protocol::{CatalogSymbol, SymbolTradeMode};

use crate::catalog::is_currency;
use crate::text::{
    describes_dated_future, mentions_futures, name_looks_dated, normalize_phrase, tokens,
};
use crate::{
    AssetClass, AutomapResult, BrokerMap, Candidate, Instrument, InstrumentCatalog, Pattern,
};

/// Longest prefix/suffix (in characters) allowed around a recognised core.
const MAX_AFFIX: usize = 3;
/// Majors that must agree on one affix before a blanket pattern is trusted.
const FX_AFFIX_QUORUM: usize = 4;
/// Candidates kept per instrument in the result.
const MAX_CANDIDATES: usize = 8;
const FX_MAJORS: [&str; 7] = [
    "EURUSD", "GBPUSD", "USDJPY", "AUDUSD", "USDCAD", "USDCHF", "NZDUSD",
];
/// Tails that may follow a core even though they are longer than [`MAX_AFFIX`].
const TAIL_WORDS: [&str; 5] = ["cash", "spot", "roll", "rolling", "raw"];
/// The tails (and description words) that mark a spot/cash contract and earn a bonus.
const CASH_WORDS: [&str; 4] = ["cash", "spot", "roll", "rolling"];
const STOCK_PATH_TOKENS: [&str; 7] = [
    "stock", "stocks", "equity", "equities", "share", "shares", "etf",
];
const STOCK_DESC_TOKENS: [&str; 13] = [
    "inc",
    "incorporated",
    "corp",
    "corporation",
    "ltd",
    "limited",
    "plc",
    "etf",
    "holdings",
    "trust",
    "company",
    "bancorp",
    "nv",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Affix {
    prefix: String,
    suffix: String,
}

impl Affix {
    fn is_empty(&self) -> bool {
        self.prefix.is_empty() && self.suffix.is_empty()
    }
    fn len(&self) -> usize {
        self.prefix.chars().count() + self.suffix.chars().count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tradable {
    Full,
    OneWay,
    CloseOnly,
    Disabled,
}

struct Sym<'a> {
    raw: &'a CatalogSymbol,
    upper: String,
    /// Normalised description (see [`normalize_phrase`]).
    desc: String,
    tradable: Tradable,
    usable: bool,
    desc_stock: bool,
    micro: bool,
    undated_future: bool,
    cash_in_desc: bool,
}

impl<'a> Sym<'a> {
    fn new(raw: &'a CatalogSymbol) -> Self {
        let desc = normalize_phrase(&raw.description);
        let path = normalize_phrase(&raw.path);
        let tradable = match raw.trade_mode {
            SymbolTradeMode::Full => Tradable::Full,
            SymbolTradeMode::LongOnly | SymbolTradeMode::ShortOnly => Tradable::OneWay,
            SymbolTradeMode::CloseOnly => Tradable::CloseOnly,
            SymbolTradeMode::Disabled => Tradable::Disabled,
        };
        let dated = name_looks_dated(&raw.name) || describes_dated_future(&desc);
        let path_stock = tokens(&path).any(|t| STOCK_PATH_TOKENS.contains(&t));
        let name_lower = raw.name.to_ascii_lowercase();
        let micro = tokens(&desc).any(|t| matches!(t, "mini" | "micro"))
            || name_lower.contains("mini")
            || name_lower.contains("micro");
        let desc_stock = tokens(&desc).any(|t| STOCK_DESC_TOKENS.contains(&t));
        let undated_future = !dated && mentions_futures(&desc);
        let cash_in_desc = tokens(&desc).any(|t| CASH_WORDS.contains(&t));
        Sym {
            upper: raw.name.to_ascii_uppercase(),
            desc_stock,
            micro,
            undated_future,
            cash_in_desc,
            usable: tradable != Tradable::Disabled && !dated && !path_stock,
            tradable,
            desc,
            raw,
        }
    }
}

struct Inst<'a> {
    def: &'a Instrument,
    /// Upper-case cores to look for in a name: canonical first, then aliases.
    cores: Vec<(String, bool)>,
    /// `(space-padded normalised keyword, original keyword)`.
    keywords: Vec<(String, String)>,
}

impl<'a> Inst<'a> {
    fn new(def: &'a Instrument) -> Self {
        let mut cores = vec![(def.canonical.clone(), true)];
        cores.extend(def.aliases.iter().map(|a| (a.clone(), false)));
        let keywords = def
            .keywords
            .iter()
            .map(|k| (normalize_phrase(k), k.clone()))
            .filter(|(n, _)| !n.trim().is_empty())
            .collect();
        Inst {
            def,
            cores,
            keywords,
        }
    }

    /// The longest keyword found in the symbol's description.
    fn keyword_hit(&self, sym: &Sym<'_>) -> Option<&str> {
        self.keywords
            .iter()
            .filter(|(norm, _)| sym.desc.contains(norm.as_str()))
            .max_by_key(|(_, orig)| orig.len())
            .map(|(_, orig)| orig.as_str())
    }
}

#[derive(Debug, Clone)]
struct Valid {
    core: String,
    canonical: bool,
    affix: Affix,
    /// The tail is one of [`TAIL_WORDS`].
    tail: bool,
    /// The tail is a cash/spot/roll word.
    cash_tail: bool,
}

#[derive(Debug, Clone)]
enum NameMatch {
    /// The name is the core with a plausible affix.
    Valid(Valid),
    /// The core is there but glued to alphanumerics in a way that makes it a different
    /// word (`NDXX`, `US300`). A description keyword may still rescue it.
    Weak,
    /// The core is there with a long tail (`XAUUSDINDEX2`): a different product. Never a
    /// candidate.
    Blocked,
}

fn tail_kind(suffix: &str) -> (bool, bool) {
    let lower = suffix.to_ascii_lowercase();
    let word = lower.trim_start_matches(['.', '_', '-']);
    (TAIL_WORDS.contains(&word), CASH_WORDS.contains(&word))
}

/// How `core` sits inside `sym`'s name; `None` when it does not occur.
fn match_core(sym: &Sym<'_>, core: &str, canonical: bool, detected: &Affix) -> Option<NameMatch> {
    let idx = sym.upper.find(core)?;
    let name = &sym.raw.name;
    let prefix = &name[..idx];
    let suffix = &name[idx + core.len()..];
    let (tail, cash_tail) = tail_kind(suffix);
    if prefix.chars().count() > MAX_AFFIX || (!tail && suffix.chars().count() > MAX_AFFIX) {
        return Some(NameMatch::Blocked);
    }
    let mut weak = false;
    if prefix
        .chars()
        .last()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && prefix != detected.prefix
    {
        weak = true;
    }
    if !tail
        && suffix
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && suffix != detected.suffix
    {
        let letters_only = suffix.chars().all(|c| c.is_ascii_alphabetic());
        if !(core.len() >= 4 && letters_only) {
            weak = true;
        }
    }
    if weak {
        return Some(NameMatch::Weak);
    }
    Some(NameMatch::Valid(Valid {
        core: core.to_owned(),
        canonical,
        affix: Affix {
            prefix: prefix.to_owned(),
            suffix: suffix.to_owned(),
        },
        tail,
        cash_tail,
    }))
}

/// Best name match of any of the instrument's cores: the tightest valid one, else weak,
/// else blocked.
fn best_name_match(sym: &Sym<'_>, inst: &Inst<'_>, detected: &Affix) -> Option<NameMatch> {
    let mut best_valid: Option<Valid> = None;
    let mut weak = false;
    let mut blocked = false;
    for (core, canonical) in &inst.cores {
        match match_core(sym, core, *canonical, detected) {
            None => {}
            Some(NameMatch::Blocked) => blocked = true,
            Some(NameMatch::Weak) => weak = true,
            Some(NameMatch::Valid(v)) => {
                let better = match &best_valid {
                    None => true,
                    // Tightest affix, then canonical over alias, then the longer core.
                    Some(b) => {
                        (v.affix.len(), !v.canonical, std::cmp::Reverse(v.core.len()))
                            < (b.affix.len(), !b.canonical, std::cmp::Reverse(b.core.len()))
                    }
                };
                if better {
                    best_valid = Some(v);
                }
            }
        }
    }
    match (best_valid, weak, blocked) {
        (Some(v), _, _) => Some(NameMatch::Valid(v)),
        (None, _, true) => Some(NameMatch::Blocked),
        (None, true, false) => Some(NameMatch::Weak),
        (None, false, false) => None,
    }
}

fn is_pair_code(code: &str) -> bool {
    is_currency(code)
        || matches!(
            code,
            "XAU" | "XAG" | "XPT" | "XPD" | "BTC" | "ETH" | "LTC" | "XRP"
        )
}

/// The name starts with a six-letter pair that is not the instrument itself (`XAUEUR`,
/// `BTCEUR` are not gold/bitcoin against the dollar).
fn foreign_pair(upper_name: &str, canonical: &str) -> bool {
    let s = upper_name.trim_start_matches(|c: char| !c.is_ascii_alphanumeric());
    let Some(window) = s.get(..6) else {
        return false;
    };
    if !window.bytes().all(|b| b.is_ascii_uppercase()) || window == canonical {
        return false;
    }
    let (base, quote) = window.split_at(3);
    base != quote && is_pair_code(base) && is_pair_code(quote)
}

// ---------------------------------------------------------------------------------
// Affix detection
// ---------------------------------------------------------------------------------

/// Tradability/visibility points of a symbol for set selection (0..=2).
fn quality(sym: &Sym<'_>) -> u32 {
    u32::from(matches!(sym.tradable, Tradable::Full | Tradable::OneWay))
        + u32::from(sym.raw.visible)
}

/// Affix of the best-looking symbol for a metal, ignoring any detected affix.
fn metal_affix(syms: &[Sym<'_>], inst: &Inst<'_>) -> Option<Affix> {
    let none = Affix::default();
    syms.iter()
        .filter(|s| s.usable)
        .filter_map(|s| match best_name_match(s, inst, &none) {
            Some(NameMatch::Valid(v)) if !v.tail => Some((s, v)),
            _ => None,
        })
        .max_by(|(sa, va), (sb, vb)| {
            quality(sa)
                .cmp(&quality(sb))
                .then(va.canonical.cmp(&vb.canonical))
                .then(vb.affix.len().cmp(&va.affix.len()))
                .then(sb.raw.name.cmp(&sa.raw.name))
        })
        .map(|(_, v)| v.affix)
}

/// The affix the FX majors agree on (empty when none, bare, or ambiguous).
fn detect_affix(syms: &[Sym<'_>], insts: &[Inst<'_>]) -> Affix {
    let none = Affix::default();
    // affix -> (distinct majors present, quality points)
    let mut sets: BTreeMap<Affix, (usize, u32)> = BTreeMap::new();
    for inst in insts
        .iter()
        .filter(|i| i.def.class == AssetClass::Fx && FX_MAJORS.contains(&i.def.canonical.as_str()))
    {
        let mut seen: HashSet<Affix> = HashSet::new();
        for sym in syms.iter().filter(|s| s.usable) {
            let Some(NameMatch::Valid(v)) = match_core(sym, &inst.def.canonical, true, &none)
            else {
                continue;
            };
            if v.tail || (!v.affix.prefix.is_empty() && !v.affix.suffix.is_empty()) {
                continue;
            }
            if seen.insert(v.affix.clone()) {
                let entry = sets.entry(v.affix).or_insert((0, 0));
                entry.0 += 1;
                entry.1 += quality(sym);
            }
        }
    }
    let metals: Vec<Affix> = insts
        .iter()
        .filter(|i| matches!(i.def.canonical.as_str(), "XAUUSD" | "XAGUSD"))
        .filter_map(|i| metal_affix(syms, i))
        .collect();
    let best = sets
        .into_iter()
        .filter(|(_, (n, _))| *n >= FX_AFFIX_QUORUM)
        .map(|(affix, (n, points))| {
            let agree = metals.iter().filter(|m| **m == affix).count();
            (affix, n, points, agree)
        })
        .max_by(|a, b| {
            // Average quality per member (cross-multiplied), then the metals' set, then
            // size, then the shorter and alphabetically smaller affix.
            let lhs = u64::from(a.2) * b.1 as u64;
            let rhs = u64::from(b.2) * a.1 as u64;
            lhs.cmp(&rhs)
                .then(a.3.cmp(&b.3))
                .then(a.1.cmp(&b.1))
                .then(b.0.len().cmp(&a.0.len()))
                .then(b.0.cmp(&a.0))
        });
    best.map(|(affix, ..)| affix).unwrap_or_default()
}

// ---------------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------------

struct Scored {
    sym_idx: usize,
    candidate: Candidate,
    affix_len: usize,
}

fn describe_detected(detected: &Affix) -> String {
    if detected.suffix.is_empty() {
        format!("matches detected prefix `{}`", detected.prefix)
    } else {
        format!("matches detected suffix `{}`", detected.suffix)
    }
}

fn score_candidate(
    sym: &Sym<'_>,
    name: Option<&Valid>,
    keyword: Option<&str>,
    detected: &Affix,
) -> (i32, Vec<String>) {
    let mut score = 0;
    let mut reasons: Vec<String> = Vec::new();
    let mut add = |points: i32, reason: String| {
        score += points;
        reasons.push(reason);
    };
    match sym.tradable {
        Tradable::Full => add(3, "tradable".to_owned()),
        Tradable::OneWay => add(1, "one-way trading only".to_owned()),
        Tradable::CloseOnly => add(-5, "close-only".to_owned()),
        Tradable::Disabled => add(-100, "disabled".to_owned()),
    }
    if sym.raw.visible {
        add(3, "visible in Market Watch".to_owned());
    }
    if let Some(v) = name {
        let exact = v.affix.is_empty();
        match (v.canonical, exact) {
            (true, true) => add(4, "exact name".to_owned()),
            (true, false) => add(3, format!("name contains {}", v.core)),
            (false, true) => add(3, format!("exact alias {}", v.core)),
            (false, false) => add(2, format!("alias {} in name", v.core)),
        }
        // Fitting the broker's detected affix is evidence only when there is one; "no affix"
        // is merely the default and must not outweigh visibility.
        if !detected.is_empty() && v.affix == *detected {
            add(3, describe_detected(detected));
        }
        if v.cash_tail {
            add(2, "cash/spot/roll contract".to_owned());
        }
    }
    if let Some(kw) = keyword {
        add(3, format!("description matches '{kw}'"));
    }
    if name.is_none_or(|v| !v.cash_tail) && sym.cash_in_desc {
        add(2, "cash/spot/roll contract".to_owned());
    }
    if sym.micro {
        add(-3, "mini/micro variant".to_owned());
    }
    if sym.undated_future {
        add(-2, "futures contract".to_owned());
    }
    (score, reasons)
}

fn rank(a: &Scored, b: &Scored) -> Ordering {
    b.candidate
        .score
        .cmp(&a.candidate.score)
        .then(a.affix_len.cmp(&b.affix_len))
        .then(a.candidate.broker_symbol.cmp(&b.candidate.broker_symbol))
}

fn escape_replacement(s: &str) -> String {
    s.replace('$', "$$")
}

/// What the blanket pattern (if any) would produce for `canonical`.
fn pattern_produces(canonical: &str, detected: &Affix) -> String {
    if !detected.is_empty()
        && canonical.len() == 6
        && canonical.bytes().all(|b| b.is_ascii_uppercase())
    {
        format!("{}{canonical}{}", detected.prefix, detected.suffix)
    } else {
        canonical.to_owned()
    }
}

// ---------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------

pub(crate) fn automap(
    broker: &str,
    symbols: &[CatalogSymbol],
    catalog: &InstrumentCatalog,
) -> AutomapResult {
    // De-duplicate by name and fix the order so the result never depends on input order.
    let mut seen: HashSet<&str> = HashSet::new();
    let mut unique: Vec<&CatalogSymbol> = symbols
        .iter()
        .filter(|s| !s.name.trim().is_empty() && seen.insert(s.name.as_str()))
        .collect();
    unique.sort_by(|a, b| a.name.cmp(&b.name));
    let syms: Vec<Sym<'_>> = unique.into_iter().map(Sym::new).collect();
    let insts: Vec<Inst<'_>> = catalog.instruments().iter().map(Inst::new).collect();

    let detected = detect_affix(&syms, &insts);

    // Name matches per instrument (sparse), and which instruments each symbol validly is.
    let mut name_matches: Vec<HashMap<usize, NameMatch>> = Vec::with_capacity(insts.len());
    let mut valid_for: Vec<Vec<usize>> = vec![Vec::new(); syms.len()];
    for (i, inst) in insts.iter().enumerate() {
        let mut map = HashMap::new();
        for (j, sym) in syms.iter().enumerate().filter(|(_, s)| s.usable) {
            if let Some(m) = best_name_match(sym, inst, &detected) {
                if matches!(m, NameMatch::Valid(_)) {
                    valid_for[j].push(i);
                }
                map.insert(j, m);
            }
        }
        name_matches.push(map);
    }

    // Candidates for every instrument.
    let mut all: Vec<Vec<Scored>> = Vec::with_capacity(insts.len());
    for (i, inst) in insts.iter().enumerate() {
        let mut scored: Vec<Scored> = Vec::new();
        for (j, sym) in syms.iter().enumerate().filter(|(_, s)| s.usable) {
            let name = name_matches[i].get(&j);
            let keyword = inst.keyword_hit(sym);
            let valid = match name {
                Some(NameMatch::Blocked) => continue,
                Some(NameMatch::Valid(v)) => Some(v),
                Some(NameMatch::Weak) | None => None,
            };
            match valid {
                Some(v) => {
                    // A description that names a company is a stock, not the instrument.
                    if sym.desc_stock && !(v.canonical && v.affix.is_empty()) {
                        continue;
                    }
                }
                None => {
                    // Description-only: needs a keyword, must not be a stock, a different
                    // pair, or validly named for another instrument.
                    if keyword.is_none()
                        || sym.desc_stock
                        || foreign_pair(&sym.upper, &inst.def.canonical)
                        || valid_for[j].iter().any(|&k| k != i)
                    {
                        continue;
                    }
                }
            }
            let (score, reasons) = score_candidate(sym, valid, keyword, &detected);
            scored.push(Scored {
                sym_idx: j,
                affix_len: valid.map_or(MAX_AFFIX * 2 + 1, |v| v.affix.len()),
                candidate: Candidate {
                    broker_symbol: sym.raw.name.clone(),
                    description: sym.raw.description.clone(),
                    score,
                    reasons,
                },
            });
        }
        scored.sort_by(rank);
        all.push(scored);
    }

    // Choose the best per instrument; a symbol is claimed by one instrument only.
    let mut order: Vec<usize> = (0..insts.len()).collect();
    order.sort_by(|&a, &b| {
        let top = |i: usize| all[i].first().map_or(i32::MIN, |c| c.candidate.score);
        top(b)
            .cmp(&top(a))
            .then(insts[a].def.canonical.cmp(&insts[b].def.canonical))
    });
    let mut claimed: HashSet<usize> = HashSet::new();
    let mut chosen: BTreeMap<&str, &str> = BTreeMap::new();
    for i in order {
        if let Some(best) = all[i].iter().find(|c| !claimed.contains(&c.sym_idx)) {
            claimed.insert(best.sym_idx);
            chosen.insert(
                insts[i].def.canonical.as_str(),
                best.candidate.broker_symbol.as_str(),
            );
        }
    }

    // Explicit entries only where the detected pattern would not already do it.
    let mut map = BrokerMap::default();
    for (canonical, symbol) in &chosen {
        if pattern_produces(canonical, &detected) != *symbol {
            map.explicit
                .insert((*canonical).to_owned(), (*symbol).to_owned());
        }
    }
    if !detected.is_empty() {
        let replace = format!(
            "{}${{1}}{}",
            escape_replacement(&detected.prefix),
            escape_replacement(&detected.suffix)
        );
        map.patterns.push(Pattern {
            matches: "^([A-Z]{6})$".to_owned(),
            replace,
        });
    }
    let affix_note = if !detected.suffix.is_empty() {
        format!(", suffix `{}`", detected.suffix)
    } else if !detected.prefix.is_empty() {
        format!(", prefix `{}`", detected.prefix)
    } else {
        String::new()
    };
    map.note = Some(format!(
        "Auto-detected from {} symbols{affix_note}.",
        symbols.len()
    ));

    let matched = chosen.len();
    let candidates: BTreeMap<String, Vec<Candidate>> = insts
        .iter()
        .zip(all)
        .map(|(inst, scored)| {
            (
                inst.def.canonical.clone(),
                scored
                    .into_iter()
                    .take(MAX_CANDIDATES)
                    .map(|s| s.candidate)
                    .collect(),
            )
        })
        .collect();
    tracing::debug!(broker, matched, total = symbols.len(), suffix = %detected.suffix, prefix = %detected.prefix, "automap finished");

    AutomapResult {
        broker: broker.to_owned(),
        map,
        candidates,
        detected_suffix: detected.suffix,
        detected_prefix: detected.prefix,
        matched,
        total: symbols.len(),
    }
}
