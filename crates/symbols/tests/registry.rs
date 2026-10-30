//! Registry: layering, precedence, reverse lookups, loading and saving.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::{map_of, real_catalog, real_registry};
use jarvis_symbols::{BrokerMap, InstrumentCatalog, Pattern, Registry, Source, SymbolsError};

const CFI: &str = "CFI Financial Markets L.L.C";
const RAW: &str = "Raw Trading Ltd";
const EQUITI: &str = "Equiti Securities Currencies Brokers L.L.C";
const KIRA: &str = "Kira Financial Brokers L.L.C";
const EXNESS: &str = "Exness";
const MQ: &str = "MetaQuotes Ltd.";

fn pattern(m: &str, r: &str) -> Pattern {
    Pattern {
        matches: m.to_owned(),
        replace: r.to_owned(),
    }
}

fn broker_map(explicit: &[(&str, &str)], patterns: &[(&str, &str)]) -> BrokerMap {
    BrokerMap {
        explicit: map_of(explicit).explicit,
        patterns: patterns.iter().map(|(m, r)| pattern(m, r)).collect(),
        note: None,
    }
}

fn registry(
    user: Option<BrokerMap>,
    curated: Option<BrokerMap>,
    generated: Option<BrokerMap>,
) -> Registry {
    let mut reg = Registry::default();
    if let Some(m) = user {
        reg.user.insert("B".to_owned(), m);
    }
    if let Some(m) = curated {
        reg.curated.insert("B".to_owned(), m);
    }
    if let Some(m) = generated {
        reg.generated.insert("B".to_owned(), m);
    }
    reg
}

fn resolve(reg: &Registry, broker: &str, name: &str) -> (String, Source) {
    let r = reg.resolve(broker, name, &real_catalog());
    (r.broker_symbol, r.source)
}

// ---------------------------------------------------------------------------------
// The shipped config/broker_symbols.toml
// ---------------------------------------------------------------------------------

#[test]
fn shipped_curated_file_loads_with_all_brokers() {
    let reg = real_registry();
    for broker in [CFI, RAW, EQUITI, KIRA, EXNESS, MQ] {
        assert!(reg.curated.contains_key(broker), "{broker}");
    }
    assert!(reg.user.is_empty() && reg.generated.is_empty());
    // The `${1}` spelling survives loading.
    assert_eq!(reg.curated[CFI].patterns, vec![pattern("^(.+)$", "${1}_")]);
}

#[test]
fn shipped_curated_resolve_table() {
    let reg = real_registry();
    let cases: &[(&str, &str, &str, Source)] = &[
        // CFI: the underscore set is the traded one; indices have their own names.
        (CFI, "XAUUSD", "XAUUSD_", Source::Curated),
        (CFI, "XAGUSD", "XAGUSD_", Source::Curated),
        (CFI, "US100", "US100_Spot", Source::Curated),
        (CFI, "US30", "US30_SPOT", Source::Curated),
        (CFI, "US500", "US500_SPOT", Source::Curated),
        (CFI, "GER40", "GER30_SPOT", Source::Curated),
        (CFI, "BTCUSD", "BTCUSD", Source::Curated),
        (CFI, "ETHUSD", "ETHUSD", Source::Curated),
        (CFI, "WTI", "WTI", Source::Curated),
        (CFI, "BRENT", "BRENT", Source::Curated),
        (CFI, "NATGAS", "NatGas_SPOT", Source::Curated),
        (CFI, "EURUSD", "EURUSD_", Source::Pattern),
        (CFI, "GBPJPY", "GBPJPY_", Source::Pattern),
        // Equiti
        (EQUITI, "EURUSD", "EURUSD.pr", Source::Pattern),
        (EQUITI, "GBPJPY", "GBPJPY.pr", Source::Pattern),
        (EQUITI, "XAUUSD", "XAUUSD.pr", Source::Curated),
        (EQUITI, "BTCUSD", "BTCUSD.lv", Source::Curated),
        (EQUITI, "US100", "UT100Roll", Source::Curated),
        (EQUITI, "US30", "US30Roll", Source::Curated),
        (EQUITI, "WTI", "WTI", Source::Passthrough),
        // Kira: no blanket pattern.
        (KIRA, "XAUUSD", "GOLD_CASH", Source::Curated),
        (KIRA, "US30", "US30cash", Source::Curated),
        (KIRA, "US100", "NDXcash", Source::Curated),
        (KIRA, "BRENT", "BRENTcash", Source::Curated),
        (KIRA, "EURUSD", "EURUSD", Source::Passthrough),
        (KIRA, "XAGUSD", "XAGUSD", Source::Passthrough),
        // Raw Trading (IC Markets)
        (RAW, "XAUUSD", "XAUUSD", Source::Curated),
        (RAW, "XAGUSD", "XAGUSD", Source::Curated),
        (RAW, "EURUSD", "EURUSD", Source::Passthrough),
        (RAW, "US100", "US100", Source::Passthrough),
        // Exness (unverified)
        (EXNESS, "XAUUSD", "XAUUSDm", Source::Curated),
        (EXNESS, "US30", "US30m", Source::Curated),
        (EXNESS, "BTCUSD", "BTCUSDm", Source::Curated),
        (EXNESS, "EURUSD", "EURUSDm", Source::Pattern),
        (EXNESS, "US100", "US100", Source::Passthrough),
        // MetaQuotes: empty explicit table, nothing changes.
        (MQ, "EURUSD", "EURUSD", Source::Passthrough),
        (MQ, "XAUUSD", "XAUUSD", Source::Passthrough),
        // Unknown broker: the name unchanged.
        ("Nobody Ltd", "XAUUSD", "XAUUSD", Source::Passthrough),
        ("Nobody Ltd", "US100", "US100", Source::Passthrough),
        // Broker names are matched exactly.
        (
            "cfi financial markets l.l.c",
            "XAUUSD",
            "XAUUSD",
            Source::Passthrough,
        ),
    ];
    for (broker, canonical, want, source) in cases {
        let r = reg.resolve(broker, canonical, &real_catalog());
        assert_eq!(
            (r.broker_symbol.as_str(), r.source),
            (*want, *source),
            "{broker}: {canonical}"
        );
        assert_eq!(r.canonical, *canonical);
    }
}

