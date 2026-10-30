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
use serde_json::{Value, json};
use tokio::sync::{Semaphore, watch};

use crate::agent::Agent;
use crate::audit::Audit;
use crate::auth::crypto::PasswordHasherCfg;
use crate::auth::{AuthParts, AuthReleaseGuard, AuthService, Session};
use crate::clock::{ServerClock, TimeSource, WallClock};
use crate::config::{Config, Mode};
use crate::events::EventBus;
use crate::symbols::SymbolResolver;
use crate::tools::ToolRegistry;
use crate::trading::{Trading, TradingParts, WriteSink};
use crate::voice::Voice;

/// Environment variable holding the session signing key (at least 32 bytes).
pub const JWT_SECRET_ENV: &str = "JARVIS_JWT_SECRET";

/// How often expired previews and idle rate-limit keys are cleaned up.
pub const PURGE_INTERVAL: Duration = Duration::from_secs(5);

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

/// Open WebSocket connections allowed at once.
const MAX_SOCKETS: usize = crate::ws::MAX_SOCKETS;

struct Inner {
    config: Config,
    bridge: BridgeClient,
    providers: Vec<ProviderEntry>,
    audit: Audit,
    agent: Agent,
    resolver: Arc<SymbolResolver>,
    clock: ServerClock,
    /// Rule 7: set when the config says DEMO but the account reports a real trade mode.
    mode: Arc<ModeMonitor>,
    trading: Arc<Trading>,
    events: EventBus,
    release_guard: RwLock<Arc<dyn ReleaseGuard>>,
    auth: Option<Arc<AuthService>>,
    voice: Voice,
    shutdown: ShutdownSignal,
    ws_slots: Arc<Semaphore>,
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
        config: Config,
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
        let resolver = Arc::new(SymbolResolver::new(
            &config.symbols.aliases,
            &config.symbols.suffix,
        ));
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
        let mode = Arc::new(ModeMonitor::new(config.mode));
        let events = EventBus::new();
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
            clock,
            mode,
            trading,
            events,
            release_guard: RwLock::new(release_guard),
            auth,
            voice: overrides.voice.unwrap_or_else(Voice::disabled),
            shutdown: ShutdownSignal::new(),
            ws_slots: Arc::new(Semaphore::new(MAX_SOCKETS)),
        }))
    }

    /// Production wiring: bridge client, providers via `build_provider`, audit database.
    pub async fn build(config: Config) -> anyhow::Result<Self> {
        let bridge = BridgeClient::new(BridgeConfig {
            endpoint: config.bridge.endpoint.clone(),
            timeout: Duration::from_millis(config.bridge.timeout_ms),
        })?;
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
            Some(Ok(Some(voice_config))) => Voice::from_config(&voice_config).await,
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
                ..Overrides::default()
            },
        );
        // A kill switch engaged before a restart stays engaged (SPEC rule 6).
        state.trading().restore_kill_switch().await;
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

    /// Ping the bridge and run the rule-7 check on success.
    pub async fn ping_bridge(&self) -> Result<PingInfo, BridgeError> {
        let info = self.0.bridge.ping().await?;
        self.observe_trade_mode(info.trade_mode);
        self.0
            .clock
            .observe(info.server_time, chrono::Utc::now().timestamp());
        Ok(info)
    }

    /// Start the periodic cleanup of expired previews and idle rate-limit keys. Call once
    /// from `main`; tests call [`Trading::purge`] directly.
    pub fn spawn_background_tasks(&self) -> tokio::task::JoinHandle<()> {
        let trading = self.0.trading.clone();
        let auth = self.0.auth.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(PURGE_INTERVAL);
            loop {
                tick.tick().await;
                trading.purge();
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
