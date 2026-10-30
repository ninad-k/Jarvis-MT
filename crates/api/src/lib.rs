//! `jarvis-api`: HTTP API, tool registry, agent loop and audit log for Jarvis MT.
//!
//! The model can read prices, account, positions, orders and history, and can *propose*
//! trades (SPEC rule 1). A proposal becomes a preview; only a user confirmation
//! (`POST /api/confirm/{token}`) sends a write to the bridge (rule 3).
//!
//! The same process serves the web UI, the login (password + TOTP, JWT cookie), the
//! WebSocket event feed, the voice routes and, when configured, TLS.

pub mod agent;
pub mod audit;
pub mod auth;
pub mod cli;
pub mod clock;
pub mod config;
pub mod events;
pub mod grounding;
pub mod limit_overrides;
pub mod ratelimit;
pub mod routes;
pub mod security;
pub mod serve;
#[cfg(windows)]
pub mod service;
pub mod settings;
pub mod settings_routes;
pub mod state;
pub mod static_site;
pub mod symbols;
pub mod tools;
pub mod trading;
pub mod users;
pub mod voice;
pub mod ws;

pub use routes::app;
pub use state::AppState;
