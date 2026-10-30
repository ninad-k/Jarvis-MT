//! Per-user settings and the stored limit overrides (docs/api.md, "Personas and user
//! settings").
//!
//! Settings are preferences only (persona, model, voice behaviour, watchlist, spoken aliases).
//! Nothing here can loosen a trading limit, change a confirmation or reach the bridge.

use std::collections::BTreeMap;

use anyhow::Context;
use chrono::{SecondsFormat, Utc};
use jarvis_persona::PersonaRegistry;
use serde::{Deserialize, Deserializer, Serialize};
use sqlx::{Row, SqlitePool};

use crate::limit_overrides::PartialLimits;
use crate::symbols::{SymbolResolver, alias_key};

/// The settings key used when login is off (one shared local profile).
pub const LOCAL_USER: &str = "local";

/// Most symbols in the watchlist.
pub const MAX_WATCHLIST: usize = 30;
/// Most user aliases.
pub const MAX_ALIASES: usize = 100;
/// Longest alias (spoken name) and watchlist entry, in characters.
const MAX_NAME_CHARS: usize = 64;

/// How the microphone is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    /// Press and hold to talk.
    #[default]
    Hold,
    /// Tap to start and stop.
    Toggle,
    /// Hands-free: only speech that starts with a persona's wake phrase is acted on.
    WakeWord,
}

/// Speech-to-text language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VoiceLanguage {
    #[default]
    En,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    pub read_aloud: bool,
    pub input_mode: InputMode,
    pub language: VoiceLanguage,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            read_aloud: true,
            input_mode: InputMode::Hold,
            language: VoiceLanguage::En,
        }
    }
}

/// The settings object of `GET /api/settings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserSettings {
    pub persona: String,
    /// Provider name, or `None` for the server default.
    pub llm_provider: Option<String>,
    pub voice: VoiceSettings,
    pub watchlist: Vec<String>,
    /// Lower-cased spoken name to canonical instrument name.
    pub aliases: BTreeMap<String, String>,
}

impl UserSettings {
    /// What a user who never touched the settings gets.
    pub fn defaults(default_persona: &str, watchlist: &[String]) -> Self {
        Self {
            persona: default_persona.to_owned(),
            llm_provider: None,
            voice: VoiceSettings::default(),
            watchlist: watchlist.to_vec(),
            aliases: BTreeMap::new(),
        }
    }
}

/// Fields that may be sent to `PUT /api/settings`. Anything else is a 400.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsPatch {
    pub persona: Option<String>,
    /// `Some(None)` (JSON `null`) resets to the server default.
    #[serde(default, deserialize_with = "present")]
    pub llm_provider: Option<Option<String>>,
    pub voice: Option<VoicePatch>,
    pub watchlist: Option<Vec<String>>,
    pub aliases: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoicePatch {
    pub read_aloud: Option<bool>,
    pub input_mode: Option<InputMode>,
    pub language: Option<VoiceLanguage>,
}

/// Tells "field absent" from "field is null".
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// A rejected update (400).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SettingsError(pub String);

fn bad(message: impl Into<String>) -> SettingsError {
    SettingsError(message.into())
}

/// What validation checks a patch against.
pub struct Rules<'a> {
    pub personas: &'a PersonaRegistry,
    /// Names of the configured LLM providers.
    pub providers: Vec<&'a str>,
    /// The configured aliases and suffix (without any user's aliases).
    pub resolver: &'a SymbolResolver,
}

