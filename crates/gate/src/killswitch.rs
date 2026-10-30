use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Process-wide kill switch (SPEC rule 6). Cheap to clone; all clones share state.
/// Checked by `evaluate`, `TokenStore::confirm` and right before every bridge write.
#[derive(Debug, Clone, Default)]
pub struct KillSwitch {
    engaged: Arc<AtomicBool>,
    detail: Arc<Mutex<KillSwitchState>>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct KillSwitchState {
    pub engaged: bool,
    pub reason: Option<String>,
    /// Unix ms of the last change.
    pub changed_at_ms: Option<i64>,
}

impl KillSwitch {
    pub fn new(engaged: bool) -> Self {
        let ks = Self::default();
        ks.engaged.store(engaged, Ordering::SeqCst);
        if let Ok(mut d) = ks.detail.lock() {
            d.engaged = engaged;
        }
        ks
    }

    pub fn is_engaged(&self) -> bool {
        self.engaged.load(Ordering::SeqCst)
    }

    pub fn engage(&self, reason: impl Into<String>, now_ms: i64) {
        self.engaged.store(true, Ordering::SeqCst);
        if let Ok(mut d) = self.detail.lock() {
            *d = KillSwitchState { engaged: true, reason: Some(reason.into()), changed_at_ms: Some(now_ms) };
        }
    }

    pub fn release(&self, now_ms: i64) {
        self.engaged.store(false, Ordering::SeqCst);
        if let Ok(mut d) = self.detail.lock() {
            *d = KillSwitchState { engaged: false, reason: None, changed_at_ms: Some(now_ms) };
        }
    }

    pub fn state(&self) -> KillSwitchState {
        self.detail.lock().map(|d| d.clone()).unwrap_or_else(|_| KillSwitchState {
            engaged: self.is_engaged(),
            ..Default::default()
        })
    }
}
