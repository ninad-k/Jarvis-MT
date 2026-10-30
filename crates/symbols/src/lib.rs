//! Broker-aware symbol mapping.
//!
//! Users and the rest of Arc MT speak in **canonical** instrument names (`XAUUSD`,
//! `US100`, `US30`, `EURUSD`, ...); every broker names them differently (`XAUUSD_`,
//! `GOLD_CASH`, `US100_Spot`, `USTEC`, `UT100Roll`, `EURUSD.pr`, ...). This crate maps
//! between the two, per broker:
//!
//! - [`InstrumentCatalog`]: canonical instruments with their generic aliases (`GOLD`,
//!   `NASDAQ`, `NAS100`, `DOW`, ...) and description keywords ("Nasdaq 100", "Dow Jones").
//!   Loaded from `config/instruments.toml`.
//! - [`Registry`]: three layers of per-broker maps, keyed by the broker's company name
//!   (`ACCOUNT_COMPANY`): user overrides (`data/broker_symbols.user.toml`, written from
//!   Settings) > curated (`config/broker_symbols.toml`, in the repo) > generated
//!   (`data/broker_symbols.generated.toml`, written by [`automap`]). Explicit entries of
//!   any layer win over patterns of any layer (a blanket curated pattern never hides a
//!   verified generated entry); among explicit entries and among patterns the layer
//!   order above applies, and patterns are tried in order (first full regex match
//!   wins). Nothing matched → the canonical name passes through unchanged.
//! - [`automap`]: infers a broker's map from its full MT5 symbol catalog (the EA's
//!   `list_symbols`), following the maintainer's earlier mapping approach
//!   with extra signals: tradability, Market Watch visibility, description keywords, and
//!   rejection of dated futures.
//!
//! Pure and I/O-light (only the explicit load/save functions touch files).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use jarvis_bridge_client::protocol::CatalogSymbol;
use serde::{Deserialize, Serialize};

mod automap;
mod catalog;
mod layers;
mod text;

#[derive(Debug, thiserror::Error)]
pub enum SymbolsError {
    #[error("cannot read {path}: {message}")]
    Io { path: String, message: String },
    #[error("invalid {path}: {message}")]
    Invalid { path: String, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetClass {
    Fx,
    Metal,
    Index,
    Energy,
    Crypto,
    Other,
}

/// One canonical instrument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instrument {
    /// Canonical name, upper case, e.g. "US100".
    pub canonical: String,
    pub class: AssetClass,
    /// Generic names that mean this instrument, upper case: "NASDAQ", "NAS100", "USTEC".
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Lower-case phrases that identify it in a broker's symbol description:
    /// "nasdaq 100", "us tech 100".
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// All canonical instruments.
#[derive(Debug, Clone, PartialEq)]
pub struct InstrumentCatalog {
    instruments: Vec<Instrument>,
    /// Upper-case canonical name or alias -> index into `instruments`.
    index: HashMap<String, usize>,
}

impl InstrumentCatalog {
    /// Load `config/instruments.toml` (`[[instrument]]` tables). Validates: canonical
    /// and aliases upper case `[A-Z0-9]{2,16}`, no alias used twice, FX canonicals are
    /// six letters.
    pub fn load(path: &Path) -> Result<Self, SymbolsError> {
        catalog::load(path)
    }

    pub fn from_instruments(instruments: Vec<Instrument>) -> Result<Self, SymbolsError> {
        catalog::from_instruments(instruments)
    }

    pub fn instruments(&self) -> &[Instrument] {
        &self.instruments
    }

    /// Canonical name for a canonical name or generic alias (case-insensitive):
    /// "gold" → "XAUUSD", "NAS100" → "US100", "eurusd" → "EURUSD". Bare six-letter FX
    /// pairs not in the catalog also pass (upper-cased) when both halves are ISO
    /// currency codes.
    pub fn canonical_for(&self, name: &str) -> Option<String> {
        self.canonical_for_impl(name)
    }
}

/// A `{ match, replace }` rule: regex full match on the canonical name, `$1`-style
/// replacement (an older spelling uses `\1`; both are accepted on load).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pattern {
    #[serde(rename = "match")]
    pub matches: String,
    pub replace: String,
}

/// One broker's map in one layer.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct BrokerMap {
    /// Canonical (or generic alias) → broker symbol.
    #[serde(default)]
    pub explicit: BTreeMap<String, String>,
    #[serde(default)]
    pub patterns: Vec<Pattern>,
    /// Free-text provenance, e.g. "verified from trade history 2026-10-12".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Where a resolution came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    User,
    Curated,
    Generated,
    Pattern,
    Passthrough,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    pub canonical: String,
    pub broker_symbol: String,
    pub source: Source,
}

