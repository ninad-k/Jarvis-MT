//! Symbol resolution: what the user or the model typed -> canonical instrument -> the
//! current broker's symbol.
//!
//! 1. a spoken alias (`[symbols.aliases]` and the user's own) points at a canonical name;
//! 2. the instrument catalog maps canonical names and generic aliases (`GOLD`, `NAS100`) to
//!    the canonical name;
//! 3. the current broker's map ([`BrokerBook`]) turns the canonical name into the broker's
//!    symbol (user override > curated > auto-detected > unchanged).
//!
//! Broker symbols the user types (`XAUUSD_`, `US100_Spot`) are recognised in the broker's
//! catalog and reversed to their canonical name.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::broker::{BrokerBook, Resolved};

/// Lowercase and collapse whitespace so "Dow   Jones" matches the alias "dow jones".
pub fn alias_key(text: &str) -> String {
    text.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Uppercase and drop everything except ASCII letters/digits and `.`, `_`, `#`.
pub(crate) fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '#'))
        .collect::<String>()
        .to_ascii_uppercase()
}

#[derive(Clone)]
pub struct SymbolResolver {
    /// Spoken name (alias key) -> canonical name where the catalog knows it, else the
    /// cleaned name.
    aliases: HashMap<String, String>,
    book: Arc<BrokerBook>,
}

impl std::fmt::Debug for SymbolResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SymbolResolver")
            .field("aliases", &self.aliases.len())
            .finish_non_exhaustive()
    }
}

impl Default for SymbolResolver {
    fn default() -> Self {
        Self::new(&BTreeMap::new(), "")
    }
}

impl SymbolResolver {
    /// A resolver with no instrument catalog and no broker maps: only the aliases and the
    /// configured `suffix`, as before broker maps existed. Tests and degraded start-up.
    pub fn new(aliases: &BTreeMap<String, String>, suffix: &str) -> Self {
        Self::with_book(aliases, BrokerBook::standalone(suffix))
    }

    /// `aliases` keys are matched case-insensitively; values are canonical names (or, for
    /// older configs, the broker symbol before the suffix: both work).
    pub fn with_book(aliases: &BTreeMap<String, String>, book: Arc<BrokerBook>) -> Self {
        let mut resolver = Self {
            aliases: HashMap::new(),
            book,
        };
        resolver.add_aliases(aliases);
        resolver
    }

    fn add_aliases(&mut self, aliases: &BTreeMap<String, String>) {
        for (k, v) in aliases {
            let key = alias_key(k);
            if key.is_empty() {
                continue;
            }
            if let Some(target) = self.book.canonical_loose(v) {
                self.aliases.insert(key, target);
            }
        }
    }

    /// A copy of this resolver with a user's own aliases merged over the configured ones (a
    /// user entry wins).
    pub fn with_user_aliases(&self, user: &BTreeMap<String, String>) -> SymbolResolver {
        let mut merged = self.clone();
        merged.add_aliases(user);
        merged
    }

    pub fn book(&self) -> &Arc<BrokerBook> {
        &self.book
    }

    /// The name to resolve: the alias target, or the cleaned input.
    fn target(&self, input: &str) -> Option<String> {
        let key = alias_key(input);
        if key.is_empty() {
            return None;
        }
        let name = self
            .aliases
            .get(&key)
            .cloned()
            .unwrap_or_else(|| clean(input));
        (!name.is_empty()).then_some(name)
    }

    /// Resolve user text to the canonical name and the current broker's symbol, or `None`
    /// if nothing usable is left.
    pub fn resolve_full(&self, input: &str) -> Option<Resolved> {
        self.target(input).map(|name| self.book.resolve(&name))
    }

    /// Resolve user text to the broker symbol, or `None` if nothing usable is left.
    ///
    /// 1. trim + case-insensitive alias lookup (multi-word aliases work);
    /// 2. otherwise uppercase the text, dropping characters other than letters,
    ///    digits, `.`, `_`, `#`;
    /// 3. canonical name via the instrument catalog, then the current broker's map. With no
    ///    map for the broker (or no broker known) the configured suffix is appended instead.
    pub fn resolve(&self, input: &str) -> Option<String> {
        self.resolve_full(input).map(|r| r.broker)
    }

    /// The canonical name for user text (`gold` -> `XAUUSD`, `XAUUSD_` -> `XAUUSD`).
    pub fn canonical(&self, input: &str) -> Option<String> {
        self.resolve_full(input).map(|r| r.canonical)
    }

    /// The canonical name of a broker symbol the bridge returned (`XAUUSD_` -> `XAUUSD`).
    pub fn canonical_of_broker_symbol(&self, broker_symbol: &str) -> String {
        self.book.canonical_of_broker_symbol(broker_symbol)
    }