#[test]
fn aliases_and_case_resolve_to_the_canonical_entry() {
    let reg = real_registry();
    let cases: &[(&str, &str, &str, &str)] = &[
        (CFI, "GOLD", "XAUUSD", "XAUUSD_"),
        (CFI, "gold", "XAUUSD", "XAUUSD_"),
        (CFI, "NAS100", "US100", "US100_Spot"),
        (CFI, "nasdaq", "US100", "US100_Spot"),
        (CFI, "DOW", "US30", "US30_SPOT"),
        (CFI, "dax", "GER40", "GER30_SPOT"),
        (CFI, "  XAUUSD ", "XAUUSD", "XAUUSD_"),
        (EQUITI, "USTEC", "US100", "UT100Roll"),
        (KIRA, "gold", "XAUUSD", "GOLD_CASH"),
        (KIRA, "NDX", "US100", "NDXcash"),
        ("Nobody Ltd", "gold", "XAUUSD", "XAUUSD"),
        ("Nobody Ltd", "nas100", "US100", "US100"),
    ];
    for (broker, input, canonical, symbol) in cases {
        let r = reg.resolve(broker, input, &real_catalog());
        assert_eq!(
            (r.canonical.as_str(), r.broker_symbol.as_str()),
            (*canonical, *symbol),
            "{broker}: {input}"
        );
    }
}

#[test]
fn shipped_curated_reverse_table() {
    let reg = real_registry();
    let cat = real_catalog();
    let cases: &[(&str, &str, Option<&str>)] = &[
        (CFI, "XAUUSD_", Some("XAUUSD")),
        (CFI, "XAGUSD_", Some("XAGUSD")),
        (CFI, "US100_Spot", Some("US100")),
        (CFI, "US30_SPOT", Some("US30")),
        (CFI, "US500_SPOT", Some("US500")),
        (CFI, "GER30_SPOT", Some("GER40")),
        (CFI, "EURUSD_", Some("EURUSD")),
        (CFI, "USDJPY_", Some("USDJPY")),
        (CFI, "BTCUSD", Some("BTCUSD")),
        (CFI, "NatGas_SPOT", Some("NATGAS")),
        (CFI, "WTI", Some("WTI")),
        // The bare set and the dated futures are not what Arc MT maps to.
        (CFI, "EURUSD", None),
        (CFI, "XAUUSD", None),
        (CFI, "NAS1226", None),
        (CFI, "GC0826", None),
        (CFI, "NatGas_X6", None),
        (EQUITI, "EURUSD.pr", Some("EURUSD")),
        (EQUITI, "XAUUSD.pr", Some("XAUUSD")),
        (EQUITI, "UT100Roll", Some("US100")),
        (EQUITI, "US30Roll", Some("US30")),
        (EQUITI, "BTCUSD.lv", Some("BTCUSD")),
        (EQUITI, "UT100U6", None),
        (EQUITI, "EURUSD", None),
        (KIRA, "GOLD_CASH", Some("XAUUSD")),
        (KIRA, "US30cash", Some("US30")),
        (KIRA, "NDXcash", Some("US100")),
        (KIRA, "BRENTcash", Some("BRENT")),
        (KIRA, "EURUSD", Some("EURUSD")),
        (KIRA, "XAGUSD", Some("XAGUSD")),
        (KIRA, "XAUUSD", None),
        (RAW, "XAUUSD", Some("XAUUSD")),
        (RAW, "EURUSD", Some("EURUSD")),
        (RAW, "US100", Some("US100")),
        (RAW, "USTEC", None),
        (EXNESS, "XAUUSDm", Some("XAUUSD")),
        (EXNESS, "EURUSDm", Some("EURUSD")),
        (EXNESS, "US30m", Some("US30")),
        (EXNESS, "EURUSD", None),
        (MQ, "EURUSD", Some("EURUSD")),
        ("Nobody Ltd", "XAUUSD", Some("XAUUSD")),
        ("Nobody Ltd", "GOLD", None),
        (CFI, "", None),
        (CFI, "AAPL.OQ", None),
        // Matching is exact: no case folding, no trimming.
        (CFI, "xauusd_", None),
        (CFI, " XAUUSD_", None),
    ];
    for (broker, symbol, want) in cases {
        assert_eq!(
            reg.reverse(broker, symbol, &cat).as_deref(),
            *want,
            "{broker}: reverse({symbol:?})"
        );
    }
}

