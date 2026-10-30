//! Configuration: non-secret settings from `config/jarvis.toml`, secrets from the
//! environment (`.env` is loaded first).

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use anyhow::{Context, anyhow, bail};
use jarvis_bridge_client::BridgeKey;
use jarvis_bridge_client::auth::KEY_ENV;
use jarvis_gate::Limits;
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
    /// `[limits]`. When the section is absent the limits fail closed (see
    /// [`Config::effective_limits`]).
    #[serde(default)]
    pub limits: Option<Limits>,
    #[serde(default)]
    pub trading: TradingConfig,
    /// `[auth]`. Login is on unless the section says otherwise.
    #[serde(default)]
    pub auth: AuthConfig,
    /// `[voice]`. Absent means voice is off.
    #[serde(default)]
    pub voice: Option<VoiceSection>,
}

/// `[auth]`: password + TOTP login with a JWT session cookie (ADR-0011).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AuthConfig {
    /// Require a session on every `/api/*` route except login and health. The secret comes
    /// from the environment variable `JARVIS_JWT_SECRET` (at least 32 bytes).
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Session lifetime in hours (1 to 168).
    #[serde(default = "default_session_hours")]
    pub session_hours: u64,
    /// Force the `Secure` cookie attribute on or off. Unset: `Secure` whenever the server
    /// itself serves TLS. Set it to `true` behind a TLS-terminating reverse proxy that
    /// forwards to a plain-HTTP loopback port.
    #[serde(default)]
    pub cookie_secure: Option<bool>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            session_hours: default_session_hours(),
            cookie_secure: None,
        }
    }
}

fn yes() -> bool {
    true
}
fn default_session_hours() -> u64 {
    12
}

/// `[voice]`. Every path is optional in the file so that `enabled = false` alone is valid;
/// [`VoiceSection::to_voice_config`] checks them when voice is on.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct VoiceSection {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub whisper_model: Option<PathBuf>,
    #[serde(default = "default_voice_language")]
    pub language: String,
    #[serde(default = "default_voice_threads")]
    pub threads: u32,
    #[serde(default)]
    pub piper_exe: Option<PathBuf>,
    #[serde(default)]
    pub piper_voice: Option<PathBuf>,
    #[serde(default = "default_voice_max_seconds")]
    pub max_seconds: f32,
}

fn default_voice_language() -> String {
    "en".to_owned()
}
fn default_voice_threads() -> u32 {
    4
}
fn default_voice_max_seconds() -> f32 {
    30.0
}

impl VoiceSection {
    /// The voice crate's config, or `None` when voice is disabled.
    pub fn to_voice_config(&self) -> anyhow::Result<Option<jarvis_voice::VoiceConfig>> {
        if !self.enabled {
            return Ok(None);
        }
        let need = |v: &Option<PathBuf>, key: &str| {
            v.clone().ok_or_else(|| {
                anyhow!("[voice] enabled = true needs '{key}' (see config/jarvis.example.toml)")
            })
        };
        if !(self.max_seconds.is_finite() && self.max_seconds > 0.0 && self.max_seconds <= 120.0) {
            bail!("voice.max_seconds must be between 0 and 120");
        }
        Ok(Some(jarvis_voice::VoiceConfig {
            enabled: true,
            whisper_model: need(&self.whisper_model, "whisper_model")?,
            language: self.language.clone(),
            threads: self.threads,
            piper_exe: need(&self.piper_exe, "piper_exe")?,
            piper_voice: need(&self.piper_voice, "piper_voice")?,
            max_seconds: self.max_seconds,
        }))
    }
}

/// `[trading]`: confirmation window and execution settings.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TradingConfig {
    /// How long a confirmation token is valid.
    #[serde(default = "default_token_ttl_secs")]
    pub token_ttl_secs: u64,
    /// Voice flow: the window starts after the read-back, never later than this after issue.
    #[serde(default = "default_voice_cap_secs")]
    pub voice_token_cap_secs: u64,
    /// Max slippage for market orders and closes, in points.
    #[serde(default = "default_deviation_points")]
    pub deviation_points: u32,
    /// Start with trading disabled.
    #[serde(default)]
    pub kill_switch_engaged: bool,
}

impl Default for TradingConfig {
    fn default() -> Self {
        Self {
            token_ttl_secs: default_token_ttl_secs(),
            voice_token_cap_secs: default_voice_cap_secs(),
            deviation_points: default_deviation_points(),
            kill_switch_engaged: false,
        }
    }
}

