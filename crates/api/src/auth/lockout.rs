//! Failed-login bookkeeping (AUTH-1).
//!
//! Locking an account by its name alone lets anyone who knows the name lock the owner out with
//! ten bad passwords. So:
//!
//! - failures are counted per **(account, client address)**. Ten failures from one address lock
//!   that address out of that account for 15 minutes; the owner, coming from another address,
//!   is not affected;
//! - failures from all addresses together are watched per account as well. Past
//!   [`SLOW_DOWN_AFTER`] failures within [`SLOW_DOWN_WINDOW_MS`] every attempt on that account
//!   is delayed (2 s by default), which makes a guess from a botnet expensive without ever
//!   refusing the owner.
//!
//! Only accounts that exist are tracked, so a stranger typing random names cannot grow either
//! table. The state lives in memory: a restart forgets it, which costs an attacker nothing they
//! did not already have (the per-address login rate limit starts over too) and never locks
//! anybody out.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use super::{LOCKOUT_MS, MAX_FAILURES};

/// Failures from all addresses together, within [`SLOW_DOWN_WINDOW_MS`], after which every
/// attempt on the account is delayed.
pub const SLOW_DOWN_AFTER: usize = 50;
pub const SLOW_DOWN_WINDOW_MS: i64 = 15 * 60_000;
/// The delay added to each attempt on an account that is being hammered.
pub const SLOW_DOWN_DELAY_MS: u64 = 2_000;
/// Most (account, address) pairs remembered.
const MAX_TRACKED_CLIENTS: usize = 10_000;

#[derive(Debug, Clone, Copy)]
struct ClientFailures {
    count: u32,
    /// Unix ms until which this address is refused; 0 when it is not locked.
    locked_until: i64,
    last_failure: i64,
}

#[derive(Debug, Default)]
pub struct Lockouts {
    per_client: Mutex<HashMap<(String, String), ClientFailures>>,
    per_account: Mutex<HashMap<String, VecDeque<i64>>>,
}

impl Lockouts {
    pub fn new() -> Self {
        Self::default()
    }