/// Apply `patch` to `current`, validating everything. `current` is returned unchanged inside
/// the error case; nothing is stored here.
pub fn apply_patch(
    current: UserSettings,
    patch: SettingsPatch,
    rules: &Rules<'_>,
) -> Result<UserSettings, SettingsError> {
    let mut next = current;

    if let Some(id) = patch.persona {
        if rules.personas.get(&id).is_err() {
            let known: Vec<&str> = rules
                .personas
                .list()
                .iter()
                .map(|p| p.id.as_str())
                .collect();
            return Err(bad(format!(
                "unknown persona '{id}' (available: {})",
                known.join(", ")
            )));
        }
        next.persona = id;
    }

    if let Some(provider) = patch.llm_provider {
        next.llm_provider = match provider {
            None => None,
            Some(name) => {
                let name = name.trim().to_owned();
                if !rules.providers.contains(&name.as_str()) {
                    return Err(bad(format!(
                        "unknown provider '{name}' (available: {})",
                        rules.providers.join(", ")
                    )));
                }
                Some(name)
            }
        };
    }

    if let Some(voice) = patch.voice {
        if let Some(v) = voice.read_aloud {
            next.voice.read_aloud = v;
        }
        if let Some(v) = voice.input_mode {
            next.voice.input_mode = v;
        }
        if let Some(v) = voice.language {
            next.voice.language = v;
        }
    }

    // Aliases before the watchlist: the watchlist is resolved with the new aliases.
    if let Some(aliases) = patch.aliases {
        next.aliases = validate_aliases(aliases)?;
    }

    if let Some(list) = patch.watchlist {
        let resolver = rules.resolver.with_user_aliases(&next.aliases);
        next.watchlist = validate_watchlist(&list, &resolver)?;
    }
    Ok(next)
}

fn validate_aliases(
    aliases: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, SettingsError> {
    if aliases.len() > MAX_ALIASES {
        return Err(bad(format!("at most {MAX_ALIASES} aliases are allowed")));
    }
    let mut out = BTreeMap::new();
    for (name, symbol) in aliases {
        let key = alias_key(&name);
        if key.is_empty()
            || key.chars().count() > MAX_NAME_CHARS
            || key.chars().any(char::is_control)
        {
            return Err(bad(format!(
                "alias '{name}' must be 1 to {MAX_NAME_CHARS} characters"
            )));
        }
        let symbol = symbol.trim();
        let valid = (1..=32).contains(&symbol.len())
            && symbol
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'#'));
        if !valid {
            return Err(bad(format!(
                "alias '{name}' maps to '{symbol}', which is not a symbol name (use 1 to 32 \
                 letters, digits, '.', '_' or '#')"
            )));
        }
        out.insert(key, symbol.to_owned());
    }
    Ok(out)
}

