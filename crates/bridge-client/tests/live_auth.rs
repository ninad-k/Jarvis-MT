//! Live authentication tests against a real `JarvisBridge` EA (finding BR-1, ADR-0016).
//!
//! Run explicitly: `cargo test -p jarvis-bridge-client --test live_auth -- --ignored --test-threads=1`
//!
//! Environment (same as `live.rs`):
//! - `JARVIS_BRIDGE_ENDPOINT` (default `tcp://127.0.0.1:5555`)
//! - `JARVIS_BRIDGE_KEY` (64 hex characters) or `JARVIS_BRIDGE_KEY_FILE` (path of `bridge.key`)
//!
//! These tests need no trading account and never trade: every request that is meant to be
//! accepted is a `ping` (answered with `4001` when the terminal has no account, which still
//! proves the request got past authentication), and everything else must be refused.
use std::time::Duration;

use bytes::Bytes;
use jarvis_bridge_client::protocol::RETCODE_AUTH_FAILED;
use jarvis_bridge_client::{
    BridgeClient, BridgeConfig, BridgeError, BridgeKey, DEFAULT_ENDPOINT, auth,
};
use serde_json::{Value, json};
use zeromq::{ReqSocket, Socket, SocketRecv, SocketSend, ZmqMessage};

fn endpoint() -> String {
    std::env::var("JARVIS_BRIDGE_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned())
}

fn key() -> BridgeKey {
    if let Ok(hex) = std::env::var("JARVIS_BRIDGE_KEY") {
        return BridgeKey::from_hex(&hex).expect("JARVIS_BRIDGE_KEY is not a valid key");
    }
    let path = std::env::var("JARVIS_BRIDGE_KEY_FILE")
        .expect("set JARVIS_BRIDGE_KEY or JARVIS_BRIDGE_KEY_FILE for the live tests");
    BridgeKey::from_file(std::path::Path::new(&path)).expect("cannot use JARVIS_BRIDGE_KEY_FILE")
}

fn request(id: &str, ts: i64) -> Vec<u8> {
    serde_json::to_vec(&json!({"id": id, "ts": ts, "cmd": "ping", "args": {}})).unwrap()
}

fn fresh_id(tag: &str) -> String {
    format!("live-auth-{tag}-{}", uuid::Uuid::new_v4())
}

/// Send one message and return the reply as `(frame count, verified payload as JSON)`.
async fn exchange(message: ZmqMessage) -> (usize, Value) {
    let key = key();
    let mut socket = ReqSocket::new();
    socket
        .connect(&endpoint().replace("localhost", "127.0.0.1"))
        .await
        .expect("connect");
    socket.send(message).await.expect("send");
    let reply = tokio::time::timeout(Duration::from_secs(10), socket.recv())
        .await
        .expect("the EA must answer every request exactly once")
        .expect("receive");
    let payload = auth::open(&key, &reply).expect("the reply must be signed with the shared key");
    (reply.len(), serde_json::from_slice(payload).expect("JSON"))
}

fn assert_refused(reply: &Value, id: &str) {
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(reply["retcode"], RETCODE_AUTH_FAILED, "{reply}");
    assert_eq!(reply["error"], "authentication failed", "{reply}");
    assert_eq!(reply["id"], id, "{reply}");
    assert!(reply.get("data").is_none(), "{reply}");
    assert!(reply["ts"].is_i64() || reply["ts"].is_u64(), "{reply}");
}

#[tokio::test]
#[ignore = "needs the JarvisBridge EA running"]
async fn live_a_genuine_request_gets_past_authentication() {
    let client = BridgeClient::new(BridgeConfig::new(endpoint(), Duration::from_secs(5), key()))
        .expect("client");
    match client.ping().await {
        Ok(info) => assert_eq!(info.protocol_version, 2),
        // No trading account in the terminal: the handler ran, so authentication passed.
        Err(BridgeError::Remote { retcode: 4001, .. }) => {}
        Err(other) => panic!("a genuine request must be accepted: {other}"),
    }
}

#[tokio::test]
#[ignore = "needs the JarvisBridge EA running"]
async fn live_an_unsigned_request_is_refused() {
    let plain = request(&fresh_id("plain"), auth::now_ms());
    let (frames, reply) = exchange(ZmqMessage::from(plain)).await;
    assert_eq!(frames, 2, "even the refusal is a signed two-frame message");
    assert_refused(&reply, "");
}

#[tokio::test]
#[ignore = "needs the JarvisBridge EA running"]
async fn live_a_request_signed_with_another_key_is_refused() {
    let other = BridgeKey::generate().unwrap();
    let (_, reply) = exchange(auth::seal(
        &other,
        request(&fresh_id("other"), auth::now_ms()),
    ))
    .await;
    assert_refused(&reply, "");
}

#[tokio::test]
#[ignore = "needs the JarvisBridge EA running"]
async fn live_tampered_and_malformed_frames_are_refused() {
    let key = key();
    let id = fresh_id("tamper");
    let mut frames = auth::seal(&key, request(&id, auth::now_ms())).into_vec();
    let mut payload = frames[1].to_vec();
    payload[2] ^= 0x01;
    let tampered = ZmqMessage::try_from(vec![frames[0].clone(), Bytes::from(payload)]).unwrap();
    assert_refused(&exchange(tampered).await.1, "");

    let upper = Bytes::from(frames[0].to_ascii_uppercase());
    let uppercase = ZmqMessage::try_from(vec![upper, frames[1].clone()]).unwrap();
    assert_refused(&exchange(uppercase).await.1, "");

    frames.push(Bytes::from_static(b"extra"));
    let three = ZmqMessage::try_from(frames.clone()).unwrap();
    assert_refused(&exchange(three).await.1, "");

    // None of those used up the id: the untouched request is accepted.
    frames.truncate(2);
    let (_, reply) = exchange(ZmqMessage::try_from(frames).unwrap()).await;
    assert_ne!(reply["retcode"], RETCODE_AUTH_FAILED, "{reply}");
}

#[tokio::test]
#[ignore = "needs the JarvisBridge EA running"]
async fn live_a_replayed_request_is_refused() {
    let key = key();
    let id = fresh_id("replay");
    let signed = auth::seal(&key, request(&id, auth::now_ms()));
    let (_, first) = exchange(signed.clone()).await;
    assert_ne!(first["retcode"], RETCODE_AUTH_FAILED, "{first}");
    let (_, second) = exchange(signed).await;
    assert_refused(&second, &id);
}

#[tokio::test]
#[ignore = "needs the JarvisBridge EA running"]
async fn live_stale_and_missing_timestamps_are_refused() {
    let key = key();
    let now = auth::now_ms();
    for (tag, ts) in [("old", now - 60_000), ("future", now + 60_000)] {
        let id = fresh_id(tag);
        let (_, reply) = exchange(auth::seal(&key, request(&id, ts))).await;
        assert_refused(&reply, &id);
    }
    let id = fresh_id("nots");
    let body = serde_json::to_vec(&json!({"id": id, "cmd": "ping", "args": {}})).unwrap();
    let (_, reply) = exchange(auth::seal(&key, body)).await;
    assert_refused(&reply, &id);
}
