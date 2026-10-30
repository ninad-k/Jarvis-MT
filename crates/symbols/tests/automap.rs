//! Auto-mapper: broker fixtures, ported behaviours and the extended heuristics.

mod common;

use std::collections::BTreeMap;

use common::{full, generated_registry, map_of, real_catalog, real_registry, run, sym};
use jarvis_bridge_client::protocol::{CatalogSymbol, SymbolTradeMode};
use jarvis_symbols::{
    AssetClass, AutomapResult, Instrument, InstrumentCatalog, Pattern, Registry, Source, automap,
};

const MAJORS: [&str; 7] = [
    "EURUSD", "GBPUSD", "USDJPY", "AUDUSD", "USDCAD", "USDCHF", "NZDUSD",
];

fn detect(symbols: &[CatalogSymbol]) -> AutomapResult {
    automap("Test Broker", symbols, &real_catalog())
}

fn majors(prefix: &str, suffix: &str, mode: SymbolTradeMode, visible: bool) -> Vec<CatalogSymbol> {
    MAJORS
        .iter()
        .map(|m| sym(&format!("{prefix}{m}{suffix}"), "", mode, visible))
        .collect()
}

fn explicit(result: &AutomapResult) -> BTreeMap<&str, &str> {
    result
        .map
        .explicit
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect()
}

fn pattern_of(result: &AutomapResult) -> Vec<(&str, &str)> {
    result
        .map
        .patterns
        .iter()
        .map(|p| (p.matches.as_str(), p.replace.as_str()))
        .collect()
}

fn resolved(result: &AutomapResult, canonical: &str) -> String {
    generated_registry(result)
        .resolve(&result.broker, canonical, &real_catalog())
        .broker_symbol
}

fn candidate_names(result: &AutomapResult, canonical: &str) -> Vec<String> {
    result.candidates[canonical]
        .iter()
        .map(|c| c.broker_symbol.clone())
        .collect()
}

// ---------------------------------------------------------------------------------
// Broker fixtures
// ---------------------------------------------------------------------------------

struct Expect {
    fixture: &'static str,
    suffix: &'static str,
    prefix: &'static str,
    pattern: Option<&'static str>,
    explicit: &'static [(&'static str, &'static str)],
    matched: usize,
    total: usize,
}

const EXPECTATIONS: &[Expect] = &[
    Expect {
        fixture: "cfi",
        suffix: "_",
        prefix: "",
        pattern: Some("${1}_"),
        explicit: &[
            ("BTCUSD", "BTCUSD"),
            ("ETHUSD", "ETHUSD"),
            ("GER40", "GER30_SPOT"),
            ("NATGAS", "NatGas_SPOT"),
            ("US100", "US100_Spot"),
            ("US30", "US30_SPOT"),
            ("US500", "US500_SPOT"),
            ("USDCNH", "USDCNH"),
        ],
        matched: 20,
        total: 50,
    },
    Expect {
        fixture: "equiti",
        suffix: ".pr",
        prefix: "",
        pattern: Some("${1}.pr"),
        explicit: &[
            ("BRENT", "UKOILRoll"),
            ("BTCUSD", "BTCUSD.lv"),
            ("US100", "UT100Roll"),
            ("US30", "US30Roll"),
        ],
        matched: 11,
        total: 18,
    },
    Expect {
        fixture: "kira",
        suffix: "",
        prefix: "",
        pattern: None,
        explicit: &[
            ("BRENT", "BRENTcash"),
            ("US100", "NDXcash"),
            ("US30", "US30cash"),
            ("XAUUSD", "GOLD_CASH"),
        ],
        matched: 15,
        total: 16,
    },
    Expect {
        fixture: "raw_trading_marketwatch",
        suffix: "",
        prefix: "",
        pattern: None,
        explicit: &[],
        matched: 11,
        total: 12,
    },
    Expect {
        fixture: "ic_markets_like",
        suffix: "",
        prefix: "",
        pattern: None,
        explicit: &[
            ("BRENT", "XBRUSD"),
            ("FRA40", "F40"),
            ("GER40", "DE40"),
            ("NATGAS", "XNGUSD"),
            ("US100", "USTEC"),
            ("WTI", "XTIUSD"),
        ],
        matched: 32,
        total: 35,
    },
    Expect {
        fixture: "exness_like",
        suffix: "m",
        prefix: "",
        pattern: Some("${1}m"),
        explicit: &[
            ("BRENT", "UKOILm"),
            ("GER40", "DE30m"),
            ("JP225", "JP225m"),
            ("NATGAS", "XNGUSDm"),
            ("UK100", "UK100m"),
            ("US100", "USTECm"),
            ("US30", "US30m"),
            ("US500", "US500m"),
            ("WTI", "USOILm"),
        ],
        matched: 22,
        total: 23,
    },
    Expect {
        fixture: "metaquotes_demo_like",
        suffix: "",
        prefix: "",
        pattern: None,
        explicit: &[
            ("BRENT", "UKOIL"),
            ("US100", "NAS100"),
            ("US500", "SPX500"),
            ("WTI", "USOIL"),
        ],
        matched: 19,
        total: 21,
    },
    Expect {
        fixture: "hash_prefix",
        suffix: "",
        prefix: "#",
        pattern: Some("#${1}"),
        explicit: &[("US100", "#USTEC"), ("US30", "#US30")],
        matched: 10,
        total: 11,
    },
    Expect {
        fixture: "adversarial",
        suffix: "",
        prefix: "",
        pattern: None,
        explicit: &[
            ("BRENT", "UKOIL"),
            ("EURUSD", "EURUSD.a"),
            ("US100", "USTEC"),
            ("US30", "US30.cash"),
            ("WTI", "USOIL.a"),
            ("XAGUSD", "XAGUSD.x"),
            ("XPTUSD", "XPTUSD.s"),
        ],
        matched: 8,
        total: 22,
    },
];