#[test]
fn reverse_inverts_resolve_for_every_instrument_and_broker() {
    let reg = real_registry();
    let cat = real_catalog();
    for broker in [CFI, RAW, EQUITI, KIRA, EXNESS, MQ, "Nobody Ltd"] {
        for inst in cat.instruments() {
            let resolved = reg.resolve(broker, &inst.canonical, &cat);
            // Several canonicals can share one broker symbol only through a broker-side
            // alias; the shipped files have none, so the inverse must be exact.
            assert_eq!(
                reg.reverse(broker, &resolved.broker_symbol, &cat)
                    .as_deref(),
                Some(inst.canonical.as_str()),
                "{broker}: {}",
                inst.canonical
            );
        }
    }
}

// ---------------------------------------------------------------------------------
// Layering and precedence
// ---------------------------------------------------------------------------------

#[test]
fn precedence_user_curated_generated_pattern_passthrough() {
    let user = broker_map(&[("XAUUSD", "U_GOLD")], &[]);
    let curated = broker_map(&[("XAUUSD", "C_GOLD"), ("XAGUSD", "C_SILVER")], &[]);
    let generated = broker_map(
        &[
            ("XAUUSD", "G_GOLD"),
            ("XAGUSD", "G_SILVER"),
            ("US30", "G_US30"),
        ],
        &[("^([A-Z]{6})$", "${1}x")],
    );
    let reg = registry(Some(user), Some(curated), Some(generated));
    let cases: &[(&str, &str, Source)] = &[
        ("XAUUSD", "U_GOLD", Source::User),
        ("XAGUSD", "C_SILVER", Source::Curated),
        ("US30", "G_US30", Source::Generated),
        ("EURUSD", "EURUSDx", Source::Pattern),
        ("US100", "US100", Source::Passthrough),
        ("WTI", "WTI", Source::Passthrough),
    ];
    for (canonical, want, source) in cases {
        assert_eq!(
            resolve(&reg, "B", canonical),
            ((*want).to_owned(), *source),
            "{canonical}"
        );
    }
    assert_eq!(
        resolve(&reg, "Other", "XAUUSD"),
        ("XAUUSD".to_owned(), Source::Passthrough)
    );
}

#[test]
fn explicit_entries_of_every_layer_beat_patterns_of_every_layer() {
    // A curated blanket pattern must not shadow a verified generated entry.
    let curated = broker_map(&[], &[("^(.+)$", "${1}_")]);
    let generated = broker_map(&[("HK50", "HK50_SPOT")], &[]);
    let reg = registry(None, Some(curated), Some(generated));
    assert_eq!(
        resolve(&reg, "B", "HK50"),
        ("HK50_SPOT".to_owned(), Source::Generated)
    );
    assert_eq!(
        resolve(&reg, "B", "EURUSD"),
        ("EURUSD_".to_owned(), Source::Pattern)
    );
}

