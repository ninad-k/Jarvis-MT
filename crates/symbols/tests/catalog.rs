//! Instrument catalog: loading, validation and name resolution.

mod common;

use common::{real_catalog, repo_root};
use jarvis_symbols::{AssetClass, Instrument, InstrumentCatalog, SymbolsError};

fn inst(canonical: &str, class: AssetClass, aliases: &[&str], keywords: &[&str]) -> Instrument {
    Instrument {
        canonical: canonical.to_owned(),
        class,
        aliases: aliases.iter().map(|s| (*s).to_owned()).collect(),
        keywords: keywords.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn invalid_message(result: Result<InstrumentCatalog, SymbolsError>) -> String {
    match result {
        Err(SymbolsError::Invalid { message, .. }) => message,
        other => panic!("expected an Invalid error, got {other:?}"),
    }
}

#[test]
fn real_catalog_loads_and_is_sane() {
    let catalog = real_catalog();
    let all = catalog.instruments();
    assert!(all.len() >= 35, "only {} instruments", all.len());
    for must in [
        "XAUUSD", "XAGUSD", "US100", "US30", "US500", "GER40", "UK100", "JP225", "WTI", "BRENT",
        "NATGAS", "BTCUSD", "ETHUSD", "EURUSD",
    ] {
        assert!(all.iter().any(|i| i.canonical == must), "{must} missing");
    }
    let classes = [
        AssetClass::Fx,
        AssetClass::Metal,
        AssetClass::Index,
        AssetClass::Energy,
        AssetClass::Crypto,
    ];
    for class in classes {
        assert!(
            all.iter().any(|i| i.class == class),
            "no {class:?} instrument"
        );
    }
    for i in all {
        assert_eq!(
            catalog.canonical_for(&i.canonical).as_deref(),
            Some(i.canonical.as_str())
        );
        for a in &i.aliases {
            assert_eq!(
                catalog.canonical_for(a).as_deref(),
                Some(i.canonical.as_str()),
                "alias {a}"
            );
        }
        for k in &i.keywords {
            assert_eq!(k, &k.to_lowercase(), "keywords are lower case");
        }
    }
}

#[test]
fn canonical_for_table() {
    let catalog = real_catalog();
    let cases: &[(&str, Option<&str>)] = &[
        ("XAUUSD", Some("XAUUSD")),
        ("xauusd", Some("XAUUSD")),
        ("gold", Some("XAUUSD")),
        ("GOLD", Some("XAUUSD")),
        ("Gold", Some("XAUUSD")),
        ("xau", Some("XAUUSD")),
        ("silver", Some("XAGUSD")),
        ("NAS100", Some("US100")),
        ("nasdaq", Some("US100")),
        ("USTEC", Some("US100")),
        ("ustech", Some("US100")),
        ("UT100", Some("US100")),
        ("NDX", Some("US100")),
        ("US100", Some("US100")),
        ("dow", Some("US30")),
        ("DJ30", Some("US30")),
        ("WS30", Some("US30")),
        ("spx", Some("US500")),
        ("SPX500", Some("US500")),
        ("SP500", Some("US500")),
        ("DAX", Some("GER40")),
        ("de40", Some("GER40")),
        ("GER30", Some("GER40")),
        ("ftse", Some("UK100")),
        ("nikkei", Some("JP225")),
        ("JPN225", Some("JP225")),
        ("usoil", Some("WTI")),
        ("XTIUSD", Some("WTI")),
        ("ukoil", Some("BRENT")),
        ("XBRUSD", Some("BRENT")),
        ("ngas", Some("NATGAS")),
        ("bitcoin", Some("BTCUSD")),
        ("BTC", Some("BTCUSD")),
        ("eth", Some("ETHUSD")),
        ("EURUSD", Some("EURUSD")),
        ("eurusd", Some("EURUSD")),
        ("EuRuSd", Some("EURUSD")),
        ("  eurusd  ", Some("EURUSD")),
        ("cable", Some("GBPUSD")),
        ("yen", Some("USDJPY")),
        // Bare six-letter pairs that are not catalog instruments pass when both halves are ISO codes.
        ("AUDCAD", Some("AUDCAD")),
        ("cadchf", Some("CADCHF")),
        ("EURSEK", Some("EURSEK")),
        ("USDMXN", Some("USDMXN")),
        ("GBPNZD", Some("GBPNZD")),
        // Not instruments.
        ("AAPL", None),
        ("XAUEUR", None),
        ("EURUSD_", None),
        ("EURUSDm", None),
        ("EURUSD.pr", None),
        ("ABCDEF", None),
        ("USDUSD", None),
        ("EURXXX", None),
        ("NAS", None),
        ("US10", None),
        ("", None),
        ("   ", None),
    ];
    for (input, want) in cases {
        assert_eq!(
            catalog.canonical_for(input).as_deref(),
            *want,
            "canonical_for({input:?})"
        );
    }
}

#[test]
fn from_instruments_validation_table() {
    let ok = |c: &str| inst(c, AssetClass::Index, &[], &[]);
    let cases: Vec<(&str, Vec<Instrument>, &str)> = vec![
        ("empty list", vec![], "no instruments"),
        ("lower-case canonical", vec![ok("us100")], "upper case"),
        ("too short", vec![ok("U")], "upper case"),
        ("too long", vec![ok("ABCDEFGHIJKLMNOPQ")], "upper case"),
        ("bad character", vec![ok("US-100")], "upper case"),
        ("dot in canonical", vec![ok("US.100")], "upper case"),
        (
            "duplicate canonical",
            vec![ok("US100"), ok("US100")],
            "defined twice",
        ),
        (
            "lower-case alias",
            vec![inst("US100", AssetClass::Index, &["nas100"], &[])],
            "alias",
        ),
        (
            "alias with symbol",
            vec![inst("US100", AssetClass::Index, &["NAS.100"], &[])],
            "alias",
        ),
        (
            "alias used twice in one instrument",
            vec![inst("US100", AssetClass::Index, &["NDX", "NDX"], &[])],
            "already used",
        ),
        (
            "alias used by two instruments",
            vec![
                inst("US100", AssetClass::Index, &["NDX"], &[]),
                inst("US500", AssetClass::Index, &["NDX"], &[]),
            ],
            "already used",
        ),
        (
            "alias equals another canonical",
            vec![
                ok("US100"),
                inst("US500", AssetClass::Index, &["US100"], &[]),
            ],
            "already used",
        ),
        (
            "alias equals own canonical",
            vec![inst("US100", AssetClass::Index, &["US100"], &[])],
            "already used",
        ),
        (
            "fx with five letters",
            vec![inst("EURUS", AssetClass::Fx, &[], &[])],
            "six letters",
        ),
        (
            "fx with seven letters",
            vec![inst("EURUSDX", AssetClass::Fx, &[], &[])],
            "six letters",
        ),
        (
            "fx with digits",
            vec![inst("EUR123", AssetClass::Fx, &[], &[])],
            "six letters",
        ),
        (
            "empty keyword",
            vec![inst("US100", AssetClass::Index, &[], &["  "])],
            "keyword",
        ),
    ];
    for (label, instruments, needle) in cases {
        let message = invalid_message(InstrumentCatalog::from_instruments(instruments));
        assert!(
            message.contains(needle),
            "{label}: {message:?} should mention {needle:?}"
        );
    }
}

#[test]
fn from_instruments_accepts_valid_and_normalises_keywords() {
    let catalog = InstrumentCatalog::from_instruments(vec![
        inst(
            "EURUSD",
            AssetClass::Fx,
            &["EURO"],
            &["  Euro VS US Dollar "],
        ),
        inst(
            "US100",
            AssetClass::Index,
            &["NAS100", "NDX"],
            &["Nasdaq 100"],
        ),
        inst("A1", AssetClass::Other, &[], &[]),
    ])
    .unwrap();
    assert_eq!(catalog.instruments().len(), 3);
    assert_eq!(catalog.instruments()[0].keywords, vec!["euro vs us dollar"]);
    assert_eq!(catalog.instruments()[1].keywords, vec!["nasdaq 100"]);
    assert_eq!(catalog.canonical_for("ndx").as_deref(), Some("US100"));
    assert_eq!(catalog.canonical_for("A1").as_deref(), Some("A1"));
    assert_eq!(catalog.clone(), catalog);
}

#[test]
fn load_reports_io_and_parse_errors() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.toml");
    match InstrumentCatalog::load(&missing) {
        Err(SymbolsError::Io { path, .. }) => assert!(path.contains("nope.toml")),
        other => panic!("expected Io, got {other:?}"),
    }
    let bad = dir.path().join("bad.toml");
    std::fs::write(&bad, "[[instrument]]\ncanonical = \n").unwrap();
    assert!(matches!(
        InstrumentCatalog::load(&bad),
        Err(SymbolsError::Invalid { .. })
    ));
    let bad_class = dir.path().join("class.toml");
    std::fs::write(
        &bad_class,
        "[[instrument]]\ncanonical = \"X1\"\nclass = \"stock\"\n",
    )
    .unwrap();
    assert!(matches!(
        InstrumentCatalog::load(&bad_class),
        Err(SymbolsError::Invalid { .. })
    ));
    let empty = dir.path().join("empty.toml");
    std::fs::write(&empty, "# nothing\n").unwrap();
    assert!(matches!(
        InstrumentCatalog::load(&empty),
        Err(SymbolsError::Invalid { .. })
    ));
}

#[test]
fn load_valid_file_with_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("instruments.toml");
    std::fs::write(
        &path,
        "[[instrument]]\ncanonical = \"XAUUSD\"\nclass = \"metal\"\naliases = [\"GOLD\"]\nkeywords = [\"Gold\"]\n\n[[instrument]]\ncanonical = \"EURGBP\"\nclass = \"fx\"\n",
    )
    .unwrap();
    let catalog = InstrumentCatalog::load(&path).unwrap();
    assert_eq!(catalog.instruments().len(), 2);
    assert!(catalog.instruments()[1].aliases.is_empty());
    assert_eq!(catalog.instruments()[0].keywords, vec!["gold"]);
    assert_eq!(catalog.canonical_for("gold").as_deref(), Some("XAUUSD"));
}

#[test]
fn config_file_has_unique_names() {
    // Guards against two instruments in config/instruments.toml claiming one alias.
    let text =
        std::fs::read_to_string(repo_root().join("config").join("instruments.toml")).unwrap();
    assert!(text.contains("[[instrument]]"));
    let catalog = real_catalog();
    let mut seen = std::collections::BTreeSet::new();
    for i in catalog.instruments() {
        assert!(seen.insert(i.canonical.clone()));
        for a in &i.aliases {
            assert!(seen.insert(a.clone()), "{a} used twice");
        }
    }
}