#[test]
fn every_fixture_produces_the_expected_map() {
    for e in EXPECTATIONS {
        let (fx, result) = run(e.fixture);
        let ctx = e.fixture;
        assert_eq!(result.broker, fx.broker, "{ctx}");
        assert_eq!(result.detected_suffix, e.suffix, "{ctx}: suffix");
        assert_eq!(result.detected_prefix, e.prefix, "{ctx}: prefix");
        match e.pattern {
            Some(replace) => assert_eq!(
                pattern_of(&result),
                vec![("^([A-Z]{6})$", replace)],
                "{ctx}: pattern"
            ),
            None => assert!(
                result.map.patterns.is_empty(),
                "{ctx}: unexpected {:?}",
                result.map.patterns
            ),
        }
        let want: BTreeMap<&str, &str> = e.explicit.iter().copied().collect();
        assert_eq!(explicit(&result), want, "{ctx}: explicit");
        assert_eq!(result.matched, e.matched, "{ctx}: matched");
        assert_eq!(result.total, e.total, "{ctx}: total");
        assert_eq!(result.total, fx.symbols.len(), "{ctx}: total vs fixture");
        assert!(
            result
                .map
                .note
                .as_deref()
                .is_some_and(|n| n.contains(&e.total.to_string())),
            "{ctx}: note"
        );
    }
}

#[test]
fn fixtures_resolve_end_to_end_through_a_registry() {
    // (fixture, canonical, expected broker symbol)
    let cases: &[(&str, &str, &str)] = &[
        ("cfi", "XAUUSD", "XAUUSD_"),
        ("cfi", "XAGUSD", "XAGUSD_"),
        ("cfi", "EURUSD", "EURUSD_"),
        ("cfi", "GBPUSD", "GBPUSD_"),
        ("cfi", "USDJPY", "USDJPY_"),
        ("cfi", "NZDUSD", "NZDUSD_"),
        ("cfi", "AUDJPY", "AUDJPY_"),
        ("cfi", "US100", "US100_Spot"),
        ("cfi", "US30", "US30_SPOT"),
        ("cfi", "US500", "US500_SPOT"),
        ("cfi", "GER40", "GER30_SPOT"),
        ("cfi", "BTCUSD", "BTCUSD"),
        ("cfi", "WTI", "WTI"),
        ("cfi", "BRENT", "BRENT"),
        ("cfi", "NATGAS", "NatGas_SPOT"),
        ("equiti", "EURUSD", "EURUSD.pr"),
        ("equiti", "EURGBP", "EURGBP.pr"),
        ("equiti", "XAUUSD", "XAUUSD.pr"),
        ("equiti", "US100", "UT100Roll"),
        ("equiti", "US30", "US30Roll"),
        ("equiti", "BRENT", "UKOILRoll"),
        ("kira", "XAUUSD", "GOLD_CASH"),
        ("kira", "XAGUSD", "XAGUSD"),
        ("kira", "EURUSD", "EURUSD"),
        ("kira", "US30", "US30cash"),
        ("kira", "US100", "NDXcash"),
        ("kira", "BRENT", "BRENTcash"),
        ("raw_trading_marketwatch", "XAUUSD", "XAUUSD"),
        ("raw_trading_marketwatch", "HK50", "HK50"),
        ("raw_trading_marketwatch", "US500", "US500"),
        ("ic_markets_like", "US100", "USTEC"),
        ("ic_markets_like", "GER40", "DE40"),
        ("ic_markets_like", "FRA40", "F40"),
        ("ic_markets_like", "WTI", "XTIUSD"),
        ("ic_markets_like", "BRENT", "XBRUSD"),
        ("ic_markets_like", "NATGAS", "XNGUSD"),
        ("ic_markets_like", "JP225", "JP225"),
        ("ic_markets_like", "UK100", "UK100"),
        ("ic_markets_like", "US500", "US500"),
        ("ic_markets_like", "BTCUSD", "BTCUSD"),
        ("exness_like", "XAUUSD", "XAUUSDm"),
        ("exness_like", "EURUSD", "EURUSDm"),
        ("exness_like", "US100", "USTECm"),
        ("exness_like", "US30", "US30m"),
        ("exness_like", "GER40", "DE30m"),
        ("exness_like", "WTI", "USOILm"),
        ("exness_like", "BTCUSD", "BTCUSDm"),
        ("metaquotes_demo_like", "US100", "NAS100"),
        ("metaquotes_demo_like", "US500", "SPX500"),
        ("metaquotes_demo_like", "US30", "US30"),
        ("metaquotes_demo_like", "GER40", "GER40"),
        ("metaquotes_demo_like", "WTI", "USOIL"),
        ("hash_prefix", "EURUSD", "#EURUSD"),
        ("hash_prefix", "XAUUSD", "#XAUUSD"),
        ("hash_prefix", "US30", "#US30"),
        ("hash_prefix", "US100", "#USTEC"),
        ("adversarial", "XAUUSD", "XAUUSD"),
        ("adversarial", "EURUSD", "EURUSD.a"),
        ("adversarial", "WTI", "USOIL.a"),
    ];
    let mut cache: BTreeMap<&str, AutomapResult> = BTreeMap::new();
    for (fixture, canonical, want) in cases {
        let result = cache.entry(fixture).or_insert_with(|| run(fixture).1);
        let reg = generated_registry(result);
        let cat = real_catalog();
        let got = reg.resolve(&result.broker, canonical, &cat);
        assert_eq!(got.broker_symbol, *want, "{fixture}: {canonical}");
        assert_eq!(
            reg.reverse(&result.broker, want, &cat).as_deref(),
            Some(*canonical),
            "{fixture}: reverse({want})"
        );
    }
}

#[test]
fn generated_maps_agree_with_the_curated_file_for_verified_brokers() {
    let curated = real_registry();
    let cat = real_catalog();
    for fixture in ["cfi", "equiti", "kira", "raw_trading_marketwatch"] {
        let (fx, result) = run(fixture);
        let generated = generated_registry(&result);
        let layer = &curated.curated[&fx.broker];
        for canonical in layer.explicit.keys() {
            let want = curated.resolve(&fx.broker, canonical, &cat).broker_symbol;
            let got = generated.resolve(&fx.broker, canonical, &cat).broker_symbol;
            assert_eq!(got, want, "{fixture}: {canonical}");
        }
    }
}