    fn clients(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), ClientFailures>> {
        self.per_client.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn accounts(&self) -> std::sync::MutexGuard<'_, HashMap<String, VecDeque<i64>>> {
        self.per_account.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Is `ip` locked out of `account` right now?
    pub fn is_locked(&self, account: &str, ip: &str, now_ms: i64) -> bool {
        self.clients()
            .get(&(account.to_owned(), ip.to_owned()))
            .is_some_and(|f| f.locked_until > now_ms)
    }

    /// Count a failed attempt. Returns true when this one locked `ip` out of `account`.
    pub fn record_failure(&self, account: &str, ip: &str, now_ms: i64) -> bool {
        {
            let mut accounts = self.accounts();
            let window = accounts.entry(account.to_owned()).or_default();
            window.push_back(now_ms);
            while window.len() > SLOW_DOWN_AFTER {
                window.pop_front();
            }
        }
        let mut clients = self.clients();
        let key = (account.to_owned(), ip.to_owned());
        if !clients.contains_key(&key) && clients.len() >= MAX_TRACKED_CLIENTS {
            // Make room by forgetting everything that is neither locked nor recent; if that is
            // not enough this address goes untracked (the account slow-down still counts it).
            clients.retain(|_, f| f.locked_until > now_ms || now_ms - f.last_failure < LOCKOUT_MS);
            if clients.len() >= MAX_TRACKED_CLIENTS {
                return false;
            }
        }
        let entry = clients.entry(key).or_insert(ClientFailures {
            count: 0,
            locked_until: 0,
            last_failure: now_ms,
        });
        if entry.locked_until <= now_ms && now_ms - entry.last_failure >= LOCKOUT_MS {
            // An old, unrelated run of failures: start counting again.
            entry.count = 0;
        }
        entry.last_failure = now_ms;
        entry.count += 1;
        if i64::from(entry.count) >= MAX_FAILURES {
            entry.count = 0;
            entry.locked_until = now_ms.saturating_add(LOCKOUT_MS);
            return true;
        }
        false
    }

    /// A good login from `ip`: forget its failures on `account`.
    pub fn record_success(&self, account: &str, ip: &str) {
        self.clients().remove(&(account.to_owned(), ip.to_owned()));
    }

    /// Have failures from all addresses together passed the slow-down mark for `account`?
    pub fn is_under_attack(&self, account: &str, now_ms: i64) -> bool {
        self.accounts().get(account).is_some_and(|w| {
            w.iter()
                .filter(|t| now_ms - **t < SLOW_DOWN_WINDOW_MS)
                .count()
                >= SLOW_DOWN_AFTER
        })
    }

    /// Forget everything that no longer matters.
    pub fn purge(&self, now_ms: i64) {
        self.clients()
            .retain(|_, f| f.locked_until > now_ms || now_ms - f.last_failure < LOCKOUT_MS);
        self.accounts().retain(|_, w| {
            while w
                .front()
                .is_some_and(|t| now_ms - *t >= SLOW_DOWN_WINDOW_MS)
            {
                w.pop_front();
            }
            !w.is_empty()
        });
    }

    /// Entries held, for tests.
    pub fn tracked(&self) -> (usize, usize) {
        (self.clients().len(), self.accounts().len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_failures_from_one_address_lock_that_address_only() {
        let l = Lockouts::new();
        for i in 0..9 {
            assert!(!l.record_failure("ninad", "203.0.113.9", i));
        }
        assert!(!l.is_locked("ninad", "203.0.113.9", 10));
        assert!(
            l.record_failure("ninad", "203.0.113.9", 10),
            "the tenth locks"
        );
        assert!(l.is_locked("ninad", "203.0.113.9", 11));
        // Nobody else is touched: not another address, not another account.
        assert!(!l.is_locked("ninad", "198.51.100.7", 11));
        assert!(!l.is_locked("someone", "203.0.113.9", 11));
        // The lock runs out.
        assert!(l.is_locked("ninad", "203.0.113.9", 10 + LOCKOUT_MS - 1));
        assert!(!l.is_locked("ninad", "203.0.113.9", 10 + LOCKOUT_MS));
    }

    #[test]
    fn failures_from_different_addresses_never_add_up_to_a_lock() {
        let l = Lockouts::new();
        for i in 0..200 {
            assert!(!l.record_failure("ninad", &format!("10.0.{}.{}", i / 250, i % 250), i));
        }
        for i in 0..200 {
            assert!(!l.is_locked("ninad", &format!("10.0.{}.{}", i / 250, i % 250), 300));
        }
    }

    #[test]
    fn old_failures_are_forgotten_and_success_clears_the_count() {
        let l = Lockouts::new();
        for i in 0..9 {
            l.record_failure("ninad", "a", i);
        }
        // A tenth failure long after the first nine is the first of a new run.
        assert!(!l.record_failure("ninad", "a", 9 + LOCKOUT_MS));
        for i in 0..8 {
            assert!(!l.record_failure("ninad", "a", 10 + LOCKOUT_MS + i));
        }
        assert!(
            l.record_failure("ninad", "a", 20 + LOCKOUT_MS),
            "ten in a run"
        );

        let l = Lockouts::new();
        for i in 0..9 {
            l.record_failure("ninad", "b", i);
        }
        l.record_success("ninad", "b");
        assert!(
            !l.record_failure("ninad", "b", 10),
            "the count started over"
        );
    }

    #[test]
    fn many_failures_across_addresses_slow_the_account_down_without_locking_it() {
        let l = Lockouts::new();
        for i in 0..SLOW_DOWN_AFTER - 1 {
            l.record_failure("ninad", &format!("ip{i}"), i as i64);
        }
        assert!(!l.is_under_attack("ninad", 100));
        l.record_failure("ninad", "last", 100);
        assert!(l.is_under_attack("ninad", 101));
        assert!(!l.is_under_attack("someone", 101));
        assert!(!l.is_locked("ninad", "a-new-address", 101));
        // It lets go once the failures are old.
        assert!(!l.is_under_attack("ninad", SLOW_DOWN_WINDOW_MS + 200));
    }

    #[test]
    fn purge_forgets_what_no_longer_matters_and_memory_stays_bounded() {
        let l = Lockouts::new();
        for i in 0..SLOW_DOWN_AFTER * 4 {
            l.record_failure("ninad", &format!("ip{i}"), i as i64);
        }
        let (clients, accounts) = l.tracked();
        assert_eq!((clients, accounts), (SLOW_DOWN_AFTER * 4, 1));
        // The per-account window never holds more than the mark.
        assert_eq!(
            l.accounts().get("ninad").map(VecDeque::len),
            Some(SLOW_DOWN_AFTER)
        );
        l.purge(10 * LOCKOUT_MS);
        assert_eq!(l.tracked(), (0, 0));
    }
}
