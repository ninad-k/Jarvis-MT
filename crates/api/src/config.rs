//! Configuration: non-secret settings from `config/jarvis.toml`, secrets from the
//! environment (`.env` is loaded first).

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use anyhow::{Context, anyhow, bail};
use jarvis_llm::ProviderConfig;
use serde::Deserialize;

use crate::symbols::alias_key;

/// Env var that overrides the config file location.
pub const CONFIG_ENV: &str = "JARVIS_CONFIG";
/// Default config file, relative to the working directory.
pub const DEFAULT_CONFIG_PATH: &str = "config/jarvis.toml";

/// Operating mode. LIVE needs an explicit setting; the default is DEMO (rule 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Demo,
    Live,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Demo => "demo",
            Mode::Live => "live",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub bridge: BridgeSection,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub symbols: SymbolsConfig,
    pub llm: LlmConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
        }
    }
}

fn default_bind() -> String {
    "127.0.0.1:8787".to_owned()
}

#[derive(Debug, Clone, Deserialize)]
pub struct BridgeSection {
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

impl Default for BridgeSection {
    fn default() -> Self {
        Self {
            endpoint: default_endpoint(),
            timeout_ms: default_timeout_ms(),
        }
    }
}

fn default_endpoint() -> String {
    "tcp://127.0.0.1:5555".to_owned()
}
fn default_timeout_ms() -> u64 {
    3000
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_url")]
    pub url: String,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: default_db_url(),
        }
    }
}