#[test]
fn candidate_invariants_hold_on_every_fixture() {
    let cat = real_catalog();
    for e in EXPECTATIONS {
        let (fx, result) = run(e.fixture);
        let names: BTreeMap<&str, &CatalogSymbol> =
            fx.symbols.iter().map(|s| (s.name.as_str(), s)).collect();
        assert_eq!(
            result.candidates.len(),
            cat.instruments().len(),
            "{}: one entry per instrument",
            e.fixture
        );
        for inst in cat.instruments() {
            let list = &result.candidates[&inst.canonical];
            assert!(list.len() <= 8, "{}: {}", e.fixture, inst.canonical);
            assert!(
                list.windows(2).all(|w| w[0].score >= w[1].score),
                "{}: {} ranked best first",
                e.fixture,
                inst.canonical
            );
            for c in list {
                let s = names.get(c.broker_symbol.as_str()).unwrap_or_else(|| {
                    panic!("{}: unknown candidate {}", e.fixture, c.broker_symbol)
                });
                assert_ne!(
                    s.trade_mode,
                    SymbolTradeMode::Disabled,
                    "{}: disabled {}",
                    e.fixture,
                    c.broker_symbol
                );
                assert_eq!(c.description, s.description);
                assert!(!c.reasons.is_empty());
            }
        }
        // Every generated entry points at a symbol that exists, and round-trips.
        let reg = generated_registry(&result);
        for (canonical, symbol) in &result.map.explicit {
            assert!(
                names.contains_key(symbol.as_str()),
                "{}: {canonical} -> {symbol}",
                e.fixture
            );
            assert_eq!(
                reg.reverse(&result.broker, symbol, &cat).as_deref(),
                Some(canonical.as_str()),
                "{}: {symbol}",
                e.fixture
            );
        }
    }
}

#[test]
fn dated_futures_and_stocks_never_become_candidates_in_fixtures() {
    let dated = [
        "DOWS1226",
        "GC1226",
        "NAS0926",
        "NAS1226",
        "NatGas_U6",
        "NatGas_X5",
        "NatGas_X6",
        "SI0926",
        "SI1226",
        "SP0926",
        "SP1226",
        "UT100U6",
        "US30Z6",
        "GC0826",
        "NatGas_X6",
    ];
    let stocks = [
        "AAPL.OQ", "AMD", "AXP.N", "INTC", "MSFT", "NVDA", "NVDA.OQ", "VRTX.OQ", "AAPL", "TSLA",
        "AAPLm", "#AAPL", "GOLD.N", "NDXX",
    ];
    for e in EXPECTATIONS {
        let (_, result) = run(e.fixture);
        for (canonical, list) in &result.candidates {
            for c in list {
                assert!(
                    !dated.contains(&c.broker_symbol.as_str()),
                    "{}: dated {} for {canonical}",
                    e.fixture,
                    c.broker_symbol
                );
                assert!(
                    !stocks.contains(&c.broker_symbol.as_str()),
                    "{}: stock {} for {canonical}",
                    e.fixture,
                    c.broker_symbol
                );
            }
        }
    }
}

#[test]
fn cfi_dual_set_prefers_the_market_watch_set() {
    let (_, result) = run("cfi");
    // Both EURUSD and EURUSD_ are listed and tradable; only the underscore set is in Market Watch.
    assert_eq!(
        candidate_names(&result, "EURUSD"),
        vec!["EURUSD_", "EURUSD"]
    );
    let top = &result.candidates["EURUSD"][0];
    assert!(
        top.reasons
            .iter()
            .any(|r| r.contains("detected suffix `_`")),
        "{:?}",
        top.reasons
    );
    assert!(top.reasons.iter().any(|r| r.contains("Market Watch")));
    assert!(top.score > result.candidates["EURUSD"][1].score);
    // Dated futures and the NDX/Nasdaq stocks are not even mentioned.
    assert_eq!(candidate_names(&result, "US100"), vec!["US100_Spot"]);
    assert_eq!(candidate_names(&result, "XAUUSD"), vec!["XAUUSD_"]);
    assert_eq!(candidate_names(&result, "NATGAS"), vec!["NatGas_SPOT"]);
    assert_eq!(
        candidate_names(&result, "US500"),
        vec!["US500_SPOT", "SP500m"]
    );
    assert_eq!(candidate_names(&result, "US30"), vec!["US30_SPOT"]);
    assert!(candidate_names(&result, "HK50").is_empty());
    assert!(candidate_names(&result, "US2000").is_empty());
}

#[test]
fn equiti_prefers_rolling_over_dated_contracts() {
    let (_, result) = run("equiti");
    assert_eq!(candidate_names(&result, "US100"), vec!["UT100Roll"]);
    assert_eq!(candidate_names(&result, "US30"), vec!["US30Roll"]);
    assert_eq!(candidate_names(&result, "BRENT"), vec!["UKOILRoll"]);
    assert_eq!(candidate_names(&result, "BTCUSD"), vec!["BTCUSD.lv"]);
    assert!(
        result.candidates["US100"][0]
            .reasons
            .iter()
            .any(|r| r.contains("cash/spot/roll"))
    );
}

#[test]
fn market_watch_indices_are_not_confused() {
    let (_, result) = run("raw_trading_marketwatch");
    // `CHINAH` ("Hong Kong China H-shares Index") is not HK50.
    assert_eq!(candidate_names(&result, "HK50"), vec!["HK50"]);
    assert_eq!(candidate_names(&result, "US500"), vec!["US500"]);
    let (_, result) = run("ic_markets_like");
    assert_eq!(candidate_names(&result, "HK50"), vec!["HK50"]);
    assert_eq!(candidate_names(&result, "EU50"), vec!["EU50"]);
    assert_eq!(candidate_names(&result, "FRA40"), vec!["F40"]);
    assert_eq!(candidate_names(&result, "US2000"), vec!["US2000"]);
    let top = &result.candidates["FRA40"][0];
    assert!(
        top.reasons
            .iter()
            .any(|r| r.contains("description matches")),
        "keyword-only candidate: {:?}",
        top.reasons
    );
}

