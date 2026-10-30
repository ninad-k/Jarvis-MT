//! Registry internals: TOML layer files, regex cache, resolve/reverse, atomic save.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use regex::Regex;
use serde::Deserialize;

use crate::{BrokerMap, InstrumentCatalog, Registry, Resolution, Source, SymbolsError};

#[derive(Deserialize, Default)]
struct LayerFile {
    #[serde(default)]
    brokers: BTreeMap<String, BrokerMap>,
}

fn io_err(path: &Path, e: &std::io::Error) -> SymbolsError {
    SymbolsError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

fn invalid(path: &Path, message: impl Into<String>) -> SymbolsError {
    SymbolsError::Invalid {
        path: path.display().to_string(),
        message: message.into(),
    }
}

// ---------------------------------------------------------------------------------
// Replacement spelling
// ---------------------------------------------------------------------------------

/// Rewrite a replacement template into the `regex` crate's unambiguous spelling:
/// `\1` → `${1}`, `$1` → `${1}` (so `$1m` is group 1 followed by `m`, not a group named
/// `1m`). `$$` and `${...}` are kept; `\\` becomes a literal backslash.
pub(crate) fn normalize_replacement(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 4);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match (c, next) {
            ('\\', Some(d)) if d.is_ascii_digit() => {
                let _ = write!(out, "${{{d}}}");
                i += 2;
            }
            ('\\', Some('\\')) => {
                out.push('\\');
                i += 2;
            }
            ('$', Some('$')) => {
                out.push_str("$$");
                i += 2;
            }
            ('$', Some('{')) => {
                let mut j = i;
                while j < chars.len() && chars[j] != '}' {
                    j += 1;
                }
                let end = (j + 1).min(chars.len());
                out.extend(&chars[i..end]);
                i = end;
            }
            ('$', Some(d)) if d.is_ascii_digit() => {
                let mut j = i + 1;
                let mut digits = String::new();
                while j < chars.len() && chars[j].is_ascii_digit() {
                    digits.push(chars[j]);
                    j += 1;
                }
                let _ = write!(out, "${{{digits}}}");
                i = j;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Highest numeric group referenced by a normalised replacement (`${N}`), if any.
fn max_group_ref(replacement: &str) -> Option<usize> {
    let mut max = None;
    let mut rest = replacement;
    while let Some(pos) = rest.find("${") {
        let after = &rest[pos + 2..];
        let Some(end) = after.find('}') else { break };
        if let Ok(n) = after[..end].parse::<usize>() {
            max = Some(max.map_or(n, |m: usize| m.max(n)));
        }
        rest = &after[end + 1..];
    }
    max
}

// ---------------------------------------------------------------------------------
// Regex cache (process-wide; patterns are tiny and few)
// ---------------------------------------------------------------------------------

fn regex_cache() -> &'static Mutex<HashMap<String, Option<Regex>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Regex>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Compile `pattern` as a **full match** (`^(?:pattern)$`); `None` when invalid.
pub(crate) fn compile(pattern: &str) -> Option<Regex> {
    let mut cache = regex_cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(hit) = cache.get(pattern) {
        return hit.clone();
    }
    if cache.len() >= 4096 {
        cache.clear();
    }
    let compiled = Regex::new(&format!("^(?:{pattern})$")).ok();
    cache.insert(pattern.to_owned(), compiled.clone());
    compiled
}

// ---------------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------------

fn validate_and_normalize(
    layer: &mut BTreeMap<String, BrokerMap>,
    path: &Path,
) -> Result<(), SymbolsError> {
    for (broker, map) in layer.iter_mut() {
        for (key, value) in &map.explicit {
            if key.trim().is_empty() || value.trim().is_empty() {
                return Err(invalid(
                    path,
                    format!(
                        "broker {broker:?}: explicit entries need a non-empty name and symbol ({key:?} = {value:?})"
                    ),
                ));
            }
        }
        for pattern in &mut map.patterns {
            let Some(re) = compile(&pattern.matches) else {
                return Err(invalid(
                    path,
                    format!("broker {broker:?}: invalid regex {:?}", pattern.matches),
                ));
            };
            pattern.replace = normalize_replacement(&pattern.replace);
            if let Some(n) = max_group_ref(&pattern.replace) {
                // `captures_len` counts the implicit whole-match group 0.
                if n >= re.captures_len() {
                    return Err(invalid(
                        path,
                        format!(
                            "broker {broker:?}: replacement {:?} uses group {n} but {:?} has {}",
                            pattern.replace,
                            pattern.matches,
                            re.captures_len() - 1
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn load_layer(
    path: &Path,
    required: bool,
) -> Result<BTreeMap<String, BrokerMap>, SymbolsError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => {
            return Ok(BTreeMap::new());
        }
        Err(e) => return Err(io_err(path, &e)),
    };
    let mut file: LayerFile = toml::from_str(&text).map_err(|e| invalid(path, e.to_string()))?;
    validate_and_normalize(&mut file.brokers, path)?;
    Ok(file.brokers)
}

pub(crate) fn load(
    curated: &Path,
    generated: &Path,
    user: &Path,
) -> Result<Registry, SymbolsError> {
    Ok(Registry {
        user: load_layer(user, false)?,
        curated: load_layer(curated, true)?,
        generated: load_layer(generated, false)?,
    })
}

// ---------------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------------

fn layer_list(reg: &Registry) -> [(&BTreeMap<String, BrokerMap>, Source); 3] {
    [
        (&reg.user, Source::User),
        (&reg.curated, Source::Curated),
        (&reg.generated, Source::Generated),
    ]
}

fn key_canonical(key: &str, catalog: &InstrumentCatalog) -> String {
    catalog
        .canonical_for(key)
        .unwrap_or_else(|| key.trim().to_ascii_uppercase())
}

/// Canonical name → (broker symbol, layer) from every layer's explicit entries; the
/// highest layer wins, and inside a layer a key that *is* the canonical name beats an
/// alias key (`XAUUSD` over `GOLD`), then alias keys in alphabetical order.
fn explicit_table<'a>(
    reg: &'a Registry,
    broker: &str,
    catalog: &InstrumentCatalog,
) -> HashMap<String, (&'a str, Source)> {
    let mut table: HashMap<String, (&'a str, Source)> = HashMap::new();
    for (layer, source) in layer_list(reg) {
        let Some(map) = layer.get(broker) else {
            continue;
        };
        let mut local: HashMap<String, (&'a str, bool)> = HashMap::new();
        for (key, value) in &map.explicit {
            let canon = key_canonical(key, catalog);
            let exact = key.trim().eq_ignore_ascii_case(&canon);
            match local.get(&canon) {
                Some((_, true)) => continue,
                Some((_, false)) if !exact => continue,
                _ => {
                    local.insert(canon, (value.as_str(), exact));
                }
            }
        }
        for (canon, (value, _)) in local {
            table.entry(canon).or_insert((value, source));
        }
    }
    table
}

/// A broker's patterns across all layers (user, then curated, then generated; in file
/// order), compiled once per call. Invalid regexes are skipped.
struct PatternSet {
    rules: Vec<(Regex, String)>,
}

impl PatternSet {
    fn for_broker(reg: &Registry, broker: &str) -> Self {
        let mut rules = Vec::new();
        for (layer, _) in layer_list(reg) {
            let Some(map) = layer.get(broker) else {
                continue;
            };
            for pattern in &map.patterns {
                if let Some(re) = compile(&pattern.matches) {
                    rules.push((re, normalize_replacement(&pattern.replace)));
                }
            }
        }
        PatternSet { rules }
    }

    /// Write the first full match's replacement for `canonical` into `out`.
    fn apply_into(&self, canonical: &str, out: &mut String) -> bool {
        for (re, replacement) in &self.rules {
            if let Some(caps) = re.captures(canonical) {
                out.clear();
                caps.expand(replacement, out);
                return true;
            }
        }
        false
    }
}

pub(crate) fn resolve(
    reg: &Registry,
    broker: &str,
    canonical: &str,
    catalog: &InstrumentCatalog,
) -> Resolution {
    let input = canonical.trim();
    let known = catalog.canonical_for(input);
    let canon = known.clone().unwrap_or_else(|| input.to_ascii_uppercase());
    let table = explicit_table(reg, broker, catalog);
    if let Some(&(value, source)) = table.get(&canon) {
        return Resolution {
            canonical: canon,
            broker_symbol: value.to_owned(),
            source,
        };
    }
    let mut out = String::new();
    if PatternSet::for_broker(reg, broker).apply_into(&canon, &mut out) {
        return Resolution {
            canonical: canon,
            broker_symbol: out,
            source: Source::Pattern,
        };
    }
    let passthrough = if known.is_some() {
        canon.clone()
    } else {
        input.to_owned()
    };
    Resolution {
        canonical: canon,
        broker_symbol: passthrough,
        source: Source::Passthrough,
    }
}

fn source_rank(source: Source) -> u8 {
    match source {
        Source::User => 0,
        Source::Curated => 1,
        Source::Generated => 2,
        Source::Pattern => 3,
        Source::Passthrough => 4,
    }
}

pub(crate) fn reverse(
    reg: &Registry,
    broker: &str,
    broker_symbol: &str,
    catalog: &InstrumentCatalog,
) -> Option<String> {
    let table = explicit_table(reg, broker, catalog);

    // Explicit entries (any layer) outrank patterns and passthrough, so they settle it.
    let explicit_hit = table
        .iter()
        .filter(|(_, (value, _))| *value == broker_symbol)
        .map(|(canon, (_, source))| (source_rank(*source), canon.as_str()))
        .min();
    if let Some((_, canon)) = explicit_hit {
        return Some(canon.to_owned());
    }

    // Otherwise every catalog instrument without an explicit entry resolves through the
    // patterns, else to itself.
    let patterns = PatternSet::for_broker(reg, broker);
    let mut out = String::new();
    let mut by_pattern: Option<&str> = None;
    let mut by_passthrough: Option<&str> = None;
    // Alphabetical so ties break the same way every time.
    let mut names: Vec<&str> = catalog
        .instruments()
        .iter()
        .map(|i| i.canonical.as_str())
        .filter(|c| !table.contains_key(*c))
        .collect();
    names.sort_unstable();
    for canon in names {
        if patterns.apply_into(canon, &mut out) {
            if out == broker_symbol && by_pattern.is_none() {
                by_pattern = Some(canon);
            }
        } else if canon == broker_symbol && by_passthrough.is_none() {
            by_passthrough = Some(canon);
        }
    }
    by_pattern.or(by_passthrough).map(str::to_owned)
}

pub(crate) fn set_user_override(
    reg: &mut Registry,
    broker: &str,
    canonical: &str,
    broker_symbol: Option<&str>,
) {
    let key = canonical.trim().to_ascii_uppercase();
    if key.is_empty() {
        return;
    }
    match broker_symbol.map(str::trim).filter(|s| !s.is_empty()) {
        Some(symbol) => {
            reg.user
                .entry(broker.to_owned())
                .or_default()
                .explicit
                .insert(key, symbol.to_owned());
        }
        None => {
            if let Some(map) = reg.user.get_mut(broker) {
                map.explicit.remove(&key);
                if map.explicit.is_empty() && map.patterns.is_empty() && map.note.is_none() {
                    reg.user.remove(broker);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------
// Saving
// ---------------------------------------------------------------------------------

fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn toml_key(k: &str) -> String {
    if !k.is_empty()
        && k.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        k.to_owned()
    } else {
        toml_string(k)
    }
}

pub(crate) fn render_layer(layer: &BTreeMap<String, BrokerMap>, header: &str) -> String {
    let mut out = String::new();
    out.push_str(
        "# Machine-written by Arc MT. Edits made while it is running may be overwritten.\n",
    );
    for line in header.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            out.push_str("#\n");
        } else if line.starts_with('#') {
            out.push_str(line);
            out.push('\n');
        } else {
            out.push_str("# ");
            out.push_str(line);
            out.push('\n');
        }
    }
    for (broker, map) in layer {
        let name = toml_string(broker);
        let _ = writeln!(out, "\n[brokers.{name}]");
        if let Some(note) = &map.note {
            let _ = writeln!(out, "note = {}", toml_string(note));
        }
        if !map.patterns.is_empty() {
            out.push_str("patterns = [\n");
            for p in &map.patterns {
                let _ = writeln!(
                    out,
                    "    {{ match = {}, replace = {} }},",
                    toml_string(&p.matches),
                    toml_string(&normalize_replacement(&p.replace))
                );
            }
            out.push_str("]\n");
        }
        if !map.explicit.is_empty() {
            let _ = writeln!(out, "\n[brokers.{name}.explicit]");
            for (k, v) in &map.explicit {
                let _ = writeln!(out, "{} = {}", toml_key(k), toml_string(v));
            }
        }
    }
    out
}

fn temp_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "layer".to_owned());
    let tmp = format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    path.with_file_name(tmp)
}

pub(crate) fn save_layer(
    layer: &BTreeMap<String, BrokerMap>,
    path: &Path,
    header: &str,
) -> Result<(), SymbolsError> {
    let text = render_layer(layer, header);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| io_err(parent, &e))?;
    }
    let tmp = temp_path(path);
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_err(&tmp, &e));
    }
    // `rename` replaces an existing target on Windows too; a reader holding the old file
    // open can make it fail briefly, so retry a few times.
    let mut last = None;
    for attempt in 0..6 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(15 * (attempt + 1)));
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    Err(io_err(path, &last.expect("at least one attempt")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_spellings() {
        let cases = [
            (r"\1_", "${1}_"),
            ("${1}_", "${1}_"),
            ("$1_", "${1}_"),
            ("$1m", "${1}m"),
            (r"\1\2", "${1}${2}"),
            ("#$1", "#${1}"),
            ("$$1", "$$1"),
            ("plain", "plain"),
            (r"a\\b", r"a\b"),
            ("${name}x", "${name}x"),
            ("$12", "${12}"),
        ];
        for (input, want) in cases {
            assert_eq!(normalize_replacement(input), want, "{input}");
        }
    }

    #[test]
    fn group_refs() {
        assert_eq!(max_group_ref("${1}_"), Some(1));
        assert_eq!(max_group_ref("${2}${1}"), Some(2));
        assert_eq!(max_group_ref("plain"), None);
        assert_eq!(max_group_ref("${name}"), None);
    }

    #[test]
    fn toml_escaping() {
        assert_eq!(toml_string("a\"b\\c\nd"), r#""a\"b\\c\nd""#);
        assert_eq!(toml_key("US100"), "US100");
        assert_eq!(toml_key("A.B"), "\"A.B\"");
    }
}
