//! Shared helpers for the integration tests.
#![allow(dead_code)]

use std::path::PathBuf;

use jarvis_bridge_client::protocol::{CatalogSymbol, SymbolTradeMode};
use jarvis_symbols::{AutomapResult, BrokerMap, InstrumentCatalog, Registry, automap};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Fixture {
    pub broker: String,
    pub server: String,
    pub symbols: Vec<CatalogSymbol>,
}

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

pub fn real_catalog() -> InstrumentCatalog {
    InstrumentCatalog::load(&repo_root().join("config").join("instruments.toml"))
        .expect("config/instruments.toml loads")
}

pub fn fixture(name: &str) -> Fixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub fn run(name: &str) -> (Fixture, AutomapResult) {
    let fx = fixture(name);
    let result = automap(&fx.broker, &fx.symbols, &real_catalog());
    (fx, result)
}

/// A registry whose only content is `result`'s generated layer for its broker.
pub fn generated_registry(result: &AutomapResult) -> Registry {
    let mut reg = Registry::default();
    reg.set_generated(&result.broker, result.map.clone());
    reg
}

pub fn real_registry() -> Registry {
    let root = repo_root().join("config");
    let missing = root.join("does-not-exist.toml");
    Registry::load(&root.join("broker_symbols.toml"), &missing, &missing)
        .expect("config/broker_symbols.toml loads")
}

pub fn sym(name: &str, description: &str, mode: SymbolTradeMode, visible: bool) -> CatalogSymbol {
    CatalogSymbol {
        name: name.to_owned(),
        description: description.to_owned(),
        path: String::new(),
        trade_mode: mode,
        visible,
        digits: 5,
        currency_base: String::new(),
        currency_profit: String::new(),
    }
}

pub fn full(name: &str, description: &str) -> CatalogSymbol {
    sym(name, description, SymbolTradeMode::Full, true)
}

pub fn map_of(pairs: &[(&str, &str)]) -> BrokerMap {
    BrokerMap {
        explicit: pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        ..BrokerMap::default()
    }
}
