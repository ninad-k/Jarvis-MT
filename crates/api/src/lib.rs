//! `jarvis-api`: HTTP API, tool registry, agent loop and audit log for Jarvis MT.
//!
//! Sprint 1 is read-only: the model can read prices, account, positions, orders and
//! history, and cannot trade (SPEC rule 1).

pub mod agent;
pub mod audit;
pub mod clock;
pub mod config;
pub mod routes;
pub mod state;
pub mod symbols;
pub mod tools;

pub use routes::app;
pub use state::AppState;
