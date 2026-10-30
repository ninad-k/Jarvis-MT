//! Shared application state.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::bail;
use async_trait::async_trait;
use jarvis_bridge_client::protocol::{PingInfo, TradeMode};
use jarvis_bridge_client::{BridgeClient, BridgeConfig, BridgeError};
use jarvis_gate::KillSwitch;
use jarvis_llm::{LlmProvider, ProviderConfig, ProviderKind, build_provider};
use jarvis_persona::{PersonaPack, PersonaRegistry, Theme};
use serde_json::{Value, json};
use tokio::sync::{Semaphore, watch};

use crate::agent::Agent;
use crate::audit::Audit;
use crate::auth::crypto::PasswordHasherCfg;
use crate::auth::{AuthParts, AuthReleaseGuard, AuthService, Session};
use crate::broker::{self, BrokerParts, BrokerService, DataPaths};
use crate::clock::{ServerClock, TimeSource, WallClock};
use crate::config::{Config, Mode};
use crate::events::EventBus;
use crate::limit_overrides::PartialLimits;
use crate::ratelimit::RateLimiter;
use crate::settings::{
    LOCAL_USER, Rules, SettingsError, SettingsPatch, SettingsStore, UserSettings, apply_patch,
};
use crate::symbols::SymbolResolver;
use crate::tools::ToolRegistry;
use crate::trading::{Trading, TradingParts, WriteSink};
use crate::voice::Voice;

/// Environment variable holding the session signing key (at least 32 bytes).
pub const JWT_SECRET_ENV: &str = "JARVIS_JWT_SECRET";

/// How often expired previews and idle rate-limit keys are cleaned up.
pub const PURGE_INTERVAL: Duration = Duration::from_secs(5);

/// The broker account is re-read every this many purge ticks (30 s).
const BROKER_CHECK_EVERY_TICKS: u32 = 6;

/// Replacements for production defaults, for tests and embedding.
#[derive(Default)]
pub struct Overrides {
    /// Time source for token windows and rate limits (default: the real clock).
    pub time: Option<Arc<dyn TimeSource>>,
    /// Destination of confirmed writes (default: the bridge client).
    pub write_sink: Option<Arc<dyn WriteSink>>,
    /// Login service. Required when `[auth] enabled = true`: without it every protected
    /// route answers 401 (fail closed). [`AppState::build`] creates it from the environment.
    pub auth: Option<Arc<AuthService>>,
    /// Speech engines (default: voice off).
    pub voice: Option<Voice>,
    /// The persona packs (default: loaded from `[personas] dir`, or one built-in Jarvis pack
    /// when that folder cannot be read; [`AppState::build`] refuses to start instead).
    pub personas: Option<PersonaRegistry>,
}

/// A configured LLM provider that is either ready or missing something (usually an API key).
#[derive(Clone)]
pub enum ProviderSlot {
    Available(Arc<dyn LlmProvider>),
    Unavailable(String),
}

pub struct ProviderEntry {
    pub config: ProviderConfig,
    pub slot: ProviderSlot,
}

impl ProviderEntry {
    pub fn kind_str(&self) -> &'static str {
        match self.config.kind {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Openai => "openai",
            ProviderKind::Ollama => "ollama",
        }
    }
}

/// SPEC rule 7: tracks whether DEMO mode is configured while the account is real.
/// Shared by [`AppState`] and [`Trading`].
#[derive(Debug)]
pub struct ModeMonitor {
    configured: Mode,
    mismatch: AtomicBool,
}

impl ModeMonitor {
    pub fn new(configured: Mode) -> Self {
        Self {
            configured,
            mismatch: AtomicBool::new(false),
        }
    }

    pub fn mismatch(&self) -> bool {
        self.mismatch.load(Ordering::SeqCst)
    }