fn default_token_ttl_secs() -> u64 {
    15
}
fn default_voice_cap_secs() -> u64 {
    30
}
fn default_deviation_points() -> u32 {
    20
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
    /// Built web UI served at `/` (with `index.html` fallback for client-side routes).
    #[serde(default)]
    pub static_dir: Option<PathBuf>,
    /// PEM certificate chain. Set together with `tls_key` to serve HTTPS/WSS.
    #[serde(default)]
    pub tls_cert: Option<PathBuf>,
    /// PEM private key.
    #[serde(default)]
    pub tls_key: Option<PathBuf>,
    /// Extra browser origins allowed to call the API with the session cookie (CORS and the
    /// WebSocket `Origin` check), for example `["http://localhost:5173"]` for a Vite dev
    /// server that talks to the API directly. Empty by default: the SPA is served from the
    /// API's own origin, and the Vite proxy (`pnpm dev`) is same-origin too. Anything listed
    /// here can drive the whole API, so list only origins you control.
    #[serde(default)]
    pub dev_origins: Vec<String>,
    /// Seconds a client has to send the headers of a request (NET-1). It also ends a keep-alive
    /// connection that has been idle for this long, and bounds the TLS handshake. 1 to 120.
    #[serde(default = "default_header_read_timeout_secs")]
    pub header_read_timeout_secs: u64,
    /// Seconds a request may take to be answered, body upload included (NET-1), for every
    /// route except the ones below and the WebSocket (which has no limit). 1 to 600.
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    /// The same for the routes that wait for a language model, speech or a batch of trades:
    /// `/api/chat`, `/api/voice/*` and `/api/confirm/*`. At least `request_timeout_secs`, at
    /// most 3600. Keep it above the `timeout_secs` of your slowest `[[llm.providers]]` entry.
    #[serde(default = "default_long_request_timeout_secs")]
    pub long_request_timeout_secs: u64,
    /// Open connections the server accepts at once (NET-1); further ones are closed at once.
    /// 1 to 100000.
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
    /// `POST /api/chat` requests per minute and session (NET-2); each can cost several model
    /// calls. 1 to 10000.
    #[serde(default = "default_chat_rate_limit")]
    pub chat_rate_limit_per_minute: usize,
    /// `POST /api/voice/transcribe` requests per minute and session (NET-2). 1 to 10000.
    #[serde(default = "default_transcribe_rate_limit")]
    pub transcribe_rate_limit_per_minute: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            static_dir: None,
            tls_cert: None,
            tls_key: None,
            dev_origins: Vec::new(),
            header_read_timeout_secs: default_header_read_timeout_secs(),
            request_timeout_secs: default_request_timeout_secs(),
            long_request_timeout_secs: default_long_request_timeout_secs(),
            max_connections: default_max_connections(),
            chat_rate_limit_per_minute: default_chat_rate_limit(),
            transcribe_rate_limit_per_minute: default_transcribe_rate_limit(),
        }
    }
}

fn default_bind() -> String {
    "127.0.0.1:8787".to_owned()
}
fn default_header_read_timeout_secs() -> u64 {
    10
}
fn default_request_timeout_secs() -> u64 {
    30
}
fn default_long_request_timeout_secs() -> u64 {
    200
}
fn default_max_connections() -> usize {
    256
}
fn default_chat_rate_limit() -> usize {
    20
}
fn default_transcribe_rate_limit() -> usize {
    20
}

#[derive(Debug, Clone, Deserialize)]
pub struct BridgeSection {
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// File that holds the shared bridge key (64 hex characters): the `bridge.key` that
    /// `scripts/install-ea.ps1` creates in the MT5 data folder (`MQL5\Files\JarvisMT`). The
    /// environment variable `JARVIS_BRIDGE_KEY` (the same 64 characters) overrides it. The
    /// server does not start without one of them (BR-1, ADR-0016).
    #[serde(default)]
    pub key_file: Option<PathBuf>,
}

impl Default for BridgeSection {
    fn default() -> Self {
        Self {
            endpoint: default_endpoint(),
            timeout_ms: default_timeout_ms(),
            key_file: None,
        }
    }
}

