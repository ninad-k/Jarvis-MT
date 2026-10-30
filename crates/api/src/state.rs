//! Shared application state.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::bail;
use jarvis_bridge_client::protocol::{PingInfo, TradeMode};
use jarvis_bridge_client::{BridgeClient, BridgeConfig, BridgeError};
use jarvis_llm::{LlmProvider, ProviderConfig, ProviderKind, build_provider};

use crate::agent::Agent;
use crate::audit::Audit;
use crate::clock::ServerClock;
use crate::config::{Config, Mode};
use crate::symbols::SymbolResolver;
use crate::tools::ToolRegistry;

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

struct Inner {
    config: Config,
    bridge: BridgeClient,
    providers: Vec<ProviderEntry>,
    audit: Audit,
    agent: Agent,
    resolver: Arc<SymbolResolver>,
    clock: ServerClock,
    /// Rule 7: set when the config says DEMO but the account reports a real trade mode.
    mode_mismatch: AtomicBool,
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
        mut slots: HashMap<String, ProviderSlot>,
    ) -> Self {
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
        let tools = ToolRegistry::new(bridge.clone(), resolver.clone(), clock.clone());
        let agent = Agent::new(tools, audit.clone(), resolver.alias_list(), clock.clone());
        Self(Arc::new(Inner {
            config,
            bridge,
            providers,
            audit,
            agent,
            resolver,
            clock,
            mode_mismatch: AtomicBool::new(false),
        }))
    }

    /// Production wiring: bridge client, providers via `build_provider`, audit database.
    pub async fn build(config: Config) -> anyhow::Result<Self> {
        let bridge = BridgeClient::new(BridgeConfig {
            endpoint: config.bridge.endpoint.clone(),
            timeout: Duration::from_millis(config.bridge.timeout_ms),
        })?;
        let audit = Audit::connect(&config.database.url).await?;
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
        Ok(Self::new(config, bridge, audit, slots))
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
    pub fn providers(&self) -> &[ProviderEntry] {
        &self.0.providers
    }
    pub fn provider(&self, name: &str) -> Option<&ProviderEntry> {
        self.0.providers.iter().find(|p| p.config.name == name)
    }

    /// True when DEMO mode is configured but the account reports a real trade mode.
    pub fn mode_mismatch(&self) -> bool {
        self.0.mode_mismatch.load(Ordering::SeqCst)
    }

    /// Record the account trade mode seen on a successful bridge call (rule 7).
    pub fn observe_trade_mode(&self, seen: TradeMode) {
        let mismatch = self.0.config.mode == Mode::Demo && seen == TradeMode::Real;
        let was = self.0.mode_mismatch.swap(mismatch, Ordering::SeqCst);
        if mismatch && !was {
            tracing::error!(
                "MODE MISMATCH: config mode is demo but the MT5 account trade_mode is real. \
                 Chat is disabled until the account or the config is fixed."
            );
        } else if !mismatch && was {
            tracing::warn!("mode mismatch cleared: account trade_mode is now {seen:?}");
        }
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