    /// Record the account trade mode seen on a successful bridge call.
    pub fn observe(&self, seen: TradeMode) {
        let mismatch = self.configured == Mode::Demo && seen == TradeMode::Real;
        let was = self.mismatch.swap(mismatch, Ordering::SeqCst);
        if mismatch && !was {
            tracing::error!(
                "MODE MISMATCH: config mode is demo but the MT5 account trade_mode is real. \
                 Chat and trading are disabled until the account or the config is fixed."
            );
        } else if !mismatch && was {
            tracing::warn!("mode mismatch cleared: account trade_mode is now {seen:?}");
        }
    }
}

/// Decides whether the kill switch may be released. With login enabled the guard is
/// [`AuthReleaseGuard`] (a session and a fresh TOTP code); with login off, releases are open.
/// Replace it with [`AppState::set_release_guard`].
#[async_trait]
pub trait ReleaseGuard: Send + Sync {
    /// `session` is the caller's login session (`None` when login is disabled); `totp` is
    /// the code from the request body, if any. `Err(message)` refuses.
    async fn authorize_release(
        &self,
        session: Option<&Session>,
        totp: Option<&str>,
    ) -> Result<(), String>;
}

/// Releases are always allowed (auth disabled).
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenRelease;

#[async_trait]
impl ReleaseGuard for OpenRelease {
    async fn authorize_release(
        &self,
        _session: Option<&Session>,
        _totp: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }
}

/// Tells long-lived tasks (WebSocket connections) to finish because the server is stopping.
#[derive(Debug, Clone)]
pub struct ShutdownSignal {
    tx: Arc<watch::Sender<bool>>,
}

impl Default for ShutdownSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl ShutdownSignal {
    pub fn new() -> Self {
        Self {
            tx: Arc::new(watch::channel(false).0),
        }
    }

    pub fn trigger(&self) {
        self.tx.send_replace(true);
    }

    pub fn is_triggered(&self) -> bool {
        *self.tx.borrow()
    }

    /// A receiver whose `changed()` completes when the server starts shutting down (at once
    /// if it already has).
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        let mut rx = self.tx.subscribe();
        if *rx.borrow() {
            rx.mark_changed();
        }
        rx
    }
}

/// Why `PUT /api/settings` failed.
#[derive(Debug)]
pub enum SettingsUpdateError {
    /// 400.
    Invalid(SettingsError),
    /// 500.
    Storage(anyhow::Error),
}

/// The personas of the config folder, or, when that cannot be read (tests and embedding that
/// do not ship a `personas/` folder), one built-in pack so the state is still complete.
fn load_personas_or_builtin(config: &Config) -> PersonaRegistry {
    match PersonaRegistry::load_dir(&config.personas.dir, &config.personas.default) {
        Ok(registry) => registry,
        Err(e) => {
            tracing::warn!("{e}; using the built-in Jarvis persona");
            builtin_personas()
        }
    }
}

fn builtin_personas() -> PersonaRegistry {
    let jarvis = PersonaPack {
        id: "jarvis".to_owned(),
        name: "Jarvis".to_owned(),
        description: "Calm, formal and concise.".to_owned(),
        system_prompt: "Speak like a composed, formal British butler who happens to be an \
                        expert trader's aide. Be brief and precise: lead with the answer, then \
                        the essential numbers."
            .to_owned(),
        voice_model: "models/piper/en_GB-alan-medium.onnx".into(),
        wake_word: "hey jarvis".to_owned(),
        theme: Theme {
            accent: "#38bdf8".to_owned(),
            accent_soft: "#0c4a6e".to_owned(),
            orb_core: "#7dd3fc".to_owned(),
            orb_glow: "#0ea5e9".to_owned(),
        },
    };
    PersonaRegistry::from_packs(vec![jarvis], "jarvis")
        .expect("the built-in persona is valid by construction")
}

/// Open WebSocket connections allowed at once.
const MAX_SOCKETS: usize = crate::ws::MAX_SOCKETS;