/// Where the bridge key comes from, for error messages.
const KEY_HELP: &str = r"The EA only answers a holder of the shared bridge key. Run `pwsh scripts\install-ea.ps1` (it creates MQL5\Files\JarvisMT\bridge.key in your MT5 data folder and prints the path), then set `key_file` under [bridge] in config/jarvis.toml to that path, or put the key's 64 hex characters in JARVIS_BRIDGE_KEY.";

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
        Self::load_with(None)
    }

    /// Like [`Config::load`] with an explicit config path (the `--config` flag).
    pub fn load_with(path: Option<&std::path::Path>) -> anyhow::Result<Self> {
        // A missing .env is normal; a broken one shows up later as missing API keys.
        let _ = dotenvy::dotenv();
        match path {
            Some(p) => Self::load_from(p),
            None => Self::load_from(&Self::path()),
        }
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
        // Broker symbols in `allowed_symbols` get the same suffix as every resolved symbol,
        // so `XAUUSD` in the config matches the `XAUUSD.m` the resolver produces.
        let resolver =
            crate::symbols::SymbolResolver::new(&self.symbols.aliases, &self.symbols.suffix);
        if let Some(limits) = &mut self.limits {
            limits.allowed_symbols = limits
                .allowed_symbols
                .iter()
                .filter_map(|s| resolver.resolve(s))
                .collect();
        }
    }

    /// The shared key for the bridge: `JARVIS_BRIDGE_KEY` (hex) if set and non-empty, otherwise
    /// the file named by `[bridge] key_file`. Fails, with a message that says what to do, when
    /// there is no usable key. Never includes the key in an error.
    pub fn bridge_key(&self) -> anyhow::Result<BridgeKey> {
        let from_env = std::env::var(KEY_ENV).ok();
        self.bridge_key_from(from_env.as_deref())
    }

    /// [`Config::bridge_key`] with the environment value passed in (tests).
    fn bridge_key_from(&self, from_env: Option<&str>) -> anyhow::Result<BridgeKey> {
        match (
            from_env.filter(|v| !v.trim().is_empty()),
            &self.bridge.key_file,
        ) {
            (Some(hex), _) => BridgeKey::from_hex(hex)
                .map_err(|e| anyhow!("{KEY_ENV} is not a usable bridge key: {e}. {KEY_HELP}")),
            (None, Some(path)) => BridgeKey::from_file(path)
                .map_err(|e| anyhow!("[bridge] key_file is not usable: {e}. {KEY_HELP}")),
            (None, None) => bail!("no bridge key is configured. {KEY_HELP}"),
        }
    }

    /// The limits the gate enforces. Without a `[limits]` section nothing may be traded:
    /// `allowed_symbols` is empty and the gate fails closed on that.
    pub fn effective_limits(&self) -> Limits {
        self.limits.clone().unwrap_or_else(|| Limits {
            max_lot: 1.0,
            max_positions: 5,
            max_daily_loss: 500.0,
            allowed_symbols: Vec::new(),
            trading_hours: None,
        })
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        // `mode` is checked by serde (demo | live).
        validate_loopback_endpoint(&self.bridge.endpoint)?;
        if self.bridge.timeout_ms == 0 {
            bail!("bridge.timeout_ms must be greater than 0");
        }
        self.bind_addr()?;
        for origin in &self.server.dev_origins {
            validate_origin(origin)?;
        }
        self.validate_server_limits()?;
        if !(1..=168).contains(&self.auth.session_hours) {
            bail!("auth.session_hours must be between 1 and 168");
        }
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
        self.validate_trading()?;
        Ok(())
    }

    fn validate_server_limits(&self) -> anyhow::Result<()> {
        let s = &self.server;
        if !(1..=120).contains(&s.header_read_timeout_secs) {
            bail!("server.header_read_timeout_secs must be between 1 and 120");
        }
        if !(1..=600).contains(&s.request_timeout_secs) {
            bail!("server.request_timeout_secs must be between 1 and 600");
        }
        if !(s.request_timeout_secs..=3600).contains(&s.long_request_timeout_secs) {
            bail!(
                "server.long_request_timeout_secs must be between server.request_timeout_secs \
                 and 3600"
            );
        }
        if !(1..=100_000).contains(&s.max_connections) {
            bail!("server.max_connections must be between 1 and 100000");
        }
        for (name, value) in [
            ("chat_rate_limit_per_minute", s.chat_rate_limit_per_minute),
            (
                "transcribe_rate_limit_per_minute",
                s.transcribe_rate_limit_per_minute,
            ),
        ] {
            if !(1..=10_000).contains(&value) {
                bail!("server.{name} must be between 1 and 10000");
            }
        }
        Ok(())
    }

    /// Providers whose own timeout is not shorter than the time a chat request may take: a turn
    /// that waits for them can be cut short by the server. For a start-up warning.
    pub fn providers_slower_than_chat_timeout(&self) -> Vec<&str> {
        self.llm
            .providers
            .iter()
            .filter(|p| p.timeout_secs >= self.server.long_request_timeout_secs)
            .map(|p| p.name.as_str())
            .collect()
    }

    fn validate_trading(&self) -> anyhow::Result<()> {
        let t = &self.trading;
        if t.token_ttl_secs == 0 || t.token_ttl_secs > 120 {
            bail!("trading.token_ttl_secs must be between 1 and 120");
        }
        if t.voice_token_cap_secs < t.token_ttl_secs {
            bail!("trading.voice_token_cap_secs must be at least trading.token_ttl_secs");
        }
        if t.deviation_points > 1000 {
            bail!("trading.deviation_points must be at most 1000");
        }
        if let Some(limits) = &self.limits {
            limits
                .validate()
                .map_err(|e| anyhow!("invalid [limits]: {e}"))?;
        }
        Ok(())
    }

    /// True when the server terminates TLS itself (`tls_cert` and `tls_key` are set).
    pub fn tls_enabled(&self) -> bool {
        self.server.tls_cert.is_some() && self.server.tls_key.is_some()
    }

    /// Whether the session cookie gets the `Secure` attribute: the explicit
    /// `auth.cookie_secure`, otherwise whenever the server serves TLS. Plain HTTP on
    /// loopback (development) is the only case without it.
    pub fn cookie_secure(&self) -> bool {
        self.auth
            .cookie_secure
            .unwrap_or_else(|| self.tls_enabled())
    }

    /// Parsed `server.bind`. Anything other than loopback is refused unless the server
    /// serves TLS **and** login is enabled (ADR-0011): without both, whoever can reach the
    /// port could confirm trades.
    pub fn bind_addr(&self) -> anyhow::Result<SocketAddr> {
        let addr: SocketAddr = self.server.bind.parse().map_err(|e| {
            anyhow!(
                "server.bind '{}' is not a valid host:port ({e})",
                self.server.bind
            )
        })?;
        if self.server.tls_cert.is_some() != self.server.tls_key.is_some() {
            bail!("server.tls_cert and server.tls_key must be set together");
        }
        let protected = self.tls_enabled() && self.auth.enabled;
        if !addr.ip().is_loopback() && !protected {
            bail!(
                "refusing to bind {addr}: a non-loopback address needs HTTPS and login. Set \
                 server.tls_cert and server.tls_key, and keep [auth] enabled = true, or use a \
                 loopback address such as 127.0.0.1:8787"
            );
        }
        Ok(addr)
    }
}

