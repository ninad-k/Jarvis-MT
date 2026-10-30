//! Symbol resolution: user/LLM text -> broker symbol (alias lookup, cleanup, suffix).

use std::collections::{BTreeMap, HashMap};

/// Lowercase and collapse whitespace so "Dow   Jones" matches the alias "dow jones".
pub fn alias_key(text: &str) -> String {
    text.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Uppercase and drop everything except ASCII letters/digits and `.`, `_`, `#`.
fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '#'))
        .collect::<String>()
        .to_ascii_uppercase()
}

#[derive(Debug, Clone, Default)]
pub struct SymbolResolver {
    aliases: HashMap<String, String>,
    suffix: String,
}

impl SymbolResolver {
    /// `aliases` keys are matched case-insensitively; values are broker symbols
    /// before the suffix.
    pub fn new(aliases: &BTreeMap<String, String>, suffix: &str) -> Self {
        Self {
            aliases: aliases
                .iter()
                .map(|(k, v)| (alias_key(k), clean(v)))
                .collect(),
            suffix: suffix.trim().to_owned(),
        }
    }

    /// Resolve user text to the broker symbol, or `None` if nothing usable is left.
    ///
    /// 1. trim + case-insensitive alias lookup (multi-word aliases work);
    /// 2. otherwise uppercase the text, dropping characters other than letters,
    ///    digits, `.`, `_`, `#`;
    /// 3. append the configured suffix unless the symbol already ends with it.
    pub fn resolve(&self, input: &str) -> Option<String> {
        let key = alias_key(input);
        if key.is_empty() {
            return None;
        }
        let base = self
            .aliases
            .get(&key)
            .cloned()
            .unwrap_or_else(|| clean(input));
        self.with_suffix(&base)
    }

    fn with_suffix(&self, base: &str) -> Option<String> {
        if base.is_empty() {
            return None;
        }
        if self.suffix.is_empty() {
            return Some(base.to_owned());
        }
        // `base` is ASCII uppercase, so comparing against the uppercased suffix is exact.
        let suffix_upper = self.suffix.to_ascii_uppercase();
        match base.strip_suffix(&suffix_upper) {
            Some("") => None,
            Some(stem) => Some(format!("{stem}{}", self.suffix)),
            None => Some(format!("{base}{}", self.suffix)),
        }
    }

    /// `(spoken name, broker symbol with suffix)` pairs, sorted, for the system prompt.
    pub fn alias_list(&self) -> Vec<(String, String)> {
        let mut v: Vec<_> = self
            .aliases
            .iter()
            .filter_map(|(k, base)| self.with_suffix(base).map(|s| (k.clone(), s)))
            .collect();
        v.sort();
        v
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
    fn alias_list_includes_suffix() {
        let list = resolver(".m").alias_list();
        assert!(list.contains(&("gold".to_owned(), "XAUUSD.m".to_owned())));
    }
}