// ---------------------------------------------------------------------------------
// Ported behaviours (affix detection)
// ---------------------------------------------------------------------------------

#[test]
fn underscore_suffix_broker_emits_pattern_and_resolves() {
    let mut symbols = majors("", "_", SymbolTradeMode::Full, true);
    symbols.extend([
        full("XAUUSD_", "Gold"),
        full("XAGUSD_", "Silver"),
        full("US30_", "Wall Street 30"),
    ]);
    let r = detect(&symbols);
    assert_eq!(r.detected_suffix, "_");
    assert_eq!(pattern_of(&r), vec![("^([A-Z]{6})$", "${1}_")]);
    for (name, want) in [
        ("GOLD", "XAUUSD_"),
        ("XAUUSD", "XAUUSD_"),
        ("EURUSD", "EURUSD_"),
        ("silver", "XAGUSD_"),
        ("US30", "US30_"),
    ] {
        assert_eq!(resolved(&r, name), want, "{name}");
    }
    assert!(
        !r.map.explicit.contains_key("XAUUSD"),
        "covered by the pattern"
    );
}

#[test]
fn lowercase_m_suffix() {
    let mut symbols = majors("", "m", SymbolTradeMode::Full, true);
    symbols.push(full("XAUUSDm", "Gold"));
    let r = detect(&symbols);
    assert_eq!(r.detected_suffix, "m");
    assert_eq!(pattern_of(&r), vec![("^([A-Z]{6})$", "${1}m")]);
    assert_eq!(resolved(&r, "GOLD"), "XAUUSDm");
    assert!(
        !r.map.explicit.contains_key("XAGUSD"),
        "no silver in the catalog"
    );
    assert!(r.candidates["XAGUSD"].is_empty());
    assert_eq!(r.matched, 8);
}

#[test]
fn already_canonical_broker_has_no_pattern() {
    let mut symbols = majors("", "", SymbolTradeMode::Full, true);
    symbols.extend([full("XAUUSD", "Gold"), full("XAGUSD", "Silver")]);
    let r = detect(&symbols);
    assert!(r.map.patterns.is_empty());
    assert!(r.map.explicit.is_empty());
    assert_eq!(resolved(&r, "GOLD"), "XAUUSD");
    assert_eq!(resolved(&r, "silver"), "XAGUSD");
    assert_eq!(r.matched, 9);
    assert_eq!(
        (r.detected_suffix.as_str(), r.detected_prefix.as_str()),
        ("", "")
    );
}

#[test]
fn broker_without_fx_majors_gets_explicit_entries_only() {
    let symbols = [
        full("XAUUSD.r", "Gold"),
        full("XAGUSD.r", "Silver"),
        full("US30.r", "Wall Street 30"),
    ];
    let r = detect(&symbols);
    assert!(r.map.patterns.is_empty());
    assert_eq!(
        explicit(&r),
        BTreeMap::from([
            ("US30", "US30.r"),
            ("XAGUSD", "XAGUSD.r"),
            ("XAUUSD", "XAUUSD.r")
        ])
    );
    assert_eq!(resolved(&r, "GOLD"), "XAUUSD.r");
}

#[test]
fn mixed_affixes_below_quorum_emit_no_pattern() {
    let symbols = [
        full("EURUSD_", ""),
        full("GBPUSD_", ""),
        full("USDJPY_", ""),
        full("AUDUSD", ""),
        full("USDCAD", ""),
        full("USDCHF", ""),
        full("NZDUSD", ""),
    ];
    let r = detect(&symbols);
    assert!(r.map.patterns.is_empty());
    assert_eq!(
        explicit(&r),
        BTreeMap::from([
            ("EURUSD", "EURUSD_"),
            ("GBPUSD", "GBPUSD_"),
            ("USDJPY", "USDJPY_")
        ])
    );
}

#[test]
fn empty_catalog_returns_zeros() {
    let r = detect(&[]);
    assert_eq!((r.matched, r.total), (0, 0));
    assert!(r.map.explicit.is_empty() && r.map.patterns.is_empty());
    assert_eq!(r.candidates.len(), real_catalog().instruments().len());
    assert!(r.candidates.values().all(Vec::is_empty));
    assert_eq!(r.broker, "Test Broker");
}

#[test]
fn prefix_broker_emits_prefix_pattern() {
    let r = detect(&majors(".", "", SymbolTradeMode::Full, true));
    assert_eq!(r.detected_prefix, ".");
    assert_eq!(r.detected_suffix, "");
    assert_eq!(pattern_of(&r), vec![("^([A-Z]{6})$", ".${1}")]);
    assert_eq!(resolved(&r, "EURUSD"), ".EURUSD");
}

#[test]
fn dollar_in_an_affix_is_escaped_in_the_replacement() {
    let r = detect(&majors("", "$", SymbolTradeMode::Full, true));
    assert_eq!(r.detected_suffix, "$");
    assert_eq!(pattern_of(&r), vec![("^([A-Z]{6})$", "${1}$$")]);
    assert_eq!(resolved(&r, "EURUSD"), "EURUSD$");
}

#[test]
fn quorum_needs_four_majors() {
    for (count, expect_pattern) in [
        (1usize, false),
        (2, false),
        (3, false),
        (4, true),
        (5, true),
        (7, true),
    ] {
        let symbols: Vec<CatalogSymbol> = MAJORS
            .iter()
            .take(count)
            .map(|m| full(&format!("{m}.pr"), ""))
            .collect();
        let r = detect(&symbols);
        assert_eq!(!r.map.patterns.is_empty(), expect_pattern, "{count} majors");
        assert_eq!(
            r.detected_suffix,
            if expect_pattern { ".pr" } else { "" },
            "{count} majors"
        );
        // Either way every present major resolves to its broker symbol.
        for m in MAJORS.iter().take(count) {
            assert_eq!(resolved(&r, m), format!("{m}.pr"), "{count} majors: {m}");
        }
    }
}

