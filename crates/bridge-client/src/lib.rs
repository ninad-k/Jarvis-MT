//! Rust client for the `JarvisBridge` MQL5 EA (ZeroMQ REQ/REP, JSON).
//!
//! - [`protocol`]: wire types, the single source of truth for the JSON contract.
//! - [`BridgeClient`]: async client. One request in flight at a time, per-call timeout,
//!   socket recreated after a timeout (REQ/REP is lock-step and wedges on a lost reply).
//! - `mock::MockBridge` (feature `mock`): in-process REP server with canned data.

mod client;
mod error;
pub mod protocol;

#[cfg(feature = "mock")]
pub mod mock;

pub use client::{BridgeClient, BridgeConfig};
pub use error::BridgeError;
