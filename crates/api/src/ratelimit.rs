//! A small in-process sliding-window rate limiter (no external crate).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

/// Allows `limit` hits per `window_ms` per key.
#[derive(Debug)]
pub struct RateLimiter {
    limit: usize,
    window_ms: i64,
    hits: Mutex<HashMap<String, VecDeque<i64>>>,
}

impl RateLimiter {
    pub fn new(limit: usize, window_ms: i64) -> Self {
        Self {
            limit,
            window_ms,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Record a hit for `key` at `now_ms`. `Err(retry_after_secs)` (at least 1) when the
    /// key is over its limit; a refused hit is not recorded.
    pub fn check(&self, key: &str, now_ms: i64) -> Result<(), u64> {
        let mut hits = self.hits.lock().unwrap_or_else(|p| p.into_inner());
        let queue = hits.entry(key.to_owned()).or_default();
        while queue.front().is_some_and(|t| now_ms - t >= self.window_ms) {
            queue.pop_front();
        }
        if queue.len() >= self.limit {
            let oldest = queue.front().copied().unwrap_or(now_ms);
            let wait_ms = (oldest + self.window_ms - now_ms).max(1);
            return Err(u64::try_from((wait_ms + 999) / 1000).unwrap_or(1).max(1));
        }
        queue.push_back(now_ms);
        Ok(())
    }

    /// Drop keys whose hits have all left the window.
    pub fn purge(&self, now_ms: i64) {
        let mut hits = self.hits.lock().unwrap_or_else(|p| p.into_inner());
        hits.retain(|_, q| q.back().is_some_and(|t| now_ms - t < self.window_ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_limit_then_refuses_with_retry_after() {
        let rl = RateLimiter::new(3, 60_000);
        for i in 0..3 {
            assert!(rl.check("a", 1_000 * i).is_ok());
        }
        assert_eq!(rl.check("a", 5_000), Err(55));
        // Another key is independent.
        assert!(rl.check("b", 5_000).is_ok());
        // The oldest hit leaves the window at t = 60_000.
        assert!(rl.check("a", 60_000).is_ok());
        assert!(rl.check("a", 60_500).is_err());
        assert!(rl.check("a", 61_000).is_ok());
    }

    #[test]
    fn refused_hits_are_not_counted_and_purge_forgets_idle_keys() {
        let rl = RateLimiter::new(1, 1_000);
        assert!(rl.check("k", 0).is_ok());
        for t in 1..10 {
            assert!(rl.check("k", t).is_err());
        }
        assert!(rl.check("k", 1_000).is_ok());
        rl.purge(10_000);
        assert!(rl.hits.lock().unwrap().is_empty());
    }
}