#[test]
fn affix_limits_two_sided_too_long_and_tails() {
    // Two-sided affix: no blanket pattern, but each major maps explicitly.
    let two_sided = detect(&majors(".", ".r", SymbolTradeMode::Full, true));
    assert!(two_sided.map.patterns.is_empty());
    assert_eq!(two_sided.map.explicit.len(), 7);
    assert_eq!(resolved(&two_sided, "EURUSD"), ".EURUSD.r");

    // Four-character suffix: not a plausible affix at all, nothing is mapped.
    let long = detect(&majors("", ".prime", SymbolTradeMode::Full, true));
    assert!(long.map.patterns.is_empty() && long.map.explicit.is_empty());
    assert_eq!(long.matched, 0);

    // `.raw` is a known tail: mapped, but never used for blanket detection.
    let raw = detect(&majors("", ".raw", SymbolTradeMode::Full, true));
    assert!(raw.map.patterns.is_empty());
    assert_eq!(raw.map.explicit.len(), 7);
    assert_eq!(resolved(&raw, "EURUSD"), "EURUSD.raw");

    // Three characters on each side is the limit.
    let limit = detect(&majors("#*~", ".-_", SymbolTradeMode::Full, true));
    assert_eq!(limit.map.explicit.len(), 7);
    assert_eq!(resolved(&limit, "EURUSD"), "#*~EURUSD.-_");
    let over = detect(&majors("#*~!", "", SymbolTradeMode::Full, true));
    assert_eq!(over.matched, 0);
    // Letters glued in front of a core are a different word, not an affix.
    let glued = detect(&majors("abc", "", SymbolTradeMode::Full, true));
    assert_eq!(glued.matched, 0);
}

// ---------------------------------------------------------------------------------
// Dual sets
// ---------------------------------------------------------------------------------

#[test]
fn dual_set_selection_table() {
    use SymbolTradeMode::{CloseOnly, Disabled, Full};
    struct Case {
        label: &'static str,
        bare: (SymbolTradeMode, bool),
        affixed: (SymbolTradeMode, bool),
        metal: Option<&'static str>,
        want_suffix: &'static str,
    }
    let cases = [
        Case {
            label: "bare not in Market Watch",
            bare: (Full, false),
            affixed: (Full, true),
            metal: None,
            want_suffix: "_",
        },
        Case {
            label: "affixed not in Market Watch",
            bare: (Full, true),
            affixed: (Full, false),
            metal: None,
            want_suffix: "",
        },
        Case {
            label: "bare close-only",
            bare: (CloseOnly, true),
            affixed: (Full, true),
            metal: None,
            want_suffix: "_",
        },
        Case {
            label: "affixed close-only",
            bare: (Full, true),
            affixed: (CloseOnly, true),
            metal: None,
            want_suffix: "",
        },
        Case {
            label: "bare disabled",
            bare: (Disabled, true),
            affixed: (Full, true),
            metal: None,
            want_suffix: "_",
        },
        Case {
            label: "tie, metals use affixed",
            bare: (Full, true),
            affixed: (Full, true),
            metal: Some("XAUUSD_"),
            want_suffix: "_",
        },
        Case {
            label: "tie, metals use bare",
            bare: (Full, true),
            affixed: (Full, true),
            metal: Some("XAUUSD"),
            want_suffix: "",
        },
        Case {
            label: "tie, no metals: shorter affix",
            bare: (Full, true),
            affixed: (Full, true),
            metal: None,
            want_suffix: "",
        },
        Case {
            label: "visibility beats the metals",
            bare: (Full, true),
            affixed: (Full, false),
            metal: Some("XAUUSD_"),
            want_suffix: "",
        },
        Case {
            label: "tradability beats the metals",
            bare: (CloseOnly, true),
            affixed: (Full, true),
            metal: Some("XAUUSD"),
            want_suffix: "_",
        },
        Case {
            label: "both hidden, bare close-only",
            bare: (CloseOnly, false),
            affixed: (Full, false),
            metal: None,
            want_suffix: "_",
        },
    ];
    for c in cases {
        let mut symbols = majors("", "", c.bare.0, c.bare.1);
        symbols.extend(majors("", "_", c.affixed.0, c.affixed.1));
        if let Some(metal) = c.metal {
            symbols.push(sym(metal, "Gold", Full, true));
        }
        let r = detect(&symbols);
        assert_eq!(r.detected_suffix, c.want_suffix, "{}", c.label);
        // The per-instrument choice agrees with the detected set.
        let expected = format!("EURUSD{}", c.want_suffix);
        assert_eq!(resolved(&r, "EURUSD"), expected, "{}: EURUSD", c.label);
        assert_eq!(
            resolved(&r, "USDJPY"),
            format!("USDJPY{}", c.want_suffix),
            "{}: USDJPY",
            c.label
        );
        let listed = if c.bare.0 == Disabled { 1 } else { 2 };
        assert_eq!(r.candidates["EURUSD"].len(), listed, "{}", c.label);
    }
}

#[test]
fn dual_set_with_only_one_eligible_set_ignores_the_small_one() {
    // Three visible `_` majors never form a quorum; the four-strong bare set decides.
    let mut symbols: Vec<CatalogSymbol> = MAJORS
        .iter()
        .take(4)
        .map(|m| sym(m, "", SymbolTradeMode::Full, false))
        .collect();
    symbols.extend(MAJORS.iter().take(3).map(|m| full(&format!("{m}_"), "")));
    let r = detect(&symbols);
    assert_eq!(r.detected_suffix, "");
    assert!(r.map.patterns.is_empty());
    // The explicit entries still pick the better symbol per instrument.
    assert_eq!(resolved(&r, "EURUSD"), "EURUSD_");
    assert_eq!(resolved(&r, "AUDUSD"), "AUDUSD");
}

// ---------------------------------------------------------------------------------
// Candidate classification (one symbol at a time)
// ---------------------------------------------------------------------------------

struct One {
    name: &'static str,
    desc: &'static str,
    path: &'static str,
    mode: SymbolTradeMode,
    visible: bool,
    canonical: &'static str,
    candidate: bool,
}

