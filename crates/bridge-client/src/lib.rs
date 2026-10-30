//! Rust client for the `JarvisBridge` MQL5 EA (ZeroMQ REQ/REP, JSON).
//!
//! - [`auth`]: the shared key and the message authentication of protocol 2 (HMAC-SHA256 over a
//!   two-frame message, `ts` freshness, replay protection).
//! - [`protocol`]: wire types, the single source of truth for the JSON contract.
//! - [`BridgeClient`]: async client. One request in flight at a time, per-call timeout,
//!   socket recreated after a timeout (REQ/REP is lock-step and wedges on a lost reply).
//! - `mock::MockBridge` (feature `mock`): in-process REP server with canned data.

pub mod auth;
mod client;
mod error;
pub mod protocol;

#[cfg(feature = "mock")]
pub mod mock;
#[cfg(feature = "mock")]
mod mock_catalog;
#[cfg(feature = "mock")]
mod mock_trade;

pub use auth::BridgeKey;
pub use client::{
    BridgeClient, BridgeConfig, DEFAULT_ENDPOINT, DEFAULT_TIMEOUT, MIN_WRITE_TIMEOUT,
};
pub use error::BridgeError;