#[test]
fn patterns_are_tried_layer_by_layer_in_order_first_match_wins() {
    let user = broker_map(&[], &[("^XAU.*$", "USERGOLD")]);
    let curated = broker_map(&[], &[("^EUR(.+)$", "E${1}"), ("^(.+)$", "C${1}")]);
    let generated = broker_map(&[], &[("^(.+)$", "G${1}")]);
    let reg = registry(Some(user), Some(curated), Some(generated));
    let cases: &[(&str, &str)] = &[
        ("XAUUSD", "USERGOLD"),
        ("EURGBP", "EGBP"),
        ("GBPUSD", "CGBPUSD"),
        ("US30", "CUS30"),
    ];
    for (canonical, want) in cases {
        assert_eq!(
            resolve(&reg, "B", canonical),
            ((*want).to_owned(), Source::Pattern),
            "{canonical}"
        );
    }
    // Generated patterns are reached only when higher layers have none matching.
    let reg = registry(
        None,
        Some(broker_map(&[], &[("^EUR.*$", "X")])),
        Some(broker_map(&[], &[("^(.+)$", "G${1}")])),
    );
    assert_eq!(resolve(&reg, "B", "EURUSD").0, "X");
    assert_eq!(resolve(&reg, "B", "GBPUSD").0, "GGBPUSD");
}

#[test]
fn patterns_are_full_matches() {
    let reg = registry(
        None,
        Some(broker_map(&[], &[("EUR", "X"), ("^US30$", "Y")])),
        None,
    );
    // `EUR` alone does not match `EURUSD`; neither does an anchored literal match a prefix.
    assert_eq!(
        resolve(&reg, "B", "EURUSD"),
        ("EURUSD".to_owned(), Source::Passthrough)
    );
    assert_eq!(
        resolve(&reg, "B", "US300"),
        ("US300".to_owned(), Source::Passthrough)
    );
    assert_eq!(
        resolve(&reg, "B", "US30"),
        ("Y".to_owned(), Source::Pattern)
    );
    assert_eq!(resolve(&reg, "B", "EUR"), ("X".to_owned(), Source::Pattern));
}

#[test]
fn explicit_keys_may_be_aliases_and_the_exact_key_wins() {
    let curated = broker_map(
        &[
            ("GOLD", "XAUUSD_"),
            ("NAS100", "US100_Spot"),
            ("DOW", "US30_A"),
            ("DJ30", "US30_B"),
        ],
        &[],
    );
    let reg = registry(None, Some(curated), None);
    assert_eq!(
        resolve(&reg, "B", "XAUUSD"),
        ("XAUUSD_".to_owned(), Source::Curated)
    );
    assert_eq!(
        resolve(&reg, "B", "gold"),
        ("XAUUSD_".to_owned(), Source::Curated)
    );
    assert_eq!(resolve(&reg, "B", "US100").0, "US100_Spot");
    // Two alias keys for one instrument: the alphabetically first key wins.
    assert_eq!(resolve(&reg, "B", "US30").0, "US30_B");
    assert_eq!(
        reg.reverse("B", "XAUUSD_", &real_catalog()).as_deref(),
        Some("XAUUSD")
    );

    // A key that *is* the canonical name beats alias keys in the same layer.
    let curated = broker_map(&[("GOLD", "ALIAS_GOLD"), ("XAUUSD", "EXACT_GOLD")], &[]);
    let reg = registry(None, Some(curated), None);
    assert_eq!(resolve(&reg, "B", "XAUUSD").0, "EXACT_GOLD");
    assert_eq!(reg.reverse("B", "ALIAS_GOLD", &real_catalog()), None);
    assert_eq!(
        reg.reverse("B", "EXACT_GOLD", &real_catalog()).as_deref(),
        Some("XAUUSD")
    );

    // A user alias key overrides a curated canonical key (layers beat key spelling).
    let reg = registry(
        Some(broker_map(&[("GOLD", "USER_GOLD")], &[])),
        Some(broker_map(&[("XAUUSD", "C")], &[])),
        None,
    );
    assert_eq!(
        resolve(&reg, "B", "XAUUSD"),
        ("USER_GOLD".to_owned(), Source::User)
    );
}

#[test]
fn non_catalog_names_resolve_and_reverse_through_explicit_entries() {
    let curated = broker_map(&[("TSLA", "TSLA.OQ")], &[]);
    let reg = registry(None, Some(curated), None);
    assert_eq!(
        resolve(&reg, "B", "TSLA"),
        ("TSLA.OQ".to_owned(), Source::Curated)
    );
    assert_eq!(
        resolve(&reg, "B", "tsla"),
        ("TSLA.OQ".to_owned(), Source::Curated)
    );
    assert_eq!(
        reg.reverse("B", "TSLA.OQ", &real_catalog()).as_deref(),
        Some("TSLA")
    );
    // Unknown names pass through with their spelling.
    assert_eq!(
        resolve(&reg, "B", "Foo.Bar"),
        ("Foo.Bar".to_owned(), Source::Passthrough)
    );
    // Unknown names do not appear out of nowhere in reverse.
    assert_eq!(reg.reverse("B", "Foo.Bar", &real_catalog()), None);
}