const fn one(
    name: &'static str,
    desc: &'static str,
    canonical: &'static str,
    candidate: bool,
) -> One {
    One {
        name,
        desc,
        path: "",
        mode: SymbolTradeMode::Full,
        visible: true,
        canonical,
        candidate,
    }
}

#[test]
fn single_symbol_classification_table() {
    use SymbolTradeMode::{CloseOnly, Disabled, LongOnly, ShortOnly};
    let cases = vec![
        // Plain and affixed names.
        one("XAUUSD", "Gold vs US Dollar", "XAUUSD", true),
        one("XAUUSDm", "Gold vs US Dollar", "XAUUSD", true),
        one("XAUUSD.pr", "Gold vs US Dollar", "XAUUSD", true),
        one("XAUUSD_", "Gold", "XAUUSD", true),
        one("#XAUUSD", "Gold", "XAUUSD", true),
        one("XAUUSD.a", "", "XAUUSD", true),
        one("xauusd", "", "XAUUSD", true),
        one("GOLD_CASH", "Gold vs US Dollar", "XAUUSD", true),
        one("GOLD", "Gold", "XAUUSD", true),
        one("EURUSD.raw", "Euro vs US Dollar", "EURUSD", true),
        one("USDJPY", "US Dollar vs Japense Yen", "USDJPY", true),
        one("SP500m", "Standard & Poor's 500", "US500", true),
        one("US500.cash", "US 500 Cash", "US500", true),
        one("US30cash", "US30 CASH", "US30", true),
        one("NDXcash", "US NASDAQ 100", "US100", true),
        one("UT100Roll", "US Tech 100 Cash Index", "US100", true),
        one("US30Roll", "US Wall Street 30 Cash Index", "US30", true),
        one("NatGas_SPOT", "Natural Gas - Cash", "NATGAS", true),
        one("BRENT", "UK Crude Oil - Cash", "BRENT", true),
        one("CL", "Light Sweet Crude Oil", "WTI", true),
        one("NG", "Natural Gas", "NATGAS", true),
        // Keyword-only (the name does not contain any core).
        one("EUR/USD", "Euro vs US Dollar", "EURUSD", true),
        one("F40", "France 40 Index", "FRA40", true),
        one("OILSPOT", "WTI Crude Oil", "WTI", true),
        one("NDXX", "Nasdaq 100 Index", "US100", true),
        // Dated futures are never candidates, whatever the description says.
        one(
            "NAS1226",
            "Nasdaq 100 Futures - December 2026",
            "US100",
            false,
        ),
        one("NAS1226", "Nasdaq 100 Index", "US100", false),
        one("GC0826", "Gold Futures - August 2026", "XAUUSD", false),
        one("GC0826", "Gold", "XAUUSD", false),
        one(
            "SI0926",
            "Silver Futures - September 2026 EX:26/08/2026",
            "XAGUSD",
            false,
        ),
        one(
            "DOWS1226",
            "Dow Jones 30 Futures - December 2026",
            "US30",
            false,
        ),
        one(
            "NatGas_X6",
            "Natural Gas Futures - November 2026 EX:23/10/2026",
            "NATGAS",
            false,
        ),
        one("NatGas_U6", "Natural Gas", "NATGAS", false),
        one(
            "US30Z6",
            "US Wall Street 30 Index Futures - Expiry: December 2026",
            "US30",
            false,
        ),
        one("UT100U6", "US Tech 100 Index", "US100", false),
        one(
            "OILSPOT",
            "WTI Crude Oil Futures - December 2026",
            "WTI",
            false,
        ),
        one("XAUUSD", "Gold Futures - December 2026", "XAUUSD", false),
        // Look-alikes.
        one("XAUUSDINDEX2", "XAUUSD Index Basket 2", "XAUUSD", false),
        one("XAUUSDINDEX2", "Gold Index Basket", "XAUUSD", false),
        one("XAUUSD.micro", "Gold Micro", "XAUUSD", false),
        one("EURUSD.prime", "Euro vs US Dollar", "EURUSD", false),
        one("US300", "US 300 Mid Cap Index", "US30", false),
        one("US3000", "US 3000 Index", "US30", false),
        one("AUS30", "Australia Index", "US30", false),
        one("NDXX", "Index Fund Z", "US100", false),
        one("NDXm", "Something Else", "US100", false),
        one("XAUEUR", "Gold vs Euro", "XAUUSD", false),
        one("BTCEUR", "Bitcoin vs Euro", "BTCUSD", false),
        one("CHINAH", "Hong Kong China H-shares Index", "HK50", false),
        one("CLM6", "Light Sweet Crude Oil", "WTI", false),
        one("ETHE", "Grayscale Ethereum Trust", "ETHUSD", false),
        // Stocks and ETFs.
        One {
            path: "Equities\\US.Equities\\US.Materials\\GOLD.N",
            ..one("GOLD.N", "Barrick Gold Corporation", "XAUUSD", false)
        },
        One {
            path: "Stocks\\US\\NDXX",
            ..one("NDXX", "NDXX Holdings Inc", "US100", false)
        },
        one("GOLD.N", "Barrick Gold Corporation", "XAUUSD", false),
        one("GLD", "SPDR Gold Trust ETF", "XAUUSD", false),
        One {
            path: "Shares\\US\\BTC",
            ..one("BTC", "Bitcoin", "BTCUSD", false)
        },
        One {
            path: "US ETFs\\IBIT",
            ..one("BTC.etf", "Bitcoin ETF", "BTCUSD", false)
        },
        // Trade modes.
        One {
            mode: Disabled,
            ..one("XAUUSD", "Gold vs US Dollar", "XAUUSD", false)
        },
        One {
            mode: CloseOnly,
            ..one("XAUUSD", "Gold vs US Dollar", "XAUUSD", true)
        },
        One {
            mode: LongOnly,
            ..one("XAUUSD", "Gold vs US Dollar", "XAUUSD", true)
        },
        One {
            mode: ShortOnly,
            ..one("XAUUSD", "Gold vs US Dollar", "XAUUSD", true)
        },
        One {
            visible: false,
            ..one("XAUUSD", "Gold vs US Dollar", "XAUUSD", true)
        },
    ];
    for c in cases {
        let mut s = sym(c.name, c.desc, c.mode, c.visible);
        s.path = c.path.to_owned();
        let r = detect(&[s]);
        let found = r.candidates[c.canonical]
            .iter()
            .any(|x| x.broker_symbol == c.name);
        assert_eq!(
            found, c.candidate,
            "{} ({:?}, {:?}) for {}: {:?}",
            c.name, c.desc, c.mode, c.canonical, r.candidates[c.canonical]
        );
        // A symbol claimed by exactly one instrument is never a candidate of another one
        // by accident: at most the expected canonical lists it.
        let claimers: Vec<&String> = r
            .candidates
            .iter()
            .filter(|(_, v)| v.iter().any(|x| x.broker_symbol == c.name))
            .map(|(k, _)| k)
            .collect();
        assert!(claimers.len() <= 1, "{} claimed by {claimers:?}", c.name);
    }
}