/// The three layers for every broker. Cheap to clone.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Registry {
    pub user: BTreeMap<String, BrokerMap>,
    pub curated: BTreeMap<String, BrokerMap>,
    pub generated: BTreeMap<String, BrokerMap>,
}

impl Registry {
    /// Load the three files; a missing user/generated file is an empty layer, a missing
    /// curated file is an error. TOML shape: `[brokers."<company>"]` with `explicit`
    /// and `patterns` (see config/broker_symbols.toml).
    pub fn load(curated: &Path, generated: &Path, user: &Path) -> Result<Self, SymbolsError> {
        layers::load(curated, generated, user)
    }

    /// Load one optional layer file on its own (a missing file is an empty layer; a
    /// malformed one is an error). Lets a caller recover from a corrupt generated file
    /// without giving up the curated and user layers.
    pub fn load_layer(path: &Path) -> Result<BTreeMap<String, BrokerMap>, SymbolsError> {
        layers::load_layer(path, false)
    }

    /// Canonical → broker symbol for `broker` (exact company name). Order: explicit
    /// entries (user, curated, generated), then patterns (user, curated, generated),
    /// else passthrough.
    /// Explicit keys are matched after mapping them through `catalog.canonical_for`,
    /// so a curated `GOLD = "XAUUSD_"` also answers `XAUUSD`.
    pub fn resolve(
        &self,
        broker: &str,
        canonical: &str,
        catalog: &InstrumentCatalog,
    ) -> Resolution {
        layers::resolve(self, broker, canonical, catalog)
    }

    /// Broker symbol → canonical (reverse of `resolve`, checking every catalog
    /// instrument and the explicit entries); `None` when nothing maps to it.
    pub fn reverse(
        &self,
        broker: &str,
        broker_symbol: &str,
        catalog: &InstrumentCatalog,
    ) -> Option<String> {
        layers::reverse(self, broker, broker_symbol, catalog)
    }

    /// Replace the generated layer for one broker (others untouched).
    pub fn set_generated(&mut self, broker: &str, map: BrokerMap) {
        self.generated.insert(broker.to_owned(), map);
    }

    /// Set (`Some`) or clear (`None`) a user override for one canonical name.
    pub fn set_user_override(
        &mut self,
        broker: &str,
        canonical: &str,
        broker_symbol: Option<&str>,
    ) {
        layers::set_user_override(self, broker, canonical, broker_symbol);
    }

    /// Write one layer's file (`[brokers."<company>"]` tables), atomically (temp file +
    /// rename), with a header comment saying it is machine-written.
    pub fn save_layer(
        layer: &BTreeMap<String, BrokerMap>,
        path: &Path,
        header: &str,
    ) -> Result<(), SymbolsError> {
        layers::save_layer(layer, path, header)
    }
}

/// One candidate broker symbol for a canonical instrument, with why it scored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub broker_symbol: String,
    pub description: String,
    pub score: i32,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomapResult {
    pub broker: String,
    /// The generated layer for this broker.
    pub map: BrokerMap,
    /// Every canonical instrument with its ranked candidates (best first); empty when
    /// the broker has nothing that looks like it.
    pub candidates: BTreeMap<String, Vec<Candidate>>,
    pub detected_suffix: String,
    pub detected_prefix: String,
    /// Canonical instruments that got an entry (explicit or via the detected affix).
    pub matched: usize,
    /// Symbols in the broker catalog.
    pub total: usize,
}

/// Infer `broker`'s map from its full catalog.
///
/// Rules (extending the maintainer's earlier mapping approach):
/// - FX majors drive affix detection: a blanket suffix (or prefix) pattern for
///   six-letter FX canonicals is emitted only when ≥ 4 majors agree on one affix
///   (≤ 3 chars, one side only). When a broker lists BOTH a bare and an affixed set
///   (e.g. `EURUSD` and `EURUSD_`), prefer the set whose members are tradable and
///   visible in Market Watch; tie → the set the metals use.
/// - Every catalog instrument: candidates are symbols whose name contains the
///   canonical or an alias as a contiguous core with ≤ 3 chars of affix, OR whose
///   description contains one of its keywords. Score: +tradable (full) / −close-only
///   or disabled (excluded when disabled), +visible in Market Watch, +exact name,
///   +affix equals the detected affix, +"cash"/"spot"/"roll" in name or description,
///   −dated futures (month/year codes like `NAS1226`, `GC0826`, `_U6`, descriptions
///   containing "futures" with a month name) which are excluded outright, −"mini/
///   micro" variants. The best candidate becomes an explicit entry when it differs
///   from what the detected affix pattern would produce.
/// - Deterministic: same input → same output.
pub fn automap(
    broker: &str,
    symbols: &[CatalogSymbol],
    catalog: &InstrumentCatalog,
) -> AutomapResult {
    automap::automap(broker, symbols, catalog)
}