#[test]
fn a_higher_layer_hides_lower_entries_from_reverse() {
    let reg = registry(
        Some(broker_map(&[("XAUUSD", "MY_GOLD")], &[])),
        Some(broker_map(&[("XAUUSD", "XAUUSD_")], &[])),
        None,
    );
    let cat = real_catalog();
    assert_eq!(reg.reverse("B", "MY_GOLD", &cat).as_deref(), Some("XAUUSD"));
    assert_eq!(reg.reverse("B", "XAUUSD_", &cat), None);
    assert_eq!(reg.reverse("B", "XAUUSD", &cat), None);
}

#[test]
fn reverse_through_patterns_and_ambiguity() {
    let pat = registry(
        None,
        Some(broker_map(
            &[("US30", "US30_SPOT")],
            &[("^([A-Z]{6})$", "#${1}")],
        )),
        None,
    );
    let cat = real_catalog();
    assert_eq!(pat.reverse("B", "#EURUSD", &cat).as_deref(), Some("EURUSD"));
    assert_eq!(pat.reverse("B", "#XAUUSD", &cat).as_deref(), Some("XAUUSD"));
    assert_eq!(pat.reverse("B", "EURUSD", &cat), None);
    assert_eq!(pat.reverse("B", "US30_SPOT", &cat).as_deref(), Some("US30"));
    assert_eq!(pat.reverse("B", "US30", &cat), None);
    // Two canonicals sharing one broker symbol: the higher layer wins, then the
    // alphabetically first canonical.
    let shared = registry(
        None,
        Some(broker_map(&[("US500", "SPX"), ("US100", "SPX")], &[])),
        Some(broker_map(&[("US30", "SPX")], &[])),
    );
    assert_eq!(shared.reverse("B", "SPX", &cat).as_deref(), Some("US100"));
}

#[test]
fn invalid_regex_in_a_programmatic_registry_is_skipped() {
    let reg = registry(
        None,
        Some(broker_map(&[], &[("([", "X"), ("^(.+)$", "${1}z")])),
        None,
    );
    assert_eq!(
        resolve(&reg, "B", "EURUSD"),
        ("EURUSDz".to_owned(), Source::Pattern)
    );
}

#[test]
fn backslash_replacement_spellings_work_without_loading() {
    let reg = registry(None, Some(broker_map(&[], &[("^(.+)$", "\\1_")])), None);
    assert_eq!(resolve(&reg, "B", "EURUSD").0, "EURUSD_");
    let reg = registry(None, Some(broker_map(&[], &[("^(.+)$", "$1m")])), None);
    assert_eq!(resolve(&reg, "B", "EURUSD").0, "EURUSDm");
}

// ---------------------------------------------------------------------------------
// set_user_override / set_generated
// ---------------------------------------------------------------------------------

#[test]
fn set_user_override_sets_clears_and_cleans_up() {
    let cat = real_catalog();
    let mut reg = real_registry();
    assert_eq!(reg.resolve(CFI, "US100", &cat).broker_symbol, "US100_Spot");

    reg.set_user_override(CFI, "US100", Some("NAS100.cash"));
    let r = reg.resolve(CFI, "US100", &cat);
    assert_eq!(
        (r.broker_symbol.as_str(), r.source),
        ("NAS100.cash", Source::User)
    );
    assert_eq!(
        reg.reverse(CFI, "NAS100.cash", &cat).as_deref(),
        Some("US100")
    );
    assert_eq!(reg.reverse(CFI, "US100_Spot", &cat), None);
    assert_eq!(reg.user[CFI].explicit["US100"], "NAS100.cash");

    // Replace, with surrounding blanks trimmed and the key upper-cased.
    reg.set_user_override(CFI, "us100", Some("  USTEC.x "));
    assert_eq!(reg.user[CFI].explicit.len(), 1);
    assert_eq!(reg.resolve(CFI, "US100", &cat).broker_symbol, "USTEC.x");

    // A second instrument and a second broker.
    reg.set_user_override(CFI, "XAUUSD", Some("GOLD.x"));
    reg.set_user_override(KIRA, "XAUUSD", Some("XAUUSD"));
    assert_eq!(reg.user.len(), 2);

    // Clear one.
    reg.set_user_override(CFI, "US100", None);
    assert_eq!(reg.resolve(CFI, "US100", &cat).broker_symbol, "US100_Spot");
    assert_eq!(reg.resolve(CFI, "XAUUSD", &cat).broker_symbol, "GOLD.x");
    assert!(reg.user.contains_key(CFI));

    // Clearing the last override drops the broker; clearing something absent is a no-op.
    reg.set_user_override(CFI, "XAUUSD", None);
    assert!(!reg.user.contains_key(CFI));
    reg.set_user_override(CFI, "XAUUSD", None);
    reg.set_user_override("Never Seen", "XAUUSD", None);
    assert!(!reg.user.contains_key("Never Seen"));
    assert_eq!(reg.resolve(CFI, "XAUUSD", &cat).broker_symbol, "XAUUSD_");

    // A blank symbol clears instead of storing an empty mapping.
    reg.set_user_override(KIRA, "XAUUSD", Some("   "));
    assert!(reg.user.is_empty());
    // Curated and generated layers were never touched.
    assert_eq!(reg.curated, real_registry().curated);
}