struct Inner {
    config: Config,
    bridge: BridgeClient,
    providers: Vec<ProviderEntry>,
    audit: Audit,
    agent: Agent,
    resolver: Arc<SymbolResolver>,
    /// The broker account and its symbol maps.
    broker: Arc<BrokerService>,
    clock: ServerClock,
    /// Rule 7: set when the config says DEMO but the account reports a real trade mode.
    mode: Arc<ModeMonitor>,
    trading: Arc<Trading>,
    events: EventBus,
    release_guard: RwLock<Arc<dyn ReleaseGuard>>,
    auth: Option<Arc<AuthService>>,
    voice: Voice,
    personas: PersonaRegistry,
    settings: SettingsStore,
    shutdown: ShutdownSignal,
    ws_slots: Arc<Semaphore>,
    /// Per-session budgets of the expensive routes (NET-2).
    chat_limiter: RateLimiter,
    transcribe_limiter: RateLimiter,
}

/// The window of the per-session request budgets.
const REQUEST_RATE_WINDOW_MS: i64 = 60_000;

/// Which expensive route a request budget is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Budget {
    Chat,
    Transcribe,
}

/// Cheap to clone; shared by every request handler.
#[derive(Clone)]
pub struct AppState(Arc<Inner>);

impl AppState {
    /// Assemble state from ready parts. `slots` maps provider name to its slot; configured
    /// providers without a slot are reported unavailable.
    pub fn new(
        config: Config,
        bridge: BridgeClient,
        audit: Audit,
        slots: HashMap<String, ProviderSlot>,
    ) -> Self {
        Self::new_with(config, bridge, audit, slots, Overrides::default())
    }

    /// Like [`AppState::new`] with [`Overrides`] (tests move time by hand, fake a write
    /// transport).
    pub fn new_with(
        mut config: Config,
        bridge: BridgeClient,
        audit: Audit,
        mut slots: HashMap<String, ProviderSlot>,
        overrides: Overrides,
    ) -> Self {
        let time: Arc<dyn TimeSource> = overrides.time.unwrap_or_else(|| Arc::new(WallClock));
        let auth = overrides.auth;
        let release_guard: Arc<dyn ReleaseGuard> = match &auth {
            Some(service) => Arc::new(AuthReleaseGuard(service.clone())),
            None => Arc::new(OpenRelease),
        };
        // Symbol maps: names in the config become canonical, the resolver reads the book.
        let book = broker::book_from_config(&config);
        config.canonicalize_symbols(|name| book.canonical_loose(name));
        let resolver = Arc::new(SymbolResolver::with_book(
            &config.symbols.aliases,
            book.clone(),
        ));
        let events = EventBus::new();
        let broker = BrokerService::new(BrokerParts {
            book,
            bridge: bridge.clone(),
            audit: audit.clone(),
            events: events.clone(),
            paths: DataPaths::from_dir(&config.symbols.data_dir),
            auto_detect: config.symbols.auto_detect,
        });
        let personas = overrides
            .personas
            .unwrap_or_else(|| load_personas_or_builtin(&config));
        let settings = SettingsStore::new(audit.pool().clone());
        let providers = config
            .llm
            .providers
            .iter()
            .map(|p| ProviderEntry {
                config: p.clone(),
                slot: slots
                    .remove(&p.name)
                    .unwrap_or_else(|| ProviderSlot::Unavailable("not configured".to_owned())),
            })
            .collect();
        let clock = ServerClock::default();
        let (chat_per_minute, transcribe_per_minute) = (
            config.server.chat_rate_limit_per_minute,
            config.server.transcribe_rate_limit_per_minute,
        );
        let mode = Arc::new(ModeMonitor::new(config.mode));
        let trading = Trading::new(TradingParts {
            bridge: bridge.clone(),
            write_sink: overrides.write_sink,
            clock: clock.clone(),
            mode: mode.clone(),
            audit: audit.clone(),
            limits: config.effective_limits(),
            config: config.trading.clone(),
            kill_switch: KillSwitch::new(config.trading.kill_switch_engaged),
            time,
            events: events.clone(),
            broker: broker.clone(),
        });
        let tools = ToolRegistry::new(bridge.clone(), resolver.clone(), clock.clone())
            .with_trading(trading.clone());
        let agent = Agent::new(tools, audit.clone(), clock.clone());
        Self(Arc::new(Inner {
            config,
            bridge,
            providers,
            audit,
            agent,
            resolver,
            broker,
            clock,
            mode,
            trading,
            events,
            release_guard: RwLock::new(release_guard),
            auth,
            voice: overrides.voice.unwrap_or_else(Voice::disabled),
            personas,
            settings,
            shutdown: ShutdownSignal::new(),
            ws_slots: Arc::new(Semaphore::new(MAX_SOCKETS)),
            chat_limiter: RateLimiter::new(chat_per_minute, REQUEST_RATE_WINDOW_MS),
            transcribe_limiter: RateLimiter::new(transcribe_per_minute, REQUEST_RATE_WINDOW_MS),
        }))
    }

