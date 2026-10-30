//! Broker identity and symbol maps.
//!
//! Arc MT speaks **canonical** instrument names (`XAUUSD`, `US100`, ...); each broker names
//! them differently. [`BrokerBook`] is the shared, in-memory state the resolver reads on
//! every lookup: the instrument catalog, the three map layers, the broker the bridge last
//! reported and the symbol catalog snapshot of every broker seen. [`BrokerService`] keeps the
//! book current: it notices an account switch (`get_broker` on every ping), fetches the
//! broker's catalog, runs the auto-mapper, saves the generated layer and a catalog snapshot,
//! audits it and tells the browsers.
//!
//! Resolution order for a name: instrument catalog (alias/canonical) -> user override >
//! curated > generated map of the current broker -> the name unchanged. The configured
//! `[symbols] suffix` is appended only when the current broker has no map in any layer (or
//! is not known at all).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, SecondsFormat, Utc};
use jarvis_bridge_client::protocol::{BrokerInfo, CatalogSymbol};
use jarvis_bridge_client::{BridgeClient, BridgeError};
use jarvis_symbols::{
    AssetClass, AutomapResult, BrokerMap, Candidate, InstrumentCatalog, Registry, Source, automap,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::audit::{Audit, AuditKind};
use crate::config::Config;
use crate::events::{Event, EventBus};
use crate::symbols::clean;

/// After a failed detection for a broker, the next automatic attempt waits this long.
const DETECT_RETRY: Duration = Duration::from_secs(30);

const GENERATED_HEADER: &str = "Written by Arc MT when it detects a broker's symbols. Safe to \
delete (it is rebuilt); edits are overwritten. Make permanent changes in Settings or in \
config/broker_symbols.toml.";
const USER_HEADER: &str = "Written by Arc MT when you change a symbol mapping in Settings. \
Entries here win over every other map.";

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// Which broker account the bridge reported: the company (the key of a symbol map) and the
/// trade server. A preview is only valid for the account it was made on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrokerId {
    pub company: String,
    pub server: String,
}