#[test]
fn set_generated_replaces_only_that_broker() {
    let mut reg = Registry::default();
    reg.set_generated("A", map_of(&[("XAUUSD", "X1")]));
    reg.set_generated("B", map_of(&[("XAUUSD", "Y1")]));
    reg.set_generated("A", map_of(&[("US30", "X2")]));
    let cat = real_catalog();
    assert_eq!(reg.resolve("A", "XAUUSD", &cat).broker_symbol, "XAUUSD");
    assert_eq!(reg.resolve("A", "US30", &cat).broker_symbol, "X2");
    assert_eq!(reg.resolve("B", "XAUUSD", &cat).broker_symbol, "Y1");
    assert_eq!(reg.generated.len(), 2);
}

// ---------------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------------

fn write(dir: &Path, name: &str, text: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

fn load_err(curated: &str) -> SymbolsError {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "c.toml", curated);
    let none = dir.path().join("none.toml");
    Registry::load(&path, &none, &none).expect_err("should fail")
}

#[test]
fn load_missing_layers_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let curated = write(
        dir.path(),
        "curated.toml",
        "[brokers.\"B\"]\nnote = \"hi\"\n",
    );
    let none = dir.path().join("none.toml");

    let reg = Registry::load(&curated, &none, &none).unwrap();
    assert_eq!(reg.curated["B"].note.as_deref(), Some("hi"));
    assert!(reg.user.is_empty() && reg.generated.is_empty());

    match Registry::load(&none, &none, &none) {
        Err(SymbolsError::Io { path, .. }) => assert!(path.contains("none.toml")),
        other => panic!("expected Io, got {other:?}"),
    }
    // A broken generated or user file is an error, not a silent empty layer.
    let broken = write(dir.path(), "broken.toml", "[brokers\n");
    assert!(matches!(
        Registry::load(&curated, &broken, &none),
        Err(SymbolsError::Invalid { .. })
    ));
    assert!(matches!(
        Registry::load(&curated, &none, &broken),
        Err(SymbolsError::Invalid { .. })
    ));

    // Empty files are empty layers.
    let empty = write(dir.path(), "empty.toml", "");
    let reg = Registry::load(&curated, &empty, &empty).unwrap();
    assert!(reg.generated.is_empty());
}

#[test]
fn load_layer_alone_treats_a_missing_file_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        Registry::load_layer(&dir.path().join("none.toml"))
            .unwrap()
            .is_empty()
    );
    let ok = write(
        dir.path(),
        "g.toml",
        "[brokers.\"B\".explicit]
XAUUSD = \"X\"
",
    );
    assert_eq!(
        Registry::load_layer(&ok).unwrap()["B"].explicit["XAUUSD"],
        "X"
    );
    let broken = write(dir.path(), "b.toml", "[[[");
    assert!(matches!(
        Registry::load_layer(&broken),
        Err(SymbolsError::Invalid { .. })
    ));
}

#[test]
fn load_all_three_layers_and_resolve() {
    let dir = tempfile::tempdir().unwrap();
    let curated = write(
        dir.path(),
        "c.toml",
        "[brokers.\"B\"]\npatterns = [{ match = \"^(.+)$\", replace = \"${1}_\" }]\n[brokers.\"B\".explicit]\nXAUUSD = \"CUR\"\n",
    );
    let generated = write(
        dir.path(),
        "g.toml",
        "[brokers.\"B\"]\n[brokers.\"B\".explicit]\nXAUUSD = \"GEN\"\nUS30 = \"GEN30\"\n[brokers.\"G\".explicit]\nXAUUSD = \"GONLY\"\n",
    );
    let user = write(
        dir.path(),
        "u.toml",
        "[brokers.\"B\".explicit]\nUS30 = \"USR30\"\n",
    );
    let reg = Registry::load(&curated, &generated, &user).unwrap();
    assert_eq!(
        resolve(&reg, "B", "XAUUSD"),
        ("CUR".to_owned(), Source::Curated)
    );
    assert_eq!(
        resolve(&reg, "B", "US30"),
        ("USR30".to_owned(), Source::User)
    );
    assert_eq!(
        resolve(&reg, "B", "EURUSD"),
        ("EURUSD_".to_owned(), Source::Pattern)
    );
    assert_eq!(
        resolve(&reg, "G", "XAUUSD"),
        ("GONLY".to_owned(), Source::Generated)
    );
}