/// `server.dev_origins` entries are exactly `scheme://host[:port]` (what a browser sends as
/// `Origin`): no path, no wildcard, no credentials.
fn validate_origin(origin: &str) -> anyhow::Result<()> {
    let bad = || {
        anyhow!(
            "server.dev_origins entry '{origin}' must look like http://localhost:5173 (scheme, host and optional port only)"
        )
    };
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .ok_or_else(bad)?;
    let ok = !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'));
    if ok { Ok(()) } else { Err(bad()) }
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

    fn bind_config(server: &str, auth: &str) -> anyhow::Result<Config> {
        Config::from_toml_str(&format!(
            "[server]\n{server}\n[auth]\n{auth}\n[llm]\ndefault = \"l\"\n\
             [[llm.providers]]\nname = \"l\"\nkind = \"ollama\"\nmodel = \"m\""
        ))
    }

    #[test]
    fn non_loopback_bind_needs_tls_and_auth() {
        let tls = "tls_cert = \"c.pem\"\ntls_key = \"k.pem\"";
        // Loopback is always fine, with or without auth or TLS.
        assert!(bind_config("bind = \"127.0.0.1:0\"", "enabled = false").is_ok());
        assert!(bind_config("bind = \"[::1]:8787\"", "enabled = true").is_ok());
        // Off loopback: neither, TLS only, auth only are all refused.
        for (server, auth) in [
            ("bind = \"0.0.0.0:8787\"".to_owned(), "enabled = true"),
            (format!("bind = \"0.0.0.0:8787\"\n{tls}"), "enabled = false"),
            ("bind = \"192.168.1.5:8787\"".to_owned(), "enabled = true"),
        ] {
            let err = bind_config(&server, auth).unwrap_err();
            assert!(err.to_string().contains("HTTPS and login"), "{err}");
        }
        // TLS and auth: allowed.
        assert!(bind_config(&format!("bind = \"0.0.0.0:8787\"\n{tls}"), "enabled = true").is_ok());
    }

    #[test]
    fn tls_files_must_come_as_a_pair() {
        let err = bind_config("tls_cert = \"c.pem\"", "enabled = true").unwrap_err();
        assert!(err.to_string().contains("together"), "{err}");
    }

    #[test]
    fn auth_defaults_and_cookie_secure_rules() {
        let c = Config::from_toml_str(&minimal("")).unwrap();
        assert!(c.auth.enabled);
        assert_eq!(c.auth.session_hours, 12);
        assert!(!c.cookie_secure(), "plain HTTP on loopback: no Secure");
        let tls = "[server]\ntls_cert = \"c\"\ntls_key = \"k\"";
        assert!(
            Config::from_toml_str(&minimal(tls))
                .unwrap()
                .cookie_secure()
        );
        let proxy = "[auth]\ncookie_secure = true";
        assert!(
            Config::from_toml_str(&minimal(proxy))
                .unwrap()
                .cookie_secure()
        );
        assert!(Config::from_toml_str(&minimal("[auth]\nsession_hours = 0")).is_err());
    }

    #[test]
    fn voice_section_needs_paths_only_when_enabled() {
        let off = Config::from_toml_str(&minimal("[voice]\nenabled = false")).unwrap();
        assert!(off.voice.unwrap().to_voice_config().unwrap().is_none());
        // Enabled without paths is not a config error (the server starts with voice off and
        // says why); asking for the engine config is.
        let c = Config::from_toml_str(&minimal("[voice]\nenabled = true")).unwrap();
        let err = c.voice.unwrap().to_voice_config().unwrap_err();
        assert!(format!("{err:#}").contains("whisper_model"), "{err:#}");
        let c = Config::from_toml_str(&minimal(
            "[voice]\nwhisper_model = \"w.bin\"\npiper_exe = \"p.exe\"\npiper_voice = \"v.onnx\"",
        ))
        .unwrap();
        let v = c.voice.unwrap().to_voice_config().unwrap().unwrap();
        assert_eq!(v.language, "en");
        assert!((v.max_seconds - 30.0).abs() < f32::EPSILON);
    }

    // ---- the shared bridge key (BR-1) ------------------------------------------------------

    const KEY_A: &str = "0f0e0d0c0b0a09080706050403020100ffeeddccbbaa99887766554433221100";
    const KEY_B: &str = "a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebfc0";

    fn config_with_key_file(contents: &str) -> (std::path::PathBuf, Config) {
        let dir = std::env::temp_dir().join(format!("jarvis-keycfg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bridge.key");
        std::fs::write(&path, contents).unwrap();
        let toml = minimal(&format!("[bridge]\nkey_file = '{}'", path.display()));
        (path, Config::from_toml_str(&toml).unwrap())
    }

    fn remove_key_dir(path: &std::path::Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn the_bridge_key_comes_from_the_file() {
        let (path, c) = config_with_key_file(&format!("{KEY_A}\n"));
        assert_eq!(c.bridge.key_file.as_deref(), Some(path.as_path()));
        assert_eq!(c.bridge_key_from(None).unwrap().expose_hex(), KEY_A);
        // An empty or blank environment variable counts as unset.
        assert_eq!(c.bridge_key_from(Some("  ")).unwrap().expose_hex(), KEY_A);
        remove_key_dir(&path);
    }

    #[test]
    fn the_environment_key_wins_over_the_file() {
        let (_, c) = config_with_key_file(&format!("{KEY_A}\n"));
        assert_eq!(c.bridge_key_from(Some(KEY_B)).unwrap().expose_hex(), KEY_B);
        // A broken environment key is an error, not a silent fall back to the file.
        let err = c.bridge_key_from(Some("not-a-key")).unwrap_err();
        assert!(format!("{err:#}").contains("JARVIS_BRIDGE_KEY"), "{err:#}");
    }

    #[test]
    fn without_a_key_the_server_says_how_to_get_one() {
        let c = Config::from_toml_str(&minimal("")).unwrap();
        assert!(c.bridge.key_file.is_none());
        let err = format!("{:#}", c.bridge_key_from(None).unwrap_err());
        assert!(err.contains("no bridge key"), "{err}");
        assert!(err.contains("install-ea.ps1"), "{err}");
        assert!(
            err.contains("key_file") && err.contains("JARVIS_BRIDGE_KEY"),
            "{err}"
        );
    }

    #[test]
    fn a_missing_or_malformed_key_file_is_refused_without_echoing_it() {
        let c =
            Config::from_toml_str(&minimal("[bridge]\nkey_file = 'no/such/bridge.key'")).unwrap();
        let err = format!("{:#}", c.bridge_key_from(None).unwrap_err());
        assert!(err.contains("cannot read the bridge key file"), "{err}");
        assert!(err.contains("install-ea.ps1"), "{err}");

        let wrong_chars = "zz".repeat(32);
        let zeros = "00".repeat(32);
        for bad in ["", "tooshort", wrong_chars.as_str(), zeros.as_str()] {
            let (path, c) = config_with_key_file(bad);
            let err = format!("{:#}", c.bridge_key_from(None).unwrap_err());
            remove_key_dir(&path);
            assert!(err.contains("install-ea.ps1"), "{err}");
            if bad.len() > 8 {
                assert!(
                    !err.contains(bad),
                    "the file content must not appear in the error"
                );
            }
        }
    }

    #[test]
    fn missing_file_explains_how_to_fix() {
        let err = Config::load_from(std::path::Path::new("definitely/not/here.toml")).unwrap_err();
        assert!(err.to_string().contains("jarvis.example.toml"), "{err}");
    }

    // ---- [server] limits (NET-1, NET-2) -------------------------------------------------------

    #[test]
    fn server_limits_have_documented_defaults() {
        let c = Config::from_toml_str(&minimal("")).unwrap();
        let s = &c.server;
        assert_eq!(s.header_read_timeout_secs, 10);
        assert_eq!(s.request_timeout_secs, 30);
        assert_eq!(s.long_request_timeout_secs, 200);
        assert_eq!(s.max_connections, 256);
        assert_eq!(s.chat_rate_limit_per_minute, 20);
        assert_eq!(s.transcribe_rate_limit_per_minute, 20);
        // The example file spells the same numbers out.
        let e = Config::from_toml_str(EXAMPLE).unwrap();
        assert_eq!(e.server.header_read_timeout_secs, 10);
        assert_eq!(e.server.request_timeout_secs, 30);
        assert_eq!(e.server.long_request_timeout_secs, 200);
        assert_eq!(e.server.max_connections, 256);
        assert_eq!(e.server.chat_rate_limit_per_minute, 20);
        assert_eq!(e.server.transcribe_rate_limit_per_minute, 20);
    }

    #[test]
    fn server_limits_are_range_checked() {
        let with = |line: &str| Config::from_toml_str(&minimal(&format!("[server]\n{line}")));
        for ok in [
            "header_read_timeout_secs = 1",
            "request_timeout_secs = 600\nlong_request_timeout_secs = 600",
            "long_request_timeout_secs = 3600",
            "max_connections = 1",
            "chat_rate_limit_per_minute = 10000",
            "transcribe_rate_limit_per_minute = 1",
        ] {
            assert!(with(ok).is_ok(), "{ok}");
        }
        for bad in [
            "header_read_timeout_secs = 0",
            "header_read_timeout_secs = 121",
            "request_timeout_secs = 0",
            "request_timeout_secs = 601",
            "long_request_timeout_secs = 29",
            "long_request_timeout_secs = 3601",
            "request_timeout_secs = 100\nlong_request_timeout_secs = 50",
            "max_connections = 0",
            "chat_rate_limit_per_minute = 0",
            "chat_rate_limit_per_minute = 10001",
            "transcribe_rate_limit_per_minute = 0",
        ] {
            assert!(with(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn slow_providers_are_reported_against_the_chat_timeout() {
        let c = Config::from_toml_str(&format!("{}\ntimeout_secs = 300", minimal(""))).unwrap();
        assert_eq!(c.providers_slower_than_chat_timeout(), ["local"]);
        let c = Config::from_toml_str(&minimal("")).unwrap();
        assert!(c.providers_slower_than_chat_timeout().is_empty());
    }
}
