//! MT5 server clock.
//!
//! MT5 timestamps (deal times, `get_history` ranges, `PingInfo::server_time`) are *server
//! time* Unix seconds: the broker's wall clock read as if it were UTC. The offset between
//! server time and real UTC is learned from every successful `ping`.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use chrono::{DateTime, Utc};

/// Wall-clock source (Unix milliseconds) for token windows, rate limits and event
/// timestamps. Injected so tests can move time without sleeping.
pub trait TimeSource: Send + Sync {
    fn now_ms(&self) -> i64;
}

/// The real clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct WallClock;

impl TimeSource for WallClock {
    fn now_ms(&self) -> i64 {
        Utc::now().timestamp_millis()
    }
}

/// A clock that only moves when told to. Cheap to clone; clones share the same time.
#[derive(Debug, Clone)]
pub struct ManualClock(Arc<AtomicI64>);

impl ManualClock {
    pub fn new(start_ms: i64) -> Self {
        Self(Arc::new(AtomicI64::new(start_ms)))
    }

    /// A manual clock starting at the real current time.
    pub fn starting_now() -> Self {
        Self::new(Utc::now().timestamp_millis())
    }

    pub fn advance_ms(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }

    pub fn set_ms(&self, ms: i64) {
        self.0.store(ms, Ordering::SeqCst);
    }
}

impl TimeSource for ManualClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Offsets are rounded to quarter hours (the finest real-world timezone granularity).
const OFFSET_STEP_SECS: i64 = 15 * 60;

/// Shared, cheap-to-clone view of `server_time - utc_now`.
#[derive(Debug, Clone, Default)]
pub struct ServerClock(Arc<AtomicI64>);

impl ServerClock {
    /// Learn the offset from a ping's `server_time`, measured against `utc_now`.
    pub fn observe(&self, server_time: i64, utc_now: i64) {
        self.0
            .store(round_offset(server_time - utc_now), Ordering::Relaxed);
    }

    /// Seconds to add to a real UTC timestamp to get server time.
    pub fn offset_secs(&self) -> i64 {
        self.0.load(Ordering::Relaxed)
    }

    /// The current server wall clock, as a UTC-labelled datetime.
    pub fn now_server(&self) -> DateTime<Utc> {
        DateTime::from_timestamp(Utc::now().timestamp() + self.offset_secs(), 0)
            .unwrap_or_else(Utc::now)
    }
}

fn round_offset(raw: i64) -> i64 {
    // Round half away from zero to the nearest quarter hour.
    let half = OFFSET_STEP_SECS / 2;
    let rounded = if raw >= 0 {
        (raw + half) / OFFSET_STEP_SECS
    } else {
        (raw - half) / OFFSET_STEP_SECS
    };
    rounded * OFFSET_STEP_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_rounds_to_quarter_hours() {
        let c = ServerClock::default();
        assert_eq!(c.offset_secs(), 0);
        // Server is UTC+3, plus 4 s of ping latency/skew.
        c.observe(1_000_000 + 3 * 3600 + 4, 1_000_000);
        assert_eq!(c.offset_secs(), 3 * 3600);
        c.observe(1_000_000 - 5 * 3600 - 20, 1_000_000);
        assert_eq!(c.offset_secs(), -5 * 3600);
        c.observe(1_000_000 + 5400 + 100, 1_000_000);
        assert_eq!(c.offset_secs(), 5400);
        c.observe(1_000_000 + 30, 1_000_000);
        assert_eq!(c.offset_secs(), 0);
    }

    #[test]
    fn now_server_applies_the_offset() {
        let c = ServerClock::default();
        c.observe(3 * 3600, 0);
        let diff = c.now_server().timestamp() - Utc::now().timestamp();
        assert!((3 * 3600 - 2..=3 * 3600 + 2).contains(&diff));
    }
}