impl BrokerId {
    pub fn of(info: &BrokerInfo) -> Self {
        Self {
            company: info.company.clone(),
            server: info.server.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Catalog snapshot
// ---------------------------------------------------------------------------

/// A broker's symbol catalog as last fetched, with what the auto-mapper made of it.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub company: String,
    pub server: String,
    pub detected_at: DateTime<Utc>,
    pub symbols: Vec<CatalogSymbol>,
    pub candidates: BTreeMap<String, Vec<Candidate>>,
    pub detected_suffix: String,
    pub detected_prefix: String,
    pub matched: usize,
    pub total: usize,
    /// Read back from disk, not fetched in this run: good enough to validate an override,
    /// but detection still runs.
    pub stale: bool,
    /// upper-case name -> index into `symbols`.
    by_upper: HashMap<String, usize>,
    exact: HashSet<String>,
}

impl Snapshot {
    fn new(
        info: &BrokerInfo,
        symbols: Vec<CatalogSymbol>,
        result: &AutomapResult,
        detected_at: DateTime<Utc>,
        stale: bool,
    ) -> Self {
        let mut by_upper = HashMap::with_capacity(symbols.len());
        let mut exact = HashSet::with_capacity(symbols.len());
        for (i, s) in symbols.iter().enumerate() {
            by_upper.entry(s.name.to_ascii_uppercase()).or_insert(i);
            exact.insert(s.name.clone());
        }
        Self {
            company: info.company.clone(),
            server: info.server.clone(),
            detected_at,
            candidates: result.candidates.clone(),
            detected_suffix: result.detected_suffix.clone(),
            detected_prefix: result.detected_prefix.clone(),
            matched: result.matched,
            total: result.total,
            symbols,
            stale,
            by_upper,
            exact,
        }
    }

    /// The broker's own spelling of `name` (case-insensitive).
    pub fn lookup(&self, name: &str) -> Option<&str> {
        self.by_upper
            .get(&name.to_ascii_uppercase())
            .map(|i| self.symbols[*i].name.as_str())
    }

    /// True when the catalog has exactly this name.
    pub fn contains(&self, name: &str) -> bool {
        self.exact.contains(name)
    }

    /// Case-insensitive search over name and description. Returns the first `limit` matches
    /// and the number of matches.
    pub fn search(&self, query: &str, limit: usize) -> (Vec<&CatalogSymbol>, usize) {
        let q = query.trim().to_lowercase();
        let mut total = 0;
        let mut out = Vec::new();
        for s in &self.symbols {
            let hit = q.is_empty()
                || s.name.to_lowercase().contains(&q)
                || s.description.to_lowercase().contains(&q);
            if hit {
                total += 1;
                if out.len() < limit {
                    out.push(s);
                }
            }
        }
        (out, total)
    }
}

/// The on-disk form of a [`Snapshot`] (`data/broker-catalogs/<slug>.json`).
#[derive(Debug, Serialize, Deserialize)]
struct SnapshotFile {
    company: String,
    server: String,
    detected_at: String,
    #[serde(default)]
    detected_suffix: String,
    #[serde(default)]
    detected_prefix: String,
    #[serde(default)]
    matched: usize,
    #[serde(default)]
    total: usize,
    #[serde(default)]
    candidates: BTreeMap<String, Vec<Candidate>>,
    symbols: Vec<CatalogSymbol>,
}

/// File-name-safe form of a company name: `CFI Financial Markets L.L.C` ->
/// `cfi-financial-markets-l-l-c`.
pub fn slug(company: &str) -> String {
    let mut out = String::new();
    let mut dash = true;
    for c in company.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() {
        "broker".to_owned()
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// The book
// ---------------------------------------------------------------------------

/// What a name resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub canonical: String,
    pub broker: String,
    pub source: Source,
}

#[derive(Default)]
struct BookInner {
    registry: Registry,
    current: Option<BrokerInfo>,
    /// By company.
    snapshots: HashMap<String, Arc<Snapshot>>,
}

impl BookInner {
    fn has_map(&self, company: &str) -> bool {
        self.registry.user.contains_key(company)
            || self.registry.curated.contains_key(company)
            || self.registry.generated.contains_key(company)
    }
}

/// One row of `GET /api/symbols`.
#[derive(Debug, Clone, Serialize)]
pub struct MappingRow {
    pub canonical: String,
    pub class: AssetClass,
    pub aliases: Vec<String>,
    pub broker_symbol: String,
    pub source: Source,
    pub available: bool,
    pub candidates: Vec<Candidate>,
}

/// The shared symbol state. Cheap to read; every method takes the lock only briefly.
pub struct BrokerBook {
    catalog: Option<InstrumentCatalog>,
    suffix: String,
    inner: RwLock<BookInner>,
}

impl BrokerBook {
    pub fn new(catalog: Option<InstrumentCatalog>, registry: Registry, suffix: &str) -> Self {
        Self {
            catalog,
            suffix: suffix.trim().to_owned(),
            inner: RwLock::new(BookInner {
                registry,
                current: None,
                snapshots: HashMap::new(),
            }),
        }
    }

    /// A book with no instrument catalog and no maps: names pass through, with `suffix`
    /// appended. What the resolver used to do; also the degraded mode when the maps cannot
    /// be loaded.
    pub fn standalone(suffix: &str) -> Arc<Self> {
        Arc::new(Self::new(None, Registry::default(), suffix))
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, BookInner> {
        self.inner.read().unwrap_or_else(|p| p.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, BookInner> {
        self.inner.write().unwrap_or_else(|p| p.into_inner())
    }

    pub fn catalog(&self) -> Option<&InstrumentCatalog> {
        self.catalog.as_ref()
    }

    /// The configured last-resort suffix.
    pub fn suffix(&self) -> &str {
        &self.suffix
    }

    pub fn current(&self) -> Option<BrokerInfo> {
        self.read().current.clone()
    }

    pub fn identity(&self) -> Option<BrokerId> {
        self.read().current.as_ref().map(BrokerId::of)
    }

    /// The snapshot of the current broker.
    pub fn snapshot(&self) -> Option<Arc<Snapshot>> {
        let inner = self.read();
        let company = &inner.current.as_ref()?.company;
        inner.snapshots.get(company).cloned()
    }

    fn snapshot_of(&self, company: &str) -> Option<Arc<Snapshot>> {
        self.read().snapshots.get(company).cloned()
    }

    /// Record the broker the bridge reports. Returns the previous one.
    fn set_current(&self, info: BrokerInfo) -> Option<BrokerInfo> {
        self.write().current.replace(info)
    }

    fn put_snapshot(&self, snapshot: Snapshot) {
        let mut inner = self.write();
        inner
            .snapshots
            .insert(snapshot.company.clone(), Arc::new(snapshot));
    }

    /// Canonical name for a canonical name or generic alias.
    pub fn canonical_for(&self, name: &str) -> Option<String> {
        self.catalog.as_ref()?.canonical_for(name)
    }

    /// The configured suffix appended to `base` (once).
    fn with_suffix(&self, base: &str) -> String {
        if self.suffix.is_empty() || base.is_empty() {
            return base.to_owned();
        }
        let suffix_upper = self.suffix.to_ascii_uppercase();
        match base.strip_suffix(&suffix_upper) {
            Some("") | None => format!("{base}{}", self.suffix),
            Some(stem) => format!("{stem}{}", self.suffix),
        }
    }

    /// Resolve a cleaned (upper-case) name to the current broker's symbol.
    pub fn resolve(&self, name: &str) -> Resolved {
        let inner = self.read();
        let canonical = self.canonical_for(name);
        let Some(info) = &inner.current else {
            // No broker known yet: the old behaviour.
            let base = canonical.unwrap_or_else(|| self.strip_suffix_canonical(name));
            return Resolved {
                broker: self.with_suffix(&base),
                canonical: base,
                source: Source::Passthrough,
            };
        };
        let company = info.company.as_str();
        let snapshot = inner.snapshots.get(company);
        let map_known = inner.has_map(company);

        if let (Some(canon), Some(cat)) = (&canonical, &self.catalog) {
            let res = inner.registry.resolve(company, canon, cat);
            let mut broker = res.broker_symbol;
            if res.source == Source::Passthrough && !map_known {
                broker = self.with_suffix(&broker);
            }
            return Resolved {
                canonical: canon.clone(),
                broker,
                source: res.source,
            };
        }

        // Not an instrument of the catalog. The user may have typed the broker's own name.
        if let Some(real) = snapshot.and_then(|s| s.lookup(name)) {
            return Resolved {
                canonical: self.reverse_locked(&inner, company, real),
                broker: real.to_owned(),
                source: Source::Passthrough,
            };
        }
        if let Some(cat) = &self.catalog {
            let res = inner.registry.resolve(company, name, cat);
            if res.source != Source::Passthrough
                && snapshot.is_none_or(|s| s.contains(&res.broker_symbol))
            {
                return Resolved {
                    canonical: name.to_owned(),
                    broker: res.broker_symbol,
                    source: res.source,
                };
            }
        }
        let canon = self.strip_suffix_canonical(name);
        let broker = if map_known {
            name.to_owned()
        } else {
            self.with_suffix(&canon)
        };
        Resolved {
            canonical: canon,
            broker,
            source: Source::Passthrough,
        }
    }

    fn reverse_locked(&self, inner: &BookInner, company: &str, broker_symbol: &str) -> String {
        if let Some(cat) = &self.catalog
            && let Some(c) = inner.registry.reverse(company, broker_symbol, cat)
        {
            return c;
        }
        self.strip_suffix_canonical(broker_symbol)
    }

    /// `XAUUSD.m` -> `XAUUSD` for the configured suffix; names the catalog knows are
    /// canonicalised, others kept (upper case).
    fn strip_suffix_canonical(&self, name: &str) -> String {
        let up = name.to_ascii_uppercase();
        if let Some(c) = self.canonical_for(&up) {
            return c;
        }
        if !self.suffix.is_empty() {
            let suffix_upper = self.suffix.to_ascii_uppercase();
            if let Some(stem) = up.strip_suffix(&suffix_upper)
                && !stem.is_empty()
            {
                return self.canonical_for(stem).unwrap_or_else(|| stem.to_owned());
            }
        }
        up
    }

    /// Canonical name for a broker symbol of the current broker (`XAUUSD_` -> `XAUUSD`).
    /// Unknown names come back upper-cased, so display never loses a row.
    pub fn canonical_of_broker_symbol(&self, broker_symbol: &str) -> String {
        let inner = self.read();
        match &inner.current {
            Some(info) => self.reverse_locked(&inner, &info.company, broker_symbol),
            None => self.strip_suffix_canonical(broker_symbol),
        }
    }

    /// Canonical form of a name from a config file, a stored setting or user input: aliases
    /// resolve, the broker's own spelling reverses, and the legacy suffix is stripped.
    pub fn canonical_loose(&self, name: &str) -> Option<String> {
        let cleaned = clean(name);
        if cleaned.is_empty() {
            return None;
        }
        if let Some(c) = self.canonical_for(&cleaned) {
            return Some(c);
        }
        let inner = self.read();
        if let Some(info) = &inner.current {
            let real = inner
                .snapshots
                .get(&info.company)
                .and_then(|s| s.lookup(&cleaned))
                .unwrap_or(name.trim());
            if let Some(cat) = &self.catalog
                && let Some(c) = inner.registry.reverse(&info.company, real, cat)
            {
                return Some(c);
            }
        }
        Some(self.strip_suffix_canonical(&cleaned))
    }

    /// The current broker's symbols for these canonical names (for the gate's exact
    /// comparison). Order kept, duplicates dropped.
    pub fn broker_symbols_for(&self, canonicals: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::with_capacity(canonicals.len());
        for c in canonicals {
            let cleaned = clean(c);
            if cleaned.is_empty() {
                continue;
            }
            let symbol = self.resolve(&cleaned).broker;
            if !out.contains(&symbol) {
                out.push(symbol);
            }
        }
        out
    }

    /// Every instrument with its resolution for the current broker.
    pub fn mappings(&self) -> Vec<MappingRow> {
        let Some(cat) = &self.catalog else {
            return Vec::new();
        };
        let snapshot = self.snapshot();
        cat.instruments()
            .iter()
            .map(|inst| {
                let res = self.resolve(&inst.canonical);
                let available = snapshot.as_ref().is_none_or(|s| s.contains(&res.broker));
                MappingRow {
                    candidates: snapshot
                        .as_ref()
                        .and_then(|s| s.candidates.get(&inst.canonical).cloned())
                        .unwrap_or_default(),
                    canonical: inst.canonical.clone(),
                    class: inst.class,
                    aliases: inst.aliases.clone(),
                    broker_symbol: res.broker,
                    source: res.source,
                    available,
                }
            })
            .collect()
    }

    /// `(mapped, total)` instruments: mapped means resolved to a symbol the broker has.
    pub fn mapped_counts(&self) -> (usize, usize) {
        let rows = self.mappings();
        let mapped = rows.iter().filter(|r| r.available).count();
        (mapped, rows.len())
    }

    /// Set (`Some`) or clear (`None`) a user override and return the user layer to save.
    fn set_user_override(
        &self,
        company: &str,
        canonical: &str,
        broker_symbol: Option<&str>,
    ) -> BTreeMap<String, BrokerMap> {
        let mut inner = self.write();
        inner
            .registry
            .set_user_override(company, canonical, broker_symbol);
        inner.registry.user.clone()
    }

    /// The user's override for `canonical` on `company`, if any.
    fn user_override(&self, company: &str, canonical: &str) -> Option<String> {
        self.read()
            .registry
            .user
            .get(company)
            .and_then(|m| m.explicit.get(canonical).cloned())
    }

    /// Install a fresh detection: the generated layer of `company` and its snapshot.
    fn install_detection(
        &self,
        company: &str,
        map: BrokerMap,
        snapshot: Snapshot,
    ) -> BTreeMap<String, BrokerMap> {
        let mut inner = self.write();
        inner.registry.set_generated(company, map);
        inner
            .snapshots
            .insert(company.to_owned(), Arc::new(snapshot));
        inner.registry.generated.clone()
    }

    /// The effective map for the export: instruments that resolved to something the
    /// broker has (or, with no catalog to check against, to anything but a plain
    /// pass-through).
    pub fn export_map(&self) -> BTreeMap<String, String> {
        self.mappings()
            .into_iter()
            .filter(|r| r.available)
            .map(|r| (r.canonical, r.broker_symbol))
            .collect()
    }

    /// Instruments with no broker symbol: they resolve to a name the broker does not have.
    pub fn unmapped(&self) -> Vec<String> {
        let Some(snapshot) = self.snapshot() else {
            return Vec::new();
        };
        self.mappings()
            .into_iter()
            .filter(|r| !snapshot.contains(&r.broker_symbol))
            .map(|r| r.canonical)
            .collect()
    }

    /// `broker` object of `GET /api/symbols`; `null` while no broker was ever seen.
    pub fn broker_view(&self) -> Value {
        let Some(info) = self.current() else {
            return Value::Null;
        };
        let snapshot = self.snapshot_of(&info.company);
        json!({
            "company": info.company,
            "server": info.server,
            "detected_at": snapshot
                .as_ref()
                .map(|s| s.detected_at.to_rfc3339_opts(SecondsFormat::Secs, true)),
            "catalog_size": snapshot.as_ref().map_or(0, |s| s.symbols.len()),
            "detected_suffix": snapshot.as_ref().map_or("", |s| s.detected_suffix.as_str()),
            "detected_prefix": snapshot.as_ref().map_or("", |s| s.detected_prefix.as_str()),
        })
    }

    /// `GET /api/symbols` body.
    pub fn view(&self) -> Value {
        json!({
            "broker": self.broker_view(),
            "mappings": self.mappings(),
            "unmapped": self.unmapped(),
        })
    }

    /// `broker` object of `GET /api/status`.
    pub fn status_view(&self) -> Value {
        let Some(info) = self.current() else {
            return Value::Null;
        };
        let (mapped, total) = self.mapped_counts();
        json!({
            "company": info.company,
            "server": info.server,
            "mapped": mapped,
            "total": total,
        })
    }

    /// The `/api/symbols/export` body.
    pub fn export_view(&self) -> Option<Value> {
        let info = self.current()?;
        let generated_at = self
            .snapshot_of(&info.company)
            .map_or_else(Utc::now, |s| s.detected_at);
        Some(json!({
            "broker": info.company,
            "server": info.server,
            "generated_at": generated_at.to_rfc3339_opts(SecondsFormat::Secs, true),
            "map": self.export_map(),
        }))
    }
}

// ---------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------

/// Where the machine-written files live.
#[derive(Debug, Clone)]
pub struct DataPaths {
    pub generated: PathBuf,
    pub user: PathBuf,
    pub catalogs: PathBuf,
}

impl DataPaths {
    pub fn from_dir(dir: &Path) -> Self {
        Self {
            generated: dir.join("broker_symbols.generated.toml"),
            user: dir.join("broker_symbols.user.toml"),
            catalogs: dir.join("broker-catalogs"),
        }
    }
}

/// Why a detection or an override failed.
#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("the broker could not be read: {0}")]
    Bridge(BridgeError),
    #[error("the instrument catalog is not loaded, so symbols cannot be mapped")]
    NoCatalog,
    #[error("no broker is known yet; is MetaTrader 5 running with the EA?")]
    NoBroker,
    #[error("the account changed while its symbols were being read")]
    ChangedDuringDetection,
    /// The account changed between checking an override and applying it.
    #[error("the broker account changed; nothing was saved")]
    AccountChanged,
    /// The override names a symbol the broker's catalog does not have.
    #[error("{0}")]
    UnknownBrokerSymbol(String),
    /// The canonical name is not an instrument.
    #[error("{0}")]
    UnknownInstrument(String),
    #[error("could not save the map: {0}")]
    Save(String),
}

impl From<BridgeError> for BrokerError {
    fn from(e: BridgeError) -> Self {
        BrokerError::Bridge(e)
    }
}

/// Why a detection ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// A ping saw a broker it has no fresh map for.
    Auto,
    /// `POST /api/symbols/detect`.
    Manual,
}

impl Trigger {
    fn as_str(self) -> &'static str {
        match self {
            Trigger::Auto => "auto",
            Trigger::Manual => "manual",
        }
    }
}

pub struct BrokerParts {
    pub book: Arc<BrokerBook>,
    pub bridge: BridgeClient,
    pub audit: Audit,
    pub events: EventBus,
    pub paths: DataPaths,
    pub auto_detect: bool,
}

/// Keeps the [`BrokerBook`] in step with the account MT5 is logged into.
pub struct BrokerService {
    book: Arc<BrokerBook>,
    bridge: BridgeClient,
    audit: Audit,
    events: EventBus,
    paths: DataPaths,
    auto_detect: bool,
    /// Only one detection runs at a time.
    detect_lock: tokio::sync::Mutex<()>,
    /// When an automatic attempt for a company last started.
    attempts: Mutex<HashMap<String, Instant>>,
    /// Serialises writes of the map files.
    file_lock: Mutex<()>,
    active: AtomicUsize,
    max_active: AtomicUsize,
    completed: AtomicUsize,
}

impl BrokerService {
    pub fn new(parts: BrokerParts) -> Arc<Self> {
        Arc::new(Self {
            book: parts.book,
            bridge: parts.bridge,
            audit: parts.audit,
            events: parts.events,
            paths: parts.paths,
            auto_detect: parts.auto_detect,
            detect_lock: tokio::sync::Mutex::new(()),
            attempts: Mutex::new(HashMap::new()),
            file_lock: Mutex::new(()),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
        })
    }

    pub fn book(&self) -> &Arc<BrokerBook> {
        &self.book
    }

    /// `(detections finished, most that ever ran at once)`. The second number is 1 unless
    /// the one-at-a-time rule is broken.
    pub fn detection_stats(&self) -> (usize, usize) {
        (
            self.completed.load(Ordering::SeqCst),
            self.max_active.load(Ordering::SeqCst),
        )
    }

    /// Ask the bridge which broker it is on and react: remember it, and (re)detect when it is
    /// a broker there is no fresh map for. Never fails: without an answer the previous broker
    /// stays current. A detection already running is not waited for.
    pub async fn observe(&self) -> Option<BrokerInfo> {
        let info = match self.bridge.get_broker().await {
            Ok(info) => info,
            Err(e) => {
                tracing::debug!("get_broker failed: {e}");
                return self.book.current();
            }
        };
        let previous = self.book.set_current(info.clone());
        let company_changed = previous.as_ref().is_none_or(|p| p.company != info.company);
        if company_changed {
            tracing::info!(
                company = %info.company,
                server = %info.server,
                "broker account changed"
            );
            if self.book.snapshot_of(&info.company).is_none() {
                self.load_snapshot(&info).await;
            }
        }
        if self.auto_detect && self.needs_detection(&info.company) {
            self.detect_auto(&info).await;
        }
        // The first sight of a broker is announced by its detection; a switch is announced here
        // too, because the map in use changed even if the detection is still to come.
        if company_changed && previous.is_some() && self.book.catalog().is_some() {
            self.events.emit(Event::SymbolMapChanged {
                broker: info.company.clone(),
            });
        }
        Some(info)
    }

    fn needs_detection(&self, company: &str) -> bool {
        if self.book.catalog().is_none() {
            return false;
        }
        if self.book.snapshot_of(company).is_some_and(|s| !s.stale) {
            return false;
        }
        let attempts = self.attempts.lock().unwrap_or_else(|p| p.into_inner());
        attempts
            .get(company)
            .is_none_or(|at| at.elapsed() >= DETECT_RETRY)
    }

    async fn detect_auto(&self, info: &BrokerInfo) {
        // Someone else is detecting: use the previous map meanwhile (never block a turn).
        let Ok(guard) = self.detect_lock.try_lock() else {
            return;
        };
        self.attempts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(info.company.clone(), Instant::now());
        if let Err(e) = self.detect_locked(Trigger::Auto).await {
            tracing::warn!(company = %info.company, "symbol detection failed: {e}");
        }
        drop(guard);
    }

    /// Detect the current broker's symbols now, waiting for a detection already running.
    pub async fn detect(&self) -> Result<(), BrokerError> {
        let _guard = self.detect_lock.lock().await;
        self.detect_locked(Trigger::Manual).await
    }

    /// Body of a detection; the caller holds `detect_lock`.
    async fn detect_locked(&self, trigger: Trigger) -> Result<(), BrokerError> {
        struct Active<'a>(&'a BrokerService);
        impl Drop for Active<'_> {
            fn drop(&mut self) {
                self.0.active.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let now_active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now_active, Ordering::SeqCst);
        let _active = Active(self);

        let catalog = self.book.catalog().ok_or(BrokerError::NoCatalog)?;
        let info = self.bridge.get_broker().await?;
        self.book.set_current(info.clone());
        let symbols = self.bridge.list_all_symbols().await?;
        // A switch while the pages were read would mix two brokers' symbols.
        let after = self.bridge.get_broker().await?;
        if after.company != info.company {
            self.book.set_current(after);
            return Err(BrokerError::ChangedDuringDetection);
        }

        let result = automap(&info.company, &symbols, catalog);
        let snapshot = Snapshot::new(&info, symbols, &result, Utc::now(), false);
        let file = snapshot_file(&snapshot);
        let layer = self
            .book
            .install_detection(&info.company, result.map.clone(), snapshot);

        self.save_layer(&layer, &self.paths.generated, GENERATED_HEADER);
        self.write_snapshot(&info.company, &file).await;

        self.audit
            .record(
                None,
                AuditKind::SymbolMapDetected,
                json!({
                    "company": info.company,
                    "server": info.server,
                    "matched": result.matched,
                    "total": result.total,
                    "suffix": result.detected_suffix,
                    "prefix": result.detected_prefix,
                    "trigger": trigger.as_str(),
                }),
            )
            .await;
        self.events.emit(Event::SymbolMapChanged {
            broker: info.company.clone(),
        });
        self.completed.fetch_add(1, Ordering::SeqCst);
        tracing::info!(
            company = %info.company,
            matched = result.matched,
            total = result.total,
            "broker symbols detected"
        );
        Ok(())
    }

    /// Save one layer file; a failure is logged (the in-memory map keeps working).
    fn save_layer(&self, layer: &BTreeMap<String, BrokerMap>, path: &Path, header: &str) {
        let _guard = self.file_lock.lock().unwrap_or_else(|p| p.into_inner());
        if let Err(e) = Registry::save_layer(layer, path, header) {
            tracing::error!("saving {} failed: {e}", path.display());
        }
    }

    async fn write_snapshot(&self, company: &str, file: &SnapshotFile) {
        let path = self.paths.catalogs.join(format!("{}.json", slug(company)));
        let result = async {
            tokio::fs::create_dir_all(&self.paths.catalogs).await?;
            let bytes = serde_json::to_vec(file).map_err(std::io::Error::other)?;
            let tmp = path.with_extension("json.tmp");
            tokio::fs::write(&tmp, bytes).await?;
            tokio::fs::rename(&tmp, &path).await
        }
        .await;
        if let Err(e) = result {
            tracing::error!("saving the catalog snapshot {} failed: {e}", path.display());
        }
    }

    /// Read a saved snapshot of `info.company` (marked stale) so overrides can be checked
    /// before the first detection of this run finishes.
    async fn load_snapshot(&self, info: &BrokerInfo) {
        let path = self
            .paths
            .catalogs
            .join(format!("{}.json", slug(&info.company)));
        let Ok(bytes) = tokio::fs::read(&path).await else {
            return;
        };
        let file: SnapshotFile = match serde_json::from_slice(&bytes) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("ignoring unreadable snapshot {}: {e}", path.display());
                return;
            }
        };
        if file.company != info.company {
            return;
        }
        let detected_at = DateTime::parse_from_rfc3339(&file.detected_at)
            .map_or_else(|_| Utc::now(), |d| d.with_timezone(&Utc));
        let result = AutomapResult {
            broker: file.company.clone(),
            map: BrokerMap::default(),
            candidates: file.candidates,
            detected_suffix: file.detected_suffix,
            detected_prefix: file.detected_prefix,
            matched: file.matched,
            total: file.total,
        };
        let snapshot = Snapshot::new(
            &BrokerInfo {
                company: file.company,
                server: file.server,
                trade_mode: info.trade_mode,
            },
            file.symbols,
            &result,
            detected_at,
            true,
        );
        self.book.put_snapshot(snapshot);
    }

