//! Loading, validating and querying the canonical instrument catalog.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::{AssetClass, Instrument, InstrumentCatalog, SymbolsError};

#[derive(Deserialize)]
struct CatalogFile {
    #[serde(default)]
    instrument: Vec<Instrument>,
}

/// ISO 4217 codes (plus CNH) accepted as halves of a bare FX pair. Metals (`XAU`, ...)
/// are deliberately absent: they are catalog instruments or nothing.
const CURRENCIES: &[&str] = &[
    "AED", "ARS", "AUD", "BGN", "BHD", "BRL", "CAD", "CHF", "CLP", "CNH", "CNY", "COP", "CZK",
    "DKK", "EGP", "EUR", "GBP", "HKD", "HRK", "HUF", "IDR", "ILS", "INR", "ISK", "JPY", "KES",
    "KRW", "KWD", "MAD", "MXN", "MYR", "NGN", "NOK", "NZD", "OMR", "PEN", "PHP", "PKR", "PLN",
    "QAR", "RON", "RSD", "RUB", "SAR", "SEK", "SGD", "THB", "TND", "TRY", "TWD", "UAH", "USD",
    "VND", "ZAR",
];

pub(crate) fn is_currency(code: &str) -> bool {
    CURRENCIES.contains(&code)
}

fn invalid(path: &str, message: impl Into<String>) -> SymbolsError {
    SymbolsError::Invalid {
        path: path.to_owned(),
        message: message.into(),
    }
}

fn valid_name(s: &str) -> bool {
    (2..=16).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

pub(crate) fn load(path: &Path) -> Result<InstrumentCatalog, SymbolsError> {
    let shown = path.display().to_string();
    let text = std::fs::read_to_string(path).map_err(|e| SymbolsError::Io {
        path: shown.clone(),
        message: e.to_string(),
    })?;
    let file: CatalogFile = toml::from_str(&text).map_err(|e| invalid(&shown, e.to_string()))?;
    build(file.instrument, &shown)
}

pub(crate) fn from_instruments(
    instruments: Vec<Instrument>,
) -> Result<InstrumentCatalog, SymbolsError> {
    build(instruments, "<instruments>")
}

fn build(mut instruments: Vec<Instrument>, path: &str) -> Result<InstrumentCatalog, SymbolsError> {
    if instruments.is_empty() {
        return Err(invalid(path, "no instruments defined"));
    }
    // Every name (canonical or alias) may belong to exactly one instrument.
    let mut owner: HashMap<String, usize> = HashMap::new();
    for (i, inst) in instruments.iter().enumerate() {
        if !valid_name(&inst.canonical) {
            return Err(invalid(
                path,
                format!(
                    "canonical {:?} must be upper case [A-Z0-9]{{2,16}}",
                    inst.canonical
                ),
            ));
        }
        if inst.class == AssetClass::Fx
            && !(inst.canonical.len() == 6
                && inst.canonical.bytes().all(|b| b.is_ascii_uppercase()))
        {
            return Err(invalid(
                path,
                format!("FX canonical {:?} must be six letters", inst.canonical),
            ));
        }
        if let Some(prev) = owner.insert(inst.canonical.clone(), i) {
            return Err(invalid(
                path,
                format!(
                    "canonical {:?} defined twice (entries {} and {})",
                    inst.canonical,
                    prev + 1,
                    i + 1
                ),
            ));
        }
    }
    for (i, inst) in instruments.iter().enumerate() {
        for alias in &inst.aliases {
            if !valid_name(alias) {
                return Err(invalid(
                    path,
                    format!(
                        "alias {alias:?} of {} must be upper case [A-Z0-9]{{2,16}}",
                        inst.canonical
                    ),
                ));
            }
            if let Some(prev) = owner.insert(alias.clone(), i) {
                let other = &instruments[prev].canonical;
                return Err(invalid(
                    path,
                    format!(
                        "name {alias:?} (alias of {}) is already used by {other}",
                        inst.canonical
                    ),
                ));
            }
        }
    }
    for inst in &mut instruments {
        for kw in &mut inst.keywords {
            let cleaned = kw.trim().to_lowercase();
            if cleaned.is_empty() {
                return Err(invalid(
                    path,
                    format!("empty keyword for {}", inst.canonical),
                ));
            }
            *kw = cleaned;
        }
    }
    Ok(InstrumentCatalog {
        instruments,
        index: owner,
    })
}

impl InstrumentCatalog {
    pub(crate) fn canonical_for_impl(&self, name: &str) -> Option<String> {
        let key = name.trim().to_ascii_uppercase();
        if key.is_empty() {
            return None;
        }
        if let Some(&i) = self.index.get(&key) {
            return Some(self.instruments[i].canonical.clone());
        }
        if key.len() == 6 && key.bytes().all(|b| b.is_ascii_uppercase()) {
            let (base, quote) = key.split_at(3);
            if base != quote && is_currency(base) && is_currency(quote) {
                return Some(key);
            }
        }
        None
    }
}
