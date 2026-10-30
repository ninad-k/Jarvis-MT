//! Server events for the UI (`GET /api/ws`, added by the platform task).
//!
//! Trading code publishes here; nothing in the HTTP routes subscribes yet. A WebSocket
//! handler only has to call [`EventBus::subscribe`] and forward each [`Event`] as a JSON
//! text frame.

use serde::Serialize;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::trading::TradeResultDto;

/// Frames sent to the browser, `{"type": "...", ...}` (see docs/api.md, "Events").
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    KillSwitch {
        engaged: bool,
        reason: Option<String>,
    },
    PreviewExpired {
        token: String,
    },
    TradeExecuted {
        preview_id: Uuid,
        results: Vec<TradeResultDto>,
    },
    PositionsChanged,
}

const CAPACITY: usize = 64;

/// Fan-out channel for [`Event`]s. Cheap to clone.
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CAPACITY);
        Self { tx }
    }

    /// Publish an event. Having no subscribers is normal and not an error.
    pub fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_serialise_as_documented() {
        let v = serde_json::to_value(Event::KillSwitch {
            engaged: true,
            reason: Some("manual".into()),
        })
        .unwrap();
        assert_eq!(
            v,
            json!({"type":"kill_switch","engaged":true,"reason":"manual"})
        );
        assert_eq!(
            serde_json::to_value(Event::PositionsChanged).unwrap(),
            json!({"type":"positions_changed"})
        );
        assert_eq!(
            serde_json::to_value(Event::PreviewExpired { token: "t".into() }).unwrap(),
            json!({"type":"preview_expired","token":"t"})
        );
    }

    #[tokio::test]
    async fn subscribers_receive_and_no_subscriber_is_fine() {
        let bus = EventBus::new();
        bus.emit(Event::PositionsChanged); // nobody listening
        let mut rx = bus.subscribe();
        bus.emit(Event::PositionsChanged);
        assert_eq!(rx.recv().await.unwrap(), Event::PositionsChanged);
    }
}
