//! `GET /api/ws`: server events as JSON text frames.
//!
//! The route sits behind the session middleware, so an upgrade without a valid cookie never
//! gets here. On top of that the `Origin` header must be this server's own origin or one of
//! the configured dev origins, and the number of open sockets is capped.

use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::http::HeaderMap;
use axum::response::Response;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

use crate::auth::Session;
use crate::auth::http::MaybeSession;
use crate::events::Event;
use crate::routes::ApiError;
use crate::security::origin_allowed;
use crate::state::AppState;

/// Open sockets allowed at once.
pub const MAX_SOCKETS: usize = 16;
/// Interval between pings (and between session re-checks).
pub const PING_EVERY: Duration = Duration::from_secs(20);
/// A socket that has sent nothing (no pong either) for this long is dropped.
pub const IDLE_LIMIT: Duration = Duration::from_secs(70);
/// Inbound frames are only pongs and the odd close; anything bigger is refused.
const MAX_INBOUND_BYTES: usize = 1024;

pub async fn handler(
    State(state): State<AppState>,
    MaybeSession(session): MaybeSession,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    if !origin_allowed(&headers, &state.config().server.dev_origins) {
        tracing::warn!("websocket refused: foreign Origin");
        return Err(ApiError::Forbidden("origin not allowed".to_owned()));
    }
    let permit = state
        .ws_slots()
        .try_acquire_owned()
        .map_err(|_| ApiError::RateLimited {
            retry_after_secs: 5,
        })?;
    Ok(upgrade
        .max_message_size(MAX_INBOUND_BYTES)
        .max_frame_size(MAX_INBOUND_BYTES)
        .on_upgrade(move |socket| async move {
            run(socket, state, session).await;
            drop(permit);
        }))
}

async fn send_event(socket: &mut WebSocket, event: &Event) -> bool {
    match serde_json::to_string(event) {
        Ok(text) => socket.send(Message::Text(text.into())).await.is_ok(),
        Err(e) => {
            tracing::error!("serialising an event failed: {e}");
            true
        }
    }
}

async fn run(mut socket: WebSocket, state: AppState, session: Option<Session>) {
    let mut events = state.events().subscribe();
    let mut shutdown = state.shutdown().subscribe();
    let mut tick = tokio::time::interval_at(Instant::now() + PING_EVERY, PING_EVERY);
    let mut last_heard = Instant::now();
    loop {
        tokio::select! {
            received = events.recv() => match received {
                Ok(event) => {
                    if !send_event(&mut socket, &event).await {
                        break;
                    }
                }
                // Events were dropped: tell the UI to re-read instead of guessing what.
                Err(RecvError::Lagged(_)) => {
                    if !send_event(&mut socket, &Event::PositionsChanged).await {
                        break;
                    }
                }
                Err(RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                None | Some(Err(_) | Ok(Message::Close(_))) => break,
                // Pongs, pings (answered by the library) and chatter count as life signs.
                Some(Ok(_)) => last_heard = Instant::now(),
            },
            _ = tick.tick() => {
                if last_heard.elapsed() > IDLE_LIMIT {
                    break;
                }
                if let (Some(auth), Some(session)) = (state.auth(), session.as_ref())
                    && !auth.is_current(session).await
                {
                    let _ = socket
                        .send(Message::Close(Some(CloseFrame {
                            code: close_code::POLICY,
                            reason: "session ended".into(),
                        })))
                        .await;
                    break;
                }
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            },
            _ = shutdown.changed() => {
                let _ = socket
                    .send(Message::Close(Some(CloseFrame {
                        code: close_code::AWAY,
                        reason: "server shutting down".into(),
                    })))
                    .await;
                break;
            }
        }
    }
}