    /// Check an override request without applying it: `canonical` must be an instrument and
    /// `broker_symbol` (when given) a symbol of the current broker's catalog snapshot.
    pub fn prepare_override(
        &self,
        canonical: &str,
        broker_symbol: Option<&str>,
    ) -> Result<PreparedOverride, BrokerError> {
        let cat = self.book.catalog().ok_or(BrokerError::NoCatalog)?;
        let canonical = cat.canonical_for(canonical).ok_or_else(|| {
            BrokerError::UnknownInstrument(format!("'{canonical}' is not a known instrument"))
        })?;
        let info = self.book.current().ok_or(BrokerError::NoBroker)?;
        let chosen = match broker_symbol {
            None => None,
            Some(name) => {
                let name = name.trim();
                let snapshot = self.book.snapshot_of(&info.company).ok_or_else(|| {
                    BrokerError::UnknownBrokerSymbol(format!(
                        "'{name}' cannot be checked: the symbols of {} have not been read yet",
                        info.company
                    ))
                })?;
                if !snapshot.contains(name) {
                    return Err(BrokerError::UnknownBrokerSymbol(format!(
                        "'{name}' is not a symbol of {}",
                        info.company
                    )));
                }
                Some(name.to_owned())
            }
        };
        Ok(PreparedOverride {
            id: BrokerId::of(&info),
            canonical,
            chosen,
        })
    }