    /// Production wiring: bridge client, providers via `build_provider`, audit database.
    pub async fn build(config: Config) -> anyhow::Result<Self> {
        // Refuse to start without the shared bridge key: the EA would refuse every request
        // anyway, and a server that cannot reach it must say why, not fail on the first call.
        let key = config
            .bridge_key()
            .map_err(|e| anyhow::anyhow!("refusing to start: {e:#}"))?;
        let bridge = BridgeClient::new(BridgeConfig::new(
            config.bridge.endpoint.clone(),
            Duration::from_millis(config.bridge.timeout_ms),
            key,
        ))?;
        let personas = PersonaRegistry::load_dir(&config.personas.dir, &config.personas.default)
            .map_err(|e| {
                anyhow::anyhow!(
                    "refusing to start: the personas are unusable: {e}. Fix or restore the pack \
                     files in {}, or point [personas] dir and default at working ones.",
                    config.personas.dir.display()
                )
            })?;
        tracing::info!(
            default = personas.default_id(),
            "personas loaded: {}",
            personas
                .list()
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let audit = Audit::connect(&config.database.url).await?;
        let auth = if config.auth.enabled {
            let secret = std::env::var(JWT_SECRET_ENV).unwrap_or_default();
            let service = AuthService::new(AuthParts {
                secret,
                session_hours: config.auth.session_hours,
                cookie_secure: config.cookie_secure(),
                time: Arc::new(WallClock),
                audit: audit.clone(),
                hasher: PasswordHasherCfg::owasp(),
            })
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "refusing to start: login is enabled but the session key is unusable: {e:#}. \
                     Set {JWT_SECRET_ENV} in .env (or the environment), or set [auth] enabled = \
                     false on a loopback-only server."
                )
            })?;
            if service.accounts().user_count().await? == 0 {
                tracing::warn!(
                    "login is enabled but there are no users yet; create one with \
                     `jarvis-api user add <name>`"
                );
            }
            Some(service)
        } else {
            tracing::warn!(
                "login is DISABLED ([auth] enabled = false): anyone who can reach the server can trade"
            );
            None
        };
        let voice = match config.voice.as_ref().map(|v| v.to_voice_config()) {
            Some(Ok(Some(voice_config))) => {
                Voice::from_config(&voice_config, personas.list()).await
            }
            Some(Err(e)) => {
                tracing::warn!("voice is off: {e:#}");
                Voice::disabled()
            }
            _ => Voice::disabled(),
        };
        let mut slots = HashMap::new();
        for p in &config.llm.providers {
            let slot = match build_provider(p) {
                Ok(provider) => ProviderSlot::Available(Arc::from(provider)),
                Err(e) => {
                    tracing::warn!(provider = %p.name, "LLM provider unavailable: {e}");
                    ProviderSlot::Unavailable(e.to_string())
                }
            };
            slots.insert(p.name.clone(), slot);
        }
        if config.limits.is_none() {
            tracing::warn!(
                "no [limits] section in the config: allowed_symbols is empty, so every new order \
                 will be refused. See config/jarvis.example.toml."
            );
        }
        let state = Self::new_with(
            config,
            bridge,
            audit,
            slots,
            Overrides {
                auth,
                voice: Some(voice),
                personas: Some(personas),
                ..Overrides::default()
            },
        );
        // A kill switch engaged before a restart stays engaged (SPEC rule 6).
        state.trading().restore_kill_switch().await;
        // So do limits the user tightened.
        state.restore_limit_overrides().await;
        Ok(state)
    }

    pub fn config(&self) -> &Config {
        &self.0.config
    }
    pub fn bridge(&self) -> &BridgeClient {
        &self.0.bridge
    }
    pub fn audit(&self) -> &Audit {
        &self.0.audit
    }
    pub fn agent(&self) -> &Agent {
        &self.0.agent
    }
    pub fn clock(&self) -> &ServerClock {
        &self.0.clock
    }
    pub fn resolver(&self) -> &SymbolResolver {
        &self.0.resolver
    }
    /// The broker account and its symbol maps.
    pub fn broker(&self) -> &Arc<BrokerService> {
        &self.0.broker
    }
    pub fn trading(&self) -> &Arc<Trading> {
        &self.0.trading
    }
    /// Server events (kill switch, trade results). The WebSocket route subscribes here.
    pub fn events(&self) -> &EventBus {
        &self.0.events
    }
    pub fn release_guard(&self) -> Arc<dyn ReleaseGuard> {
        self.0
            .release_guard
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    /// Install the guard that authorises kill-switch releases (the auth task).
    pub fn set_release_guard(&self, guard: Arc<dyn ReleaseGuard>) {
        *self
            .0
            .release_guard
            .write()
            .unwrap_or_else(|p| p.into_inner()) = guard;
    }
    /// The login service; `None` when `[auth] enabled = false`.
    pub fn auth(&self) -> Option<&Arc<AuthService>> {
        self.0.auth.as_ref()
    }
    /// `auth` object of `GET /api/status`: whether login is on and who the caller is.
    pub fn auth_status(&self, session: Option<&Session>) -> Value {
        json!({
            "enabled": self.0.config.auth.enabled,
            "username": session.map(|s| s.username.as_str()),
        })
    }
    pub fn voice(&self) -> &Voice {
        &self.0.voice
    }
    /// The persona packs.
    pub fn personas(&self) -> &PersonaRegistry {
        &self.0.personas
    }
    pub fn settings_store(&self) -> &SettingsStore {
        &self.0.settings
    }

    /// The persona a user has chosen (the default if theirs no longer exists).
    pub fn persona_for(&self, settings: &UserSettings) -> &PersonaPack {
        self.0
            .personas
            .get(&settings.persona)
            .unwrap_or_else(|_| self.0.personas.default_persona())
    }

    /// The symbol resolver for a user's turns: the configured aliases with the user's own
    /// merged over them.
    pub fn resolver_for(&self, settings: &UserSettings) -> Arc<SymbolResolver> {
        if settings.aliases.is_empty() {
            self.0.resolver.clone()
        } else {
            Arc::new(self.0.resolver.with_user_aliases(&settings.aliases))
        }
    }

    /// Where a caller's settings are stored: their login name, or one shared local profile
    /// when login is off.
    pub fn settings_key(session: Option<&Session>) -> &str {
        session.map_or(LOCAL_USER, |s| s.username.as_str())
    }

    fn default_settings(&self) -> UserSettings {
        UserSettings::defaults(
            self.0.personas.default_id(),
            &self.0.config.symbols.watchlist,
        )
    }

    /// Replace values that no longer exist (a persona pack or provider that was removed).
    fn sanitize(&self, mut settings: UserSettings) -> UserSettings {
        if self.0.personas.get(&settings.persona).is_err() {
            settings.persona = self.0.personas.default_id().to_owned();
        }
        if settings
            .llm_provider
            .as_deref()
            .is_some_and(|name| self.provider(name).is_none())
        {
            settings.llm_provider = None;
        }
        // Older settings stored broker names (`XAUUSD.m`); the watchlist is canonical now.
        let book = self.0.broker.book();
        let mut watchlist: Vec<String> = Vec::with_capacity(settings.watchlist.len());
        for name in &settings.watchlist {
            if let Some(c) = book.canonical_loose(name)
                && !watchlist.contains(&c)
            {
                watchlist.push(c);
            }
        }
        settings.watchlist = watchlist;
        settings
    }

    /// The caller's settings. A storage failure is logged and answered with the defaults, so
    /// chat keeps working.
    pub async fn user_settings(&self, session: Option<&Session>) -> UserSettings {
        match self.stored_settings(session).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("reading user settings failed, using the defaults: {e:#}");
                self.default_settings()
            }
        }
    }

    async fn stored_settings(&self, session: Option<&Session>) -> anyhow::Result<UserSettings> {
        let stored = self.0.settings.get(Self::settings_key(session)).await?;
        Ok(self.sanitize(stored.unwrap_or_else(|| self.default_settings())))
    }

    /// `PUT /api/settings`: validate and store a partial update; returns the full settings.
    pub async fn update_settings(
        &self,
        session: Option<&Session>,
        patch: SettingsPatch,
    ) -> Result<UserSettings, SettingsUpdateError> {
        let key = Self::settings_key(session);
        let _write = self.0.settings.lock().await;
        let current = self
            .stored_settings(session)
            .await
            .map_err(SettingsUpdateError::Storage)?;
        let rules = Rules {
            personas: &self.0.personas,
            providers: self
                .0
                .providers
                .iter()
                .map(|p| p.config.name.as_str())
                .collect(),
            resolver: &self.0.resolver,
        };
        let next = apply_patch(current, patch, &rules).map_err(SettingsUpdateError::Invalid)?;
        self.0
            .settings
            .put(key, &next)
            .await
            .map_err(SettingsUpdateError::Storage)?;
        Ok(next)
    }

    /// Make `persona` the caller's active persona (a spoken wake phrase). `persona` must be a
    /// loaded pack.
    pub async fn set_user_persona(
        &self,
        session: Option<&Session>,
        persona: &str,
    ) -> anyhow::Result<UserSettings> {
        let key = Self::settings_key(session);
        let _write = self.0.settings.lock().await;
        let mut settings = self.stored_settings(session).await?;
        persona.clone_into(&mut settings.persona);
        let settings = self.sanitize(settings);
        self.0.settings.put(key, &settings).await?;
        Ok(settings)
    }

    /// Startup: load the stored tighten-only limit overrides into the gate. If they cannot be
    /// read, trading is switched off by an empty symbol list until they are reset (fail
    /// closed, like the kill switch).
    pub async fn restore_limit_overrides(&self) {
        match self.0.settings.load_limit_overrides().await {
            Ok(Some(mut overrides)) => {
                // Stored by an older version as broker names; limits are canonical now.
                if let Some(list) = &overrides.allowed_symbols {
                    let book = self.0.broker.book();
                    let mut canonical: Vec<String> = Vec::with_capacity(list.len());
                    for name in list {
                        if let Some(c) = book.canonical_loose(name)
                            && !canonical.contains(&c)
                        {
                            canonical.push(c);
                        }
                    }
                    overrides.allowed_symbols = Some(canonical);
                }
                let effective = self.0.trading.set_limit_overrides(Some(overrides));
                tracing::info!(?effective, "tightened limits restored");
            }
            Ok(None) => {}
            Err(e) => {
                tracing::error!(
                    "cannot read the saved limit overrides; new orders are refused until they \
                     are reset in Settings: {e:#}"
                );
                self.0.trading.set_limit_overrides(Some(PartialLimits {
                    allowed_symbols: Some(Vec::new()),
                    ..PartialLimits::default()
                }));
            }
        }
    }
    /// `voice` object of `GET /api/status`.
    pub fn voice_status(&self) -> Value {
        json!({ "stt": self.0.voice.stt_ready(), "tts": self.0.voice.tts_ready() })
    }
    /// Fires when the server begins to stop; WebSocket tasks close on it.
    pub fn shutdown(&self) -> &ShutdownSignal {
        &self.0.shutdown
    }
    /// Permits for open WebSocket connections.
    pub fn ws_slots(&self) -> Arc<Semaphore> {
        self.0.ws_slots.clone()
    }
    /// Spend one request of `budget` for `key` (the session, or the peer address when there is
    /// none). `Err(retry_after_secs)` when the key is over its limit (NET-2).
    pub fn spend_budget(&self, budget: Budget, key: &str) -> Result<(), u64> {
        let now = self.0.trading.now_ms();
        match budget {
            Budget::Chat => self.0.chat_limiter.check(key, now),
            Budget::Transcribe => self.0.transcribe_limiter.check(key, now),
        }
    }

    pub fn providers(&self) -> &[ProviderEntry] {
        &self.0.providers
    }
    pub fn provider(&self, name: &str) -> Option<&ProviderEntry> {
        self.0.providers.iter().find(|p| p.config.name == name)
    }

    /// True when DEMO mode is configured but the account reports a real trade mode.
    pub fn mode_mismatch(&self) -> bool {
        self.0.mode.mismatch()
    }

    /// Record the account trade mode seen on a successful bridge call (rule 7).
    pub fn observe_trade_mode(&self, seen: TradeMode) {
        self.0.mode.observe(seen);
    }

    /// Ping the bridge and run the rule-7 check on success. Does not look at the broker
    /// account (see [`AppState::ping_bridge`]).
    pub async fn ping_only(&self) -> Result<PingInfo, BridgeError> {
        let info = self.0.bridge.ping().await?;
        self.observe_trade_mode(info.trade_mode);
        self.0
            .clock
            .observe(info.server_time, chrono::Utc::now().timestamp());
        Ok(info)
    }

    /// [`AppState::ping_only`], then, on success, check which broker account MT5 is on: a
    /// different broker starts a symbol detection.
    pub async fn ping_bridge(&self) -> Result<PingInfo, BridgeError> {
        let info = self.ping_only().await?;
        self.0.broker.observe().await;
        Ok(info)
    }

    /// Start the periodic cleanup of expired previews and idle rate-limit keys. Call once
    /// from `main`; tests call [`Trading::purge`] directly.
    pub fn spawn_background_tasks(&self) -> tokio::task::JoinHandle<()> {
        let trading = self.0.trading.clone();
        let auth = self.0.auth.clone();
        let state = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(PURGE_INTERVAL);
            let mut ticks: u32 = 0;
            loop {
                tick.tick().await;
                ticks = ticks.wrapping_add(1);
                trading.purge();
                // Notice an account switch even when nobody has the UI open.
                if ticks.is_multiple_of(BROKER_CHECK_EVERY_TICKS) {
                    state.0.broker.observe().await;
                }
                let now = trading.now_ms();
                state.0.chat_limiter.purge(now);
                state.0.transcribe_limiter.purge(now);
                if let Some(auth) = &auth {
                    auth.purge().await;
                }
            }
        })
    }

    /// Startup check (rule 7 / A10). Refuses to start when DEMO is configured against a real
    /// account. A bridge that is down is not fatal: the check repeats on later pings.
    pub async fn startup_check(&self) -> anyhow::Result<()> {
        match self.ping_bridge().await {
            Ok(info) => {
                if self.mode_mismatch() {
                    bail!(
                        "refusing to start: config mode is 'demo' but the MT5 account reports \
                         trade_mode '{:?}'. Use a demo account, or set mode = \"live\" \
                         deliberately.",
                        info.trade_mode
                    );
                }
                tracing::info!(
                    ea_version = %info.ea_version,
                    trade_mode = ?info.trade_mode,
                    terminal_connected = info.terminal_connected,
                    "bridge reachable"
                );
            }
            Err(e) => tracing::warn!(
                "bridge not reachable at startup ({e}); starting anyway, the account mode will \
                 be checked when the bridge comes up"
            ),
        }
        Ok(())
    }
}