// ---------------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------------

#[test]
fn close_only_loses_to_a_tradable_alternative_and_disabled_is_excluded() {
    let (_, r) = run("adversarial");
    assert_eq!(candidate_names(&r, "XAGUSD"), vec!["XAGUSD.x", "XAGUSD"]);
    let close_only = &r.candidates["XAGUSD"][1];
    assert!(close_only.reasons.iter().any(|x| x == "close-only"));
    // The disabled bare EURUSD is not mentioned; its sibling is mapped.
    assert_eq!(candidate_names(&r, "EURUSD"), vec!["EURUSD.a"]);
    // The only crude candidate that can trade wins over the disabled one.
    assert_eq!(candidate_names(&r, "WTI"), vec!["USOIL.a"]);
    // A close-only symbol is still better than nothing (positions can be closed).
    assert_eq!(candidate_names(&r, "BRENT"), vec!["UKOIL"]);
    assert_eq!(resolved(&r, "BRENT"), "UKOIL");
}

#[test]
fn micro_variants_rank_below_the_standard_contract() {
    let (_, r) = run("adversarial");
    assert_eq!(candidate_names(&r, "XPTUSD"), vec!["XPTUSD.s", "XPTUSD.m"]);
    assert!(
        r.candidates["XPTUSD"][1]
            .reasons
            .iter()
            .any(|x| x.contains("mini/micro"))
    );
}

#[test]
fn adversarial_symbols_do_not_leak_into_other_instruments() {
    let (_, r) = run("adversarial");
    assert_eq!(candidate_names(&r, "XAUUSD"), vec!["XAUUSD"]);
    assert_eq!(candidate_names(&r, "US100"), vec!["USTEC"]);
    assert!(candidate_names(&r, "NATGAS").is_empty());
    assert!(candidate_names(&r, "BTCUSD").is_empty());
    assert_eq!(candidate_names(&r, "US30"), vec!["US30.cash"]);
    assert_eq!(resolved(&r, "US30"), "US30.cash");
    // Nothing from the dated or stock lists is anywhere.
    for (canonical, list) in &r.candidates {
        for c in list {
            for bad in [
                "GC0826",
                "NAS1226",
                "NatGas_X6",
                "NatGas_U6",
                "NDXX",
                "GOLD.N",
                "XAUUSDINDEX2",
                "XAUEUR",
                "BTCEUR",
                "US300",
            ] {
                assert_ne!(c.broker_symbol, bad, "{canonical}");
            }
        }
    }
}

#[test]
fn cash_roll_and_spot_names_score_a_bonus() {
    let with = detect(&[full("US500.cash", "US 500 Index")]);
    let without = detect(&[full("US500.xy", "US 500 Index")]);
    let a = &with.candidates["US500"][0];
    let b = &without.candidates["US500"][0];
    assert_eq!(a.score - b.score, 2, "{:?} vs {:?}", a.reasons, b.reasons);
    // The description alone also counts.
    let desc = detect(&[full("US500.xy", "US 500 Cash Index")]);
    assert_eq!(desc.candidates["US500"][0].score, a.score);
}

#[test]
fn undated_futures_are_penalised_not_excluded() {
    let r = detect(&[
        full("US500.x", "US 500 Futures"),
        full("US500.y", "US 500 Index"),
    ]);
    assert_eq!(candidate_names(&r, "US500"), vec!["US500.y", "US500.x"]);
    assert!(
        r.candidates["US500"][1]
            .reasons
            .iter()
            .any(|x| x.contains("futures"))
    );
}

#[test]
fn one_way_trading_scores_between_full_and_close_only() {
    let r = detect(&[
        sym("US30.a", "Wall Street 30", SymbolTradeMode::CloseOnly, true),
        sym("US30.b", "Wall Street 30", SymbolTradeMode::LongOnly, true),
        sym("US30.c", "Wall Street 30", SymbolTradeMode::Full, true),
    ]);
    assert_eq!(
        candidate_names(&r, "US30"),
        vec!["US30.c", "US30.b", "US30.a"]
    );
}

#[test]
fn a_symbol_is_claimed_by_one_instrument_only() {
    let make = |c: &str, kw: &str| Instrument {
        canonical: c.to_owned(),
        class: AssetClass::Other,
        aliases: vec![],
        keywords: vec![kw.to_owned()],
    };
    let catalog = InstrumentCatalog::from_instruments(vec![
        make("AAAA", "shared word"),
        make("BBBB", "shared word"),
    ])
    .unwrap();
    let r = automap(
        "T",
        &[
            full("S1", "the shared word"),
            sym("S2", "another shared word", SymbolTradeMode::Full, false),
        ],
        &catalog,
    );
    assert_eq!(
        r.map.explicit,
        map_of(&[("AAAA", "S1"), ("BBBB", "S2")]).explicit
    );
    assert_eq!(r.matched, 2);
    // Both are still listed as candidates for both.
    assert_eq!(r.candidates["BBBB"].len(), 2);
}