    /// Apply a checked override: save the user layer, audit, tell the browsers.
    pub async fn apply_override(
        &self,
        prepared: PreparedOverride,
        by: &str,
    ) -> Result<(), BrokerError> {
        let PreparedOverride {
            id,
            canonical,
            chosen,
        } = prepared;
        // The account must still be the one the request was checked against.
        if self.book.identity().as_ref() != Some(&id) {
            return Err(BrokerError::AccountChanged);
        }
        let previous = self.book.user_override(&id.company, &canonical);
        let layer = self
            .book
            .set_user_override(&id.company, &canonical, chosen.as_deref());
        {
            let _guard = self.file_lock.lock().unwrap_or_else(|p| p.into_inner());
            Registry::save_layer(&layer, &self.paths.user, USER_HEADER)
                .map_err(|e| BrokerError::Save(e.to_string()))?;
        }
        self.audit
            .record(
                None,
                AuditKind::SymbolMapOverride,
                json!({
                    "by": by,
                    "company": id.company,
                    "server": id.server,
                    "canonical": canonical,
                    "broker_symbol": chosen,
                    "previous": previous,
                }),
            )
            .await;
        self.events
            .emit(Event::SymbolMapChanged { broker: id.company });
        Ok(())
    }
}

/// An override that passed [`BrokerService::prepare_override`].
#[derive(Debug, Clone)]
pub struct PreparedOverride {
    id: BrokerId,
    canonical: String,
    chosen: Option<String>,
}