#[test]
fn load_normalises_replacement_spellings() {
    let dir = tempfile::tempdir().unwrap();
    let text = r##"
[brokers."B"]
patterns = [
  { match = "^(.+)$", replace = "\\1_" },
  { match = "^(.+)$", replace = "$1m" },
  { match = "^(.+)$", replace = "#${1}" },
  { match = "^(.+)-(.+)$", replace = '\2\1' },
]
"##;
    let path = write(dir.path(), "c.toml", text);
    let none = dir.path().join("none.toml");
    let reg = Registry::load(&path, &none, &none).unwrap();
    let replaces: Vec<&str> = reg.curated["B"]
        .patterns
        .iter()
        .map(|p| p.replace.as_str())
        .collect();
    assert_eq!(replaces, vec!["${1}_", "${1}m", "#${1}", "${2}${1}"]);
}

#[test]
fn load_validation_table() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "bad regex",
            "[brokers.\"B\"]\npatterns = [{ match = \"([\", replace = \"x\" }]\n",
            "invalid regex",
        ),
        (
            "replacement group beyond the pattern",
            "[brokers.\"B\"]\npatterns = [{ match = \"^(.+)$\", replace = \"${2}\" }]\n",
            "group 2",
        ),
        (
            "backslash group beyond the pattern",
            "[brokers.\"B\"]\npatterns = [{ match = \"^.+$\", replace = \"\\\\1\" }]\n",
            "group 1",
        ),
        (
            "empty symbol",
            "[brokers.\"B\".explicit]\nXAUUSD = \"\"\n",
            "non-empty",
        ),
        (
            "empty key",
            "[brokers.\"B\".explicit]\n\"\" = \"X\"\n",
            "non-empty",
        ),
    ];
    for (label, text, needle) in cases {
        match load_err(text) {
            SymbolsError::Invalid { message, .. } => {
                assert!(message.contains(needle), "{label}: {message}")
            }
            other => panic!("{label}: {other:?}"),
        }
    }
    // Wrong types are TOML errors.
    assert!(matches!(
        load_err("[brokers.\"B\"]\nexplicit = 5\n"),
        SymbolsError::Invalid { .. }
    ));
    assert!(matches!(
        load_err("[brokers.\"B\"]\npatterns = [{ match = \"x\" }]\n"),
        SymbolsError::Invalid { .. }
    ));
}

// ---------------------------------------------------------------------------------
// Saving
// ---------------------------------------------------------------------------------

fn sample_layer() -> BTreeMap<String, BrokerMap> {
    let mut layer = BTreeMap::new();
    layer.insert(
        CFI.to_owned(),
        BrokerMap {
            explicit: map_of(&[
                ("XAUUSD", "XAUUSD_"),
                ("US100", "US100_Spot"),
                ("BTCUSD", "BTCUSD"),
            ])
            .explicit,
            patterns: vec![pattern("^([A-Z]{6})$", "${1}_"), pattern("^(.+)$", "x${1}")],
            note: Some("Verified \"live\" 2026-10-12".to_owned()),
        },
    );
    layer.insert(
        "Zeta \\ Odd \"Name\"".to_owned(),
        BrokerMap {
            explicit: map_of(&[("TSLA", "TSLA.OQ"), ("A.B", "wei\u{00e9}rd\tvalue")]).explicit,
            patterns: vec![],
            note: None,
        },
    );
    layer.insert("Empty Broker".to_owned(), BrokerMap::default());
    layer
}

#[test]
fn save_layer_round_trips_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("deeper").join("layer.toml");
    let layer = sample_layer();
    Registry::save_layer(&layer, &path, "Written by the test suite.\nSecond line.").unwrap();
    let curated = dir.path().join("c.toml");
    std::fs::write(&curated, "").unwrap();
    let loaded = Registry::load(&curated, &dir.path().join("none.toml"), &path).unwrap();
    assert_eq!(loaded.user, layer);
}