fn validate_watchlist(
    list: &[String],
    resolver: &SymbolResolver,
) -> Result<Vec<String>, SettingsError> {
    if list.len() > MAX_WATCHLIST {
        return Err(bad(format!(
            "the watchlist can hold at most {MAX_WATCHLIST} symbols"
        )));
    }
    let mut out: Vec<String> = Vec::with_capacity(list.len());
    for entry in list {
        let entry = entry.trim();
        if entry.is_empty() || entry.chars().count() > MAX_NAME_CHARS {
            return Err(bad(format!(
                "watchlist entries must be 1 to {MAX_NAME_CHARS} characters, got '{entry}'"
            )));
        }
        // The watchlist holds canonical names; the broker's own spelling is resolved when
        // the prices are read.
        let symbol = resolver
            .canonical(entry)
            .ok_or_else(|| bad(format!("'{entry}' is not a usable symbol name")))?;
        if !out.contains(&symbol) {
            out.push(symbol);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// SQLite storage for `user_settings` and `limit_overrides`. Cheap to clone.
#[derive(Clone, Debug)]
pub struct SettingsStore {
    pool: SqlitePool,
    /// Held across a read-modify-write of settings so two updates do not overwrite each other.
    write_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
}

impl SettingsStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            write_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Take the write lock for a read-modify-write.
    pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.write_lock.lock().await
    }

    /// The stored settings of `user`; `None` when the user never saved any.
    pub async fn get(&self, user: &str) -> anyhow::Result<Option<UserSettings>> {
        let row = sqlx::query("SELECT settings FROM user_settings WHERE username = ?1")
            .bind(user)
            .fetch_optional(&self.pool)
            .await
            .context("reading user settings")?;
        row.map(|r| {
            let text: String = r.try_get("settings")?;
            serde_json::from_str(&text).context("stored user settings are not valid")
        })
        .transpose()
    }

    pub async fn put(&self, user: &str, settings: &UserSettings) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO user_settings (username, settings, updated_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT(username) DO UPDATE SET settings = excluded.settings, \
             updated_at = excluded.updated_at",
        )
        .bind(user)
        .bind(serde_json::to_string(settings)?)
        .bind(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true))
        .execute(&self.pool)
        .await
        .context("saving user settings")?;
        Ok(())
    }

    /// The stored limit overrides, if any.
    pub async fn load_limit_overrides(&self) -> anyhow::Result<Option<PartialLimits>> {
        let row = sqlx::query("SELECT overrides FROM limit_overrides WHERE id = 1")
            .fetch_optional(&self.pool)
            .await
            .context("reading the limit overrides")?;
        row.map(|r| {
            let text: String = r.try_get("overrides")?;
            serde_json::from_str(&text).context("stored limit overrides are not valid")
        })
        .transpose()
    }

    pub async fn save_limit_overrides(
        &self,
        overrides: &PartialLimits,
        by: &str,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO limit_overrides (id, overrides, updated_at, updated_by) \
             VALUES (1, ?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET overrides = excluded.overrides, \
             updated_at = excluded.updated_at, updated_by = excluded.updated_by",
        )
        .bind(serde_json::to_string(overrides)?)
        .bind(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true))
        .bind(by)
        .execute(&self.pool)
        .await
        .context("saving the limit overrides")?;
        Ok(())
    }

    pub async fn clear_limit_overrides(&self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM limit_overrides WHERE id = 1")
            .execute(&self.pool)
            .await
            .context("removing the limit overrides")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_persona::{PersonaPack, Theme};
    use serde_json::json;

    fn pack(id: &str, wake: &str) -> PersonaPack {
        PersonaPack {
            id: id.to_owned(),
            name: id.to_owned(),
            description: String::new(),
            system_prompt: "x".to_owned(),
            voice_model: "v.onnx".into(),
            wake_word: wake.to_owned(),
            theme: Theme {
                accent: "#000000".into(),
                accent_soft: "#000000".into(),
                orb_core: "#000000".into(),
                orb_glow: "#000000".into(),
            },
        }
    }

    fn personas() -> PersonaRegistry {
        PersonaRegistry::from_packs(
            vec![pack("jarvis", "hey jarvis"), pack("friday", "hey friday")],
            "jarvis",
        )
        .unwrap()
    }

    fn base_resolver() -> SymbolResolver {
        let aliases = [("gold", "XAUUSD")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        SymbolResolver::new(&aliases, ".m")
    }

    fn apply(
        current: UserSettings,
        patch: serde_json::Value,
    ) -> Result<UserSettings, SettingsError> {
        let personas = personas();
        let resolver = base_resolver();
        let patch: SettingsPatch = serde_json::from_value(patch).map_err(|e| bad(e.to_string()))?;
        apply_patch(
            current,
            patch,
            &Rules {
                personas: &personas,
                providers: vec!["local", "claude"],
                resolver: &resolver,
            },
        )
    }

    fn defaults() -> UserSettings {
        UserSettings::defaults("jarvis", &["XAUUSD".to_owned()])
    }

    #[test]
    fn defaults_match_the_documented_shape() {
        let v = serde_json::to_value(defaults()).unwrap();
        assert_eq!(
            v,
            json!({
                "persona": "jarvis",
                "llm_provider": null,
                "voice": { "read_aloud": true, "input_mode": "hold", "language": "en" },
                "watchlist": ["XAUUSD"],
                "aliases": {}
            })
        );
    }

    #[test]
    fn a_partial_update_changes_only_what_was_sent() {
        let s = apply(defaults(), json!({ "persona": "friday" })).unwrap();
        assert_eq!(s.persona, "friday");
        assert_eq!(s.voice, VoiceSettings::default());
        assert_eq!(s.watchlist, ["XAUUSD"]);

        let s = apply(s, json!({ "voice": { "input_mode": "wake_word" } })).unwrap();
        assert_eq!(s.voice.input_mode, InputMode::WakeWord);
        assert!(s.voice.read_aloud);
        assert_eq!(s.persona, "friday");

        let s = apply(
            s,
            json!({ "voice": { "read_aloud": false, "language": "auto" } }),
        )
        .unwrap();
        assert!(!s.voice.read_aloud);
        assert_eq!(s.voice.language, VoiceLanguage::Auto);
        assert_eq!(s.voice.input_mode, InputMode::WakeWord);
    }

    #[test]
    fn unknown_fields_and_values_are_refused() {
        for patch in [
            json!({ "theme": "dark" }),
            json!({ "voice": { "volume": 3 } }),
            json!({ "voice": { "input_mode": "shout" } }),
            json!({ "voice": { "language": "fr" } }),
            json!({ "persona": "nobody" }),
            json!({ "persona": 3 }),
            json!({ "llm_provider": "gpt-nine" }),
            json!({ "watchlist": "XAUUSD" }),
        ] {
            assert!(apply(defaults(), patch.clone()).is_err(), "{patch}");
        }
    }

    #[test]
    fn provider_can_be_set_and_reset() {
        let s = apply(defaults(), json!({ "llm_provider": "claude" })).unwrap();
        assert_eq!(s.llm_provider.as_deref(), Some("claude"));
        let s = apply(s, json!({ "persona": "friday" })).unwrap();
        assert_eq!(s.llm_provider.as_deref(), Some("claude"), "absent keeps it");
        let s = apply(s, json!({ "llm_provider": null })).unwrap();
        assert_eq!(s.llm_provider, None);
    }

    #[test]
    fn watchlist_is_resolved_like_chat_input_and_capped() {
        let s = apply(
            defaults(),
            json!({ "watchlist": ["gold", "eur/usd", " XAUUSD.m ", "btc usd"] }),
        )
        .unwrap();
        // Canonical names: the broker's spelling is applied when prices are read.
        assert_eq!(s.watchlist, ["XAUUSD", "EURUSD", "BTCUSD"]);

        let many: Vec<String> = (0..30).map(|i| format!("SYM{i}")).collect();
        assert!(apply(defaults(), json!({ "watchlist": many })).is_ok());
        let too_many: Vec<String> = (0..31).map(|i| format!("SYM{i}")).collect();
        assert!(apply(defaults(), json!({ "watchlist": too_many })).is_err());
        assert!(apply(defaults(), json!({ "watchlist": [""] })).is_err());
        assert!(apply(defaults(), json!({ "watchlist": ["///"] })).is_err());
        assert!(apply(defaults(), json!({ "watchlist": ["x".repeat(65)] })).is_err());
    }

    #[test]
    fn the_watchlist_sees_the_aliases_of_the_same_update() {
        let s = apply(
            defaults(),
            json!({ "aliases": { "Yellow  Metal": "XAUUSD" }, "watchlist": ["yellow metal"] }),
        )
        .unwrap();
        assert_eq!(
            s.aliases.get("yellow metal").map(String::as_str),
            Some("XAUUSD")
        );
        assert_eq!(s.watchlist, ["XAUUSD"]);
    }

    #[test]
    fn aliases_are_validated_and_replace_the_old_set() {
        let s = apply(
            defaults(),
            json!({ "aliases": { "oil": "USOIL", "yen": "USDJPY" } }),
        )
        .unwrap();
        assert_eq!(s.aliases.len(), 2);
        let s = apply(s, json!({ "aliases": { "oil": "XTIUSD" } })).unwrap();
        assert_eq!(s.aliases.len(), 1);
        assert_eq!(s.aliases["oil"], "XTIUSD");
        let s = apply(s, json!({ "aliases": {} })).unwrap();
        assert!(s.aliases.is_empty());

        for bad_alias in [
            json!({ "": "XAUUSD" }),
            json!({ "   ": "XAUUSD" }),
            json!({ "oil": "" }),
            json!({ "oil": "US OIL" }),
            json!({ "oil": "US/OIL" }),
            json!({ "oil": "A".repeat(33) }),
            json!({ "a".repeat(65): "XAUUSD" }),
            json!({ "oil": 3 }),
        ] {
            assert!(
                apply(defaults(), json!({ "aliases": bad_alias.clone() })).is_err(),
                "{bad_alias}"
            );
        }
        let max: serde_json::Map<String, serde_json::Value> = (0..100)
            .map(|i| (format!("alias {i}"), json!("XAUUSD")))
            .collect();
        assert!(apply(defaults(), json!({ "aliases": max.clone() })).is_ok());
        let mut over = max;
        over.insert("one more".into(), json!("XAUUSD"));
        assert!(apply(defaults(), json!({ "aliases": over })).is_err());
    }

    #[test]
    fn a_failed_update_changes_nothing() {
        let before = defaults();
        let err = apply(
            before.clone(),
            json!({ "persona": "friday", "llm_provider": "nope" }),
        );
        assert!(err.is_err());
        // `apply` consumes its input; the caller keeps its own copy and only stores on Ok.
        assert_eq!(before.persona, "jarvis");
    }
}