#[test]
fn catalog_without_fx_majors_has_no_pattern() {
    let catalog = InstrumentCatalog::from_instruments(vec![Instrument {
        canonical: "GOLDX".to_owned(),
        class: AssetClass::Metal,
        aliases: vec![],
        keywords: vec![],
    }])
    .unwrap();
    let r = automap("T", &[full("GOLDXm", "")], &catalog);
    assert!(r.map.patterns.is_empty());
    assert_eq!(r.map.explicit["GOLDX"], "GOLDXm");
}

// ---------------------------------------------------------------------------------
// Determinism, robustness, scale
// ---------------------------------------------------------------------------------

#[test]
fn results_do_not_depend_on_input_order() {
    for name in [
        "cfi",
        "equiti",
        "ic_markets_like",
        "exness_like",
        "adversarial",
        "kira",
    ] {
        let (fx, base) = run(name);
        let cat = real_catalog();
        let mut reversed = fx.symbols.clone();
        reversed.reverse();
        let mut rotated = fx.symbols.clone();
        rotated.rotate_left(fx.symbols.len() / 3);
        let mut interleaved: Vec<CatalogSymbol> = fx.symbols.iter().step_by(2).cloned().collect();
        interleaved.extend(fx.symbols.iter().skip(1).step_by(2).cloned());
        for (label, variant) in [
            ("reversed", reversed),
            ("rotated", rotated),
            ("interleaved", interleaved),
        ] {
            assert_eq!(automap(&fx.broker, &variant, &cat), base, "{name}: {label}");
        }
        assert_eq!(
            automap(&fx.broker, &fx.symbols, &cat),
            base,
            "{name}: repeat"
        );
    }
}

#[test]
fn duplicate_names_and_blank_names_are_ignored() {
    let mut symbols = majors("", "_", SymbolTradeMode::Full, true);
    let base = detect(&symbols);
    symbols.extend(majors("", "_", SymbolTradeMode::Full, true));
    symbols.push(full("", "blank"));
    symbols.push(full("   ", "blank"));
    let r = detect(&symbols);
    assert_eq!(r.map.explicit, base.map.explicit);
    assert_eq!(r.map.patterns, base.map.patterns);
    assert_eq!(r.candidates, base.candidates);
    assert_eq!(
        r.total,
        symbols.len(),
        "total counts what the caller passed"
    );
}

#[test]
fn unicode_and_odd_names_do_not_panic() {
    let weird = [
        "XAUUSD\u{20ac}",
        "\u{00fc}ber",
        "\u{1f4a9}XAUUSD",
        "XAUUSD\u{1f4a9}",
        "x",
        "#",
        "..",
        "EURUSD\u{0301}",
        "\u{0130}STANBUL",
        "XAU\u{00dc}SD",
        "A\u{0000}B",
    ];
    let symbols: Vec<CatalogSymbol> = weird
        .iter()
        .map(|n| full(n, "G\u{00f6}ld vs \u{20ac}"))
        .collect();
    let r = detect(&symbols);
    assert_eq!(r.total, weird.len());
}

#[test]
fn a_large_catalog_with_noise_gives_the_same_mapping() {
    let (fx, base) = run("cfi");
    let mut symbols = fx.symbols.clone();
    let letters = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut count = 0;
    'outer: for a in letters {
        for b in letters {
            for c in letters {
                for d in [None, Some(b'X'), Some(b'Q')] {
                    let mut name = String::new();
                    name.extend([*a as char, *b as char, *c as char]);
                    if let Some(d) = d {
                        name.push(d as char);
                    }
                    if symbols.iter().any(|s| s.name == name) {
                        continue;
                    }
                    let mut s = full(&name, &format!("{name} Corp"));
                    // Half of them have a stock path, half only a company-looking description.
                    if count % 2 == 0 {
                        s.path = format!("Stocks\\US\\{name}");
                    }
                    s.visible = count % 3 == 0;
                    symbols.push(s);
                    count += 1;
                    if count >= 6000 {
                        break 'outer;
                    }
                }
            }
        }
    }
    assert!(symbols.len() > 6000);
    let start = std::time::Instant::now();
    let r = automap(&fx.broker, &symbols, &real_catalog());
    let elapsed = start.elapsed();
    assert_eq!(r.map.explicit, base.map.explicit);
    assert_eq!(r.map.patterns, base.map.patterns);
    assert_eq!(r.matched, base.matched);
    assert!(
        elapsed.as_secs_f64() < 20.0,
        "6000 symbols took {elapsed:?}"
    );
}

#[test]
fn generated_layer_round_trips_through_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broker_symbols.generated.toml");
    let mut layer = BTreeMap::new();
    let mut expected = BTreeMap::new();
    for e in EXPECTATIONS {
        let (_, r) = run(e.fixture);
        expected.insert(r.broker.clone(), r.map.clone());
        layer.insert(r.broker.clone(), r.map);
    }
    Registry::save_layer(&layer, &path, "Auto-detected broker maps.").unwrap();
    let curated = dir.path().join("curated.toml");
    std::fs::write(&curated, "").unwrap();
    let loaded = Registry::load(&curated, &path, &dir.path().join("none.toml")).unwrap();
    // Raw Trading is produced by two fixtures; the last write wins in both maps alike.
    assert_eq!(loaded.generated, expected);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("pattern") || text.contains("patterns"));
    assert!(
        !text.contains('\\'),
        "replacements are written in the ${{1}} spelling"
    );
}

#[test]
fn generated_pattern_value_is_a_valid_pattern() {
    let p = Pattern {
        matches: "^([A-Z]{6})$".to_owned(),
        replace: "${1}_".to_owned(),
    };
    let (_, r) = run("cfi");
    assert_eq!(r.map.patterns, vec![p]);
    assert_eq!(
        r.map.note.as_deref(),
        Some("Auto-detected from 50 symbols, suffix `_`.")
    );
    let (_, r) = run("ic_markets_like");
    assert_eq!(
        r.map.note.as_deref(),
        Some("Auto-detected from 35 symbols.")
    );
    let reg = generated_registry(&r);
    assert_eq!(
        reg.resolve(&r.broker, "US100", &real_catalog()).source,
        Source::Generated
    );
    assert_eq!(
        reg.resolve(&r.broker, "EURUSD", &real_catalog()).source,
        Source::Passthrough
    );
}