fn default_db_url() -> String {
    "sqlite://data/jarvis.db".to_owned()
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SymbolsConfig {
    /// Appended to every resolved symbol (`.m`, `#`, ...).
    #[serde(default)]
    pub suffix: String,
    /// Symbols shown in the UI watchlist (broker names without suffix).
    #[serde(default)]
    pub watchlist: Vec<String>,
    /// Spoken/typed name -> broker symbol. Keys are lowercased on load.
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LlmConfig {
    pub default: String,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

impl Config {
    /// Path of the config file: `JARVIS_CONFIG` or `config/jarvis.toml`.
    pub fn path() -> PathBuf {
        std::env::var_os(CONFIG_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH))
    }

    /// Load `.env` (if present), then the config file.
    pub fn load() -> anyhow::Result<Self> {
        // A missing .env is normal; a broken one shows up later as missing API keys.
        let _ = dotenvy::dotenv();
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
                "config file not found at {}. Copy config/jarvis.example.toml to \
                 config/jarvis.toml (or point {CONFIG_ENV} at your file) and edit it.",
                path.display()
            ),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Self::from_toml_str(&text).with_context(|| format!("invalid config {}", path.display()))
    }

    /// Parse, normalise and validate.
    pub fn from_toml_str(text: &str) -> anyhow::Result<Self> {
        let mut config: Config = toml::from_str(text).context("parsing TOML")?;
        config.normalise();
        config.validate()?;
        Ok(config)
    }

    fn normalise(&mut self) {
        let aliases = std::mem::take(&mut self.symbols.aliases);
        self.symbols.aliases = aliases
            .into_iter()
            .map(|(k, v)| (alias_key(&k), v))
            .collect();
        self.symbols.suffix = self.symbols.suffix.trim().to_owned();
        self.symbols.watchlist.retain(|s| !s.trim().is_empty());
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        // `mode` is checked by serde (demo | live).
        validate_loopback_endpoint(&self.bridge.endpoint)?;
        if self.bridge.timeout_ms == 0 {
            bail!("bridge.timeout_ms must be greater than 0");
        }
        self.bind_addr()?;
        if self.llm.providers.is_empty() {
            bail!("at least one [[llm.providers]] entry is required");
        }
        let mut seen = std::collections::BTreeSet::new();
        for p in &self.llm.providers {
            if p.name.trim().is_empty() {
                bail!("an [[llm.providers]] entry has an empty name");
            }
            if !seen.insert(p.name.as_str()) {
                bail!("duplicate llm provider name '{}'", p.name);
            }
        }
        if !self
            .llm
            .providers
            .iter()
            .any(|p| p.name == self.llm.default)
        {
            bail!(
                "llm.default = '{}' does not match any [[llm.providers]] name (have: {})",
                self.llm.default,
                self.llm
                    .providers
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Ok(())
    }

    /// Parsed `server.bind`. Sprint 1 has no auth, so only loopback is allowed.
    pub fn bind_addr(&self) -> anyhow::Result<SocketAddr> {
        let addr: SocketAddr = self.server.bind.parse().map_err(|e| {
            anyhow!(
                "server.bind '{}' is not a valid host:port ({e})",
                self.server.bind
            )
        })?;
        if !addr.ip().is_loopback() {
            bail!(
                "refusing to bind {addr}: the API has no authentication yet (arrives in Sprint 4), \
                 so server.bind must be a loopback address such as 127.0.0.1:8787"
            );
        }
        Ok(addr)
    }
}

/// `tcp://127.0.0.1:5555` style endpoint on a loopback host.
pub fn validate_loopback_endpoint(endpoint: &str) -> anyhow::Result<()> {
    let rest = endpoint.strip_prefix("tcp://").ok_or_else(|| {
        anyhow!("bridge.endpoint '{endpoint}' must look like tcp://127.0.0.1:5555")
    })?;
    let (host, port) = rest
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("bridge.endpoint '{endpoint}' has no port"))?;
    port.parse::<u16>()
        .map_err(|_| anyhow!("bridge.endpoint '{endpoint}' has an invalid port '{port}'"))?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    if !loopback {
        bail!(
            "bridge.endpoint '{endpoint}' must be a loopback address (127.0.0.1, ::1 or localhost)"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("../../../config/jarvis.example.toml");

    fn minimal(extra: &str) -> String {
        format!(
            r#"
            {extra}
            [llm]
            default = "local"
            [[llm.providers]]
            name = "local"
            kind = "ollama"
            model = "m"
            "#
        )
    }

    #[test]
    fn example_config_is_valid() {
        let c = Config::from_toml_str(EXAMPLE).unwrap();
        assert_eq!(c.mode, Mode::Demo);
        assert_eq!(c.llm.providers.len(), 3);
        assert_eq!(
            c.symbols.aliases.get("dow jones").map(String::as_str),
            Some("US30")
        );
        assert_eq!(c.symbols.watchlist.len(), 6);
    }

    #[test]
    fn mode_defaults_to_demo_and_rejects_unknown() {
        assert_eq!(
            Config::from_toml_str(&minimal("")).unwrap().mode,
            Mode::Demo
        );
        assert_eq!(
            Config::from_toml_str(&minimal("mode = \"live\""))
                .unwrap()
                .mode,
            Mode::Live
        );
        assert!(Config::from_toml_str(&minimal("mode = \"paper\"")).is_err());
    }

    #[test]
    fn aliases_are_lowercased() {
        let text = r#"
            [symbols.aliases]
            "Dow  Jones" = "US30"
            GOLD = "XAUUSD"
            [llm]
            default = "local"
            [[llm.providers]]
            name = "local"
            kind = "ollama"
            model = "m"
        "#;
        let c = Config::from_toml_str(text).unwrap();
        assert!(c.symbols.aliases.contains_key("dow jones"));
        assert!(c.symbols.aliases.contains_key("gold"));
    }

    #[test]
    fn bridge_endpoint_must_be_loopback() {
        let with = |endpoint: &str| {
            Config::from_toml_str(&format!(
                "[bridge]\nendpoint = \"{endpoint}\"\n[llm]\ndefault = \"l\"\n\
                 [[llm.providers]]\nname = \"l\"\nkind = \"ollama\"\nmodel = \"m\""
            ))
        };
        let err = with("tcp://192.168.1.5:5555").unwrap_err().to_string();
        assert!(err.contains("loopback"), "{err}");
        assert!(with("tcp://localhost:5555").is_ok());
        assert!(with("tcp://[::1]:5555").is_ok());
        assert!(with("tcp://127.0.0.1:5555").is_ok());
        assert!(with("http://127.0.0.1:5555").is_err());
    }

    #[test]
    fn needs_a_provider_and_a_valid_default() {
        let none = "[llm]\ndefault = \"x\"";
        assert!(
            Config::from_toml_str(none)
                .unwrap_err()
                .to_string()
                .contains("at least one")
        );
        let wrong = minimal("").replace("default = \"local\"", "default = \"nope\"");
        assert!(
            Config::from_toml_str(&wrong)
                .unwrap_err()
                .to_string()
                .contains("llm.default")
        );
    }

    #[test]
    fn bind_must_be_loopback() {
        let with = |bind: &str| {
            Config::from_toml_str(&format!(
                "[server]\nbind = \"{bind}\"\n[llm]\ndefault = \"l\"\n\
                 [[llm.providers]]\nname = \"l\"\nkind = \"ollama\"\nmodel = \"m\""
            ))
        };
        let err = with("0.0.0.0:8787").unwrap_err();
        assert!(err.to_string().contains("loopback"), "{err}");
        assert!(with("127.0.0.1:0").is_ok());
    }

    #[test]
    fn missing_file_explains_how_to_fix() {
        let err = Config::load_from(std::path::Path::new("definitely/not/here.toml")).unwrap_err();
        assert!(err.to_string().contains("jarvis.example.toml"), "{err}");
    }
}