#[test]
fn save_layer_output_is_stable_sorted_and_commented() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.toml");
    let b = dir.path().join("b.toml");
    let mut one = BTreeMap::new();
    one.insert(
        "B2".to_owned(),
        map_of(&[("XAUUSD", "1"), ("US30", "2"), ("BTCUSD", "3")]),
    );
    one.insert("B1".to_owned(), map_of(&[("EURUSD", "4")]));
    let mut two = BTreeMap::new();
    two.insert("B1".to_owned(), map_of(&[("EURUSD", "4")]));
    let mut explicit = BTreeMap::new();
    for (k, v) in [("BTCUSD", "3"), ("US30", "2"), ("XAUUSD", "1")] {
        explicit.insert(k.to_owned(), v.to_owned());
    }
    two.insert(
        "B2".to_owned(),
        BrokerMap {
            explicit,
            ..BrokerMap::default()
        },
    );
    Registry::save_layer(&one, &a, "header").unwrap();
    Registry::save_layer(&two, &b, "header").unwrap();
    let text = std::fs::read_to_string(&a).unwrap();
    assert_eq!(text, std::fs::read_to_string(&b).unwrap());

    assert!(text.starts_with("# Machine-written"), "{text}");
    assert!(text.contains("# header\n"));
    assert!(text.find("[brokers.\"B1\"]").unwrap() < text.find("[brokers.\"B2\"]").unwrap());
    let explicit_block = &text[text.find("[brokers.\"B2\".explicit]").unwrap()..];
    let btc = explicit_block.find("BTCUSD").unwrap();
    let us30 = explicit_block.find("US30").unwrap();
    let xau = explicit_block.find("XAUUSD").unwrap();
    assert!(btc < us30 && us30 < xau);
    assert!(!text.contains('\r'), "LF line endings");
    assert!(text.ends_with('\n'));

    // Saving the same layer again changes nothing.
    Registry::save_layer(&one, &a, "header").unwrap();
    assert_eq!(std::fs::read_to_string(&a).unwrap(), text);
}

#[test]
fn save_layer_header_variants_and_pattern_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("l.toml");
    let layer = sample_layer();
    Registry::save_layer(&layer, &path, "# already a comment\n\nplain line").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("# already a comment\n#\n# plain line\n"),
        "{text}"
    );
    // Pattern order is meaningful and kept.
    let first = text.find("^([A-Z]{6})$").unwrap();
    let second = text.find("^(.+)$").unwrap();
    assert!(first < second);
    // An empty header still carries the machine-written notice.
    Registry::save_layer(&BTreeMap::new(), &path, "").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with("# Machine-written") && text.lines().count() == 1,
        "{text}"
    );
}

#[test]
fn save_layer_replaces_atomically_and_leaves_no_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broker_symbols.user.toml");
    std::fs::write(&path, "old contents that are not even toml [[[").unwrap();
    let mut layer = BTreeMap::new();
    layer.insert("B".to_owned(), map_of(&[("XAUUSD", "X")]));
    Registry::save_layer(&layer, &path, "h").unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("XAUUSD = \"X\"")
    );
    for round in 0..5 {
        layer
            .get_mut("B")
            .unwrap()
            .explicit
            .insert("US30".to_owned(), format!("v{round}"));
        Registry::save_layer(&layer, &path, "h").unwrap();
    }
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec!["broker_symbols.user.toml".to_owned()],
        "no temp files left"
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("US30 = \"v4\"")
    );
}

#[test]
fn save_layer_into_an_unwritable_place_reports_io_and_keeps_the_old_file() {
    let dir = tempfile::tempdir().unwrap();
    // The target path is a directory: rename must fail and the temp file must be removed.
    let target = dir.path().join("target.toml");
    std::fs::create_dir(&target).unwrap();
    let mut layer = BTreeMap::new();
    layer.insert("B".to_owned(), BrokerMap::default());
    let err = Registry::save_layer(&layer, &target, "h").unwrap_err();
    assert!(matches!(err, SymbolsError::Io { .. }), "{err:?}");
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers.len(), 1, "{leftovers:?}");
}

#[test]
fn saving_and_loading_a_layer_with_backslash_replacements_normalises_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("l.toml");
    let mut layer = BTreeMap::new();
    layer.insert("B".to_owned(), broker_map(&[], &[("^(.+)$", "\\1_")]));
    Registry::save_layer(&layer, &path, "h").unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("replace = \"${1}_\"")
    );
}

#[test]
fn reverse_is_fast_enough_for_per_row_use() {
    let reg = real_registry();
    let cat: InstrumentCatalog = real_catalog();
    let symbols = [
        "XAUUSD_",
        "US100_Spot",
        "EURUSD_",
        "NatGas_X6",
        "GC0826",
        "AAPL.OQ",
    ];
    let start = std::time::Instant::now();
    for i in 0..10_000 {
        let _ = reg.reverse(CFI, symbols[i % symbols.len()], &cat);
    }
    let elapsed = start.elapsed();
    // Release builds do this in a few milliseconds; the bound only catches an accidental
    // per-call regex compilation (seconds).
    assert!(
        elapsed.as_secs_f64() < 8.0,
        "10k reverse lookups took {elapsed:?}"
    );
}