fn snapshot_file(s: &Snapshot) -> SnapshotFile {
    SnapshotFile {
        company: s.company.clone(),
        server: s.server.clone(),
        detected_at: s.detected_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        detected_suffix: s.detected_suffix.clone(),
        detected_prefix: s.detected_prefix.clone(),
        matched: s.matched,
        total: s.total,
        candidates: s.candidates.clone(),
        symbols: s.symbols.clone(),
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// The instrument catalog and the three map layers named by `[symbols]`.
pub fn load_maps(config: &Config) -> anyhow::Result<(InstrumentCatalog, Registry)> {
    let s = &config.symbols;
    let catalog = InstrumentCatalog::load(&s.instruments)
        .map_err(|e| anyhow::anyhow!("[symbols] instruments: {e}"))?;
    let paths = DataPaths::from_dir(&s.data_dir);
    let registry = Registry::load(&s.broker_map, &paths.generated, &paths.user)
        .map_err(|e| anyhow::anyhow!("[symbols] broker_map / data_dir: {e}"))?;
    Ok((catalog, registry))
}

/// The book for a config. When the maps cannot be loaded the book is degraded (names pass
/// through, the suffix applies) and the reason is logged.
pub fn book_from_config(config: &Config) -> Arc<BrokerBook> {
    match load_maps(config) {
        Ok((catalog, registry)) => Arc::new(BrokerBook::new(
            Some(catalog),
            registry,
            &config.symbols.suffix,
        )),
        Err(e) => {
            tracing::warn!(
                "symbol maps are not loaded ({e:#}); names pass through unchanged and detection \
                 is off"
            );
            BrokerBook::standalone(&config.symbols.suffix)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_file_name_safe() {
        assert_eq!(
            slug("CFI Financial Markets L.L.C"),
            "cfi-financial-markets-l-l-c"
        );
        assert_eq!(slug("Raw Trading Ltd"), "raw-trading-ltd");
        assert_eq!(slug("../../etc/passwd"), "etc-passwd");
        assert_eq!(slug("   "), "broker");
        assert_eq!(slug("日本"), "broker");
    }

    #[test]
    fn standalone_book_applies_the_suffix_once() {
        let book = BrokerBook::standalone(".m");
        assert_eq!(book.resolve("XAUUSD").broker, "XAUUSD.m");
        assert_eq!(book.resolve("XAUUSD.M").broker, "XAUUSD.m");
        assert_eq!(book.canonical_of_broker_symbol("XAUUSD.m"), "XAUUSD");
        let none = BrokerBook::standalone("");
        assert_eq!(none.resolve("XAUUSD").broker, "XAUUSD");
        assert!(none.view()["broker"].is_null());
    }
}