    /// `(spoken name, canonical name)` pairs, sorted, for the system prompt.
    pub fn alias_list(&self) -> Vec<(String, String)> {
        let mut v: Vec<_> = self
            .aliases
            .iter()
            .map(|(k, canonical)| (k.clone(), canonical.clone()))
            .collect();
        v.sort();
        v
    }

    /// Everything the intent parser should know as a name: the spoken aliases plus the
    /// instrument catalog's canonical names and generic aliases (`ustec`, `xau`), all
    /// pointing at canonical names.
    pub fn intent_aliases(&self) -> HashMap<String, String> {
        let mut table = HashMap::new();
        if let Some(catalog) = self.book.catalog() {
            for inst in catalog.instruments() {
                table.insert(alias_key(&inst.canonical), inst.canonical.clone());
                for a in &inst.aliases {
                    table.insert(alias_key(a), inst.canonical.clone());
                }
            }
        }
        table.extend(self.alias_list());
        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(suffix: &str) -> SymbolResolver {
        let aliases: BTreeMap<String, String> = [
            ("gold", "XAUUSD"),
            ("cold", "XAUUSD"),
            ("dow jones", "US30"),
            ("nasdaq", "US100"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        SymbolResolver::new(&aliases, suffix)
    }

    #[test]
    fn alias_lookup_is_case_insensitive() {
        let r = resolver("");
        for input in ["gold", "Gold", "GOLD", "  gold  ", "cold"] {
            assert_eq!(r.resolve(input).as_deref(), Some("XAUUSD"), "{input}");
        }
    }

    #[test]
    fn multi_word_alias() {
        let r = resolver("");
        assert_eq!(r.resolve("dow jones").as_deref(), Some("US30"));
        assert_eq!(r.resolve("Dow   JONES").as_deref(), Some("US30"));
    }

    #[test]
    fn unknown_text_is_uppercased_and_cleaned() {
        let r = resolver("");
        assert_eq!(r.resolve("xauusd").as_deref(), Some("XAUUSD"));
        assert_eq!(r.resolve("eur/usd").as_deref(), Some("EURUSD"));
        assert_eq!(r.resolve(" btc usd ").as_deref(), Some("BTCUSD"));
        assert_eq!(r.resolve("US_TECH.cash#").as_deref(), Some("US_TECH.CASH#"));
        assert_eq!(r.resolve("'; DROP --").as_deref(), Some("DROP"));
    }

    #[test]
    fn empty_input_resolves_to_none() {
        let r = resolver(".m");
        assert_eq!(r.resolve(""), None);
        assert_eq!(r.resolve("   "), None);
        assert_eq!(r.resolve("/*-"), None);
    }

    #[test]
    fn suffix_is_appended_once() {
        let r = resolver(".m");
        assert_eq!(r.resolve("gold").as_deref(), Some("XAUUSD.m"));
        assert_eq!(r.resolve("COLD").as_deref(), Some("XAUUSD.m"));
        assert_eq!(r.resolve("xauusd").as_deref(), Some("XAUUSD.m"));
        assert_eq!(r.resolve("XAUUSD.m").as_deref(), Some("XAUUSD.m"));
        assert_eq!(r.resolve("xauusd.M").as_deref(), Some("XAUUSD.m"));
        assert_eq!(r.resolve("dow jones").as_deref(), Some("US30.m"));
    }

    #[test]
    fn hash_suffix() {
        let r = resolver("#");
        assert_eq!(r.resolve("nasdaq").as_deref(), Some("US100#"));
        assert_eq!(r.resolve("US100#").as_deref(), Some("US100#"));
    }

    #[test]
    fn user_aliases_win_over_configured_ones() {
        let user: BTreeMap<String, String> = [("Yellow  Metal", "xauusd"), ("gold", "XAGUSD")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let base = resolver(".m");
        let merged = base.with_user_aliases(&user);
        assert_eq!(merged.resolve("yellow metal").as_deref(), Some("XAUUSD.m"));
        assert_eq!(merged.resolve("GOLD").as_deref(), Some("XAGUSD.m"));
        assert_eq!(merged.resolve("cold").as_deref(), Some("XAUUSD.m"));
        // The base resolver is untouched.
        assert_eq!(base.resolve("gold").as_deref(), Some("XAUUSD.m"));
        assert_eq!(
            base.resolve("yellow metal").as_deref(),
            Some("YELLOWMETAL.m")
        );
    }

    #[test]
    fn alias_list_shows_canonical_names_without_the_suffix() {
        let list = resolver(".m").alias_list();
        assert!(list.contains(&("gold".to_owned(), "XAUUSD".to_owned())));
    }

    #[test]
    fn canonical_strips_the_fallback_suffix() {
        let r = resolver(".m");
        assert_eq!(r.canonical("gold").as_deref(), Some("XAUUSD"));
        assert_eq!(r.canonical("XAUUSD.m").as_deref(), Some("XAUUSD"));
        assert_eq!(r.canonical_of_broker_symbol("US30.m"), "US30");
    }
}
