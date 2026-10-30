//! Time limits, connection limits and request budgets of the web edge (NET-1, NET-2), and the
//! shape of the Content-Security-Policy (CSP-1).

mod common;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::*;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use jarvis_api::serve;
use jarvis_bridge_client::mock::MockBridge;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tower::ServiceExt;

// ---------------------------------------------------------------- helpers

struct Running {
    addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<anyhow::Result<()>>,
}

impl Running {
    async fn stop(mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        let _ = tokio::time::timeout(Duration::from_secs(15), self.task).await;
    }
}

fn start(app: &TestApp) -> Running {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel::<()>();
    let task = tokio::spawn(serve::serve(app.state.clone(), listener, async move {
        let _ = rx.await;
    }));
    Running {
        addr,
        stop: Some(tx),
        task,
    }
}

async fn app_with(
    server: &str,
    llm: std::sync::Arc<ScriptedLlm>,
    auth: bool,
) -> (TestApp, MockBridge) {
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock,
        llm,
        Options {
            server,
            auth,
            ..Options::default()
        },
    )
    .await;
    (app, mock)
}

/// Read until the peer closes the connection (or resets it). `None` if it was still open
/// after `within`.
async fn closed_within(stream: &mut TcpStream, within: Duration) -> Option<Vec<u8>> {
    let mut received = Vec::new();
    let mut buf = [0u8; 1024];
    let deadline = Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, stream.read(&mut buf)).await {
            Err(_) => return None,
            Ok(Ok(0) | Err(_)) => return Some(received),
            Ok(Ok(n)) => received.extend_from_slice(&buf[..n]),
        }
    }
}

const HEALTH: &str = "GET /api/health HTTP/1.1\r\nHost: localhost\r\n\r\n";

/// Read one response of `GET /api/health` (the body is the fixed `{"ok":true}`).
async fn read_health(stream: &mut TcpStream) -> String {
    let mut received = Vec::new();
    let mut buf = [0u8; 1024];
    while !String::from_utf8_lossy(&received).contains(r#"{"ok":true}"#) {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("a response in time")
            .expect("read");
        assert!(n > 0, "closed before the response: {received:?}");
        received.extend_from_slice(&buf[..n]);
    }
    String::from_utf8_lossy(&received).into_owned()
}

// ------------------------------------------------- request time limits (NET-1)

#[tokio::test]
async fn a_request_that_never_finishes_sending_its_body_is_stopped_with_a_408() {
    let (app, _mock) = app_with(
        "request_timeout_secs = 1\nlong_request_timeout_secs = 5",
        ScriptedLlm::new("fake", vec![]),
        false,
    )
    .await;
    // A body that starts and never ends: the handler waits for it.
    let body = Body::from_stream(futures_util::stream::pending::<
        Result<axum::body::Bytes, std::io::Error>,
    >());
    let request = Request::builder()
        .method("POST")
        .uri("/api/positions/close")
        .header("content-type", "application/json")
        .body(body)
        .unwrap();
    let started = Instant::now();
    let response = app.router.clone().oneshot(request).await.unwrap();
    let elapsed = started.elapsed();
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(4),
        "{elapsed:?}"
    );
    // The standard error shape, and the usual headers.
    assert!(
        response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    assert!(response.headers().contains_key("content-security-policy"));
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "request_timeout", "{body}");
    assert!(body["error"]["message"].is_string());
}

#[tokio::test]
async fn chat_gets_the_longer_limit_and_other_routes_keep_the_short_one() {
    // Chat may take 3 s here, everything else 1 s; the model needs 2 s.
    let (app, _mock) = app_with(
        "request_timeout_secs = 1\nlong_request_timeout_secs = 3",
        ScriptedLlm::slow("fake", Duration::from_secs(2)),
        false,
    )
    .await;
    let started = Instant::now();
    let (status, body) = chat(&app, "what is gold at").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert_eq!(body["reply"], "late");

    // A model that takes longer than the chat limit is cut off with a 408.
    let (app, _mock) = app_with(
        "request_timeout_secs = 1\nlong_request_timeout_secs = 2",
        ScriptedLlm::slow("fake", Duration::from_secs(10)),
        false,
    )
    .await;
    let started = Instant::now();
    let (status, body) = chat(&app, "what is gold at").await;
    assert_eq!(status, StatusCode::REQUEST_TIMEOUT, "{body}");
    assert_eq!(body["error"]["code"], "request_timeout");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(1800) && elapsed < Duration::from_secs(6),
        "{elapsed:?}"
    );
    // Quick routes still answer at once.
    let (status, _) = request(&app.router, "GET", "/api/health", None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn the_websocket_is_not_subject_to_the_request_limit() {
    let (app, _mock) = app_with(
        "request_timeout_secs = 1\nlong_request_timeout_secs = 1",
        ScriptedLlm::new("fake", vec![]),
        false,
    )
    .await;
    let server = start(&app);
    let request = format!("ws://{}/api/ws", server.addr)
        .into_client_request()
        .unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("the upgrade works");
    // Well past the 1 s limit the socket is still open and still delivers events.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    app.state
        .trading()
        .engage_kill_switch("test: a late event")
        .await;
    let event = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(message) = ws.next().await {
            if let Ok(Message::Text(t)) = message {
                return Some(t.to_string());
            }
        }
        None
    })
    .await
    .expect("an event in time")
    .expect("the socket is still open");
    assert!(event.contains("kill_switch"), "{event}");
    server.stop().await;
}

// ------------------------------------------------- connection limits (NET-1)

#[tokio::test]
async fn a_client_that_sends_its_headers_too_slowly_is_disconnected() {
    let (app, _mock) = app_with(
        "header_read_timeout_secs = 1",
        ScriptedLlm::new("fake", vec![]),
        false,
    )
    .await;
    let server = start(&app);
    let mut stream = TcpStream::connect(server.addr).await.unwrap();
    // Half a request: the blank line that ends the headers never comes.
    stream
        .write_all(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nX-Slow: ")
        .await
        .unwrap();
    let started = Instant::now();
    let closed = closed_within(&mut stream, Duration::from_secs(6)).await;
    assert!(closed.is_some(), "the server held the connection open");
    assert!(started.elapsed() < Duration::from_secs(5));
    // The server is fine.
    let mut fine = TcpStream::connect(server.addr).await.unwrap();
    fine.write_all(HEALTH.as_bytes()).await.unwrap();
    assert!(read_health(&mut fine).await.starts_with("HTTP/1.1 200"));
    server.stop().await;
}

#[tokio::test]
async fn an_idle_keep_alive_connection_is_closed() {
    let (app, _mock) = app_with(
        "header_read_timeout_secs = 1",
        ScriptedLlm::new("fake", vec![]),
        false,
    )
    .await;
    let server = start(&app);
    let mut stream = TcpStream::connect(server.addr).await.unwrap();
    stream.write_all(HEALTH.as_bytes()).await.unwrap();
    assert!(read_health(&mut stream).await.starts_with("HTTP/1.1 200"));
    // A second request on the same connection works while it is being used ...
    stream.write_all(HEALTH.as_bytes()).await.unwrap();
    assert!(read_health(&mut stream).await.contains("200"));
    // ... and once it sits idle, the server lets go.
    let started = Instant::now();
    let closed = closed_within(&mut stream, Duration::from_secs(6)).await;
    assert!(closed.is_some(), "an idle connection was kept open");
    assert!(started.elapsed() < Duration::from_secs(5));
    server.stop().await;
}

#[tokio::test]
async fn connections_beyond_the_limit_are_closed_and_the_slot_comes_back() {
    let (app, _mock) = app_with(
        "max_connections = 2\nheader_read_timeout_secs = 30",
        ScriptedLlm::new("fake", vec![]),
        false,
    )
    .await;
    let server = start(&app);
    let first = TcpStream::connect(server.addr).await.unwrap();
    let mut second = TcpStream::connect(server.addr).await.unwrap();
    // Make sure the server has taken both (a request on the second proves it).
    second.write_all(HEALTH.as_bytes()).await.unwrap();
    assert!(read_health(&mut second).await.starts_with("HTTP/1.1 200"));

    // The third is accepted by the operating system and closed by the server unanswered.
    let mut third = TcpStream::connect(server.addr).await.unwrap();
    let _ = third.write_all(HEALTH.as_bytes()).await;
    let answer = closed_within(&mut third, Duration::from_secs(3))
        .await
        .expect("the server closes a connection over the limit");
    assert!(answer.is_empty(), "no response for it: {answer:?}");

    // One connection goes away; its slot is free again.
    drop(first);
    let mut served = false;
    for _ in 0..40 {
        let mut next = TcpStream::connect(server.addr).await.unwrap();
        if next.write_all(HEALTH.as_bytes()).await.is_ok() {
            let mut buf = [0u8; 64];
            if let Ok(Ok(n)) =
                tokio::time::timeout(Duration::from_millis(250), next.read(&mut buf)).await
                && n > 0
            {
                served = String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200");
                if served {
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(served, "the freed slot was never reused");
    drop(second);
    server.stop().await;
}

// ------------------------------------------------- request budgets (NET-2)

async fn two_sessions(app: &TestApp) -> (String, String) {
    let user = create_user(app, "ninad", PASSWORD).await;
    let a = login(app, &user).await;
    let b = login(app, &user).await;
    assert_ne!(a, b);
    (a, b)
}

async fn chat_as(app: &TestApp, cookie: &str) -> (StatusCode, axum::http::HeaderMap, Value) {
    let (status, headers, bytes) = raw_request(
        &app.router,
        "POST",
        "/api/chat",
        &[("cookie", cookie)],
        Some(json!({ "message": "what is gold at" }).to_string()),
    )
    .await;
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn chat_is_limited_per_session_with_a_retry_after() {
    let (app, _mock) = app_with(
        "chat_rate_limit_per_minute = 3",
        ScriptedLlm::always("fake", text_response("Gold is steady.")),
        true,
    )
    .await;
    let (alice, bob) = two_sessions(&app).await;

    for _ in 0..3 {
        let (status, _, body) = chat_as(&app, &alice).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let calls_before = app.llm.request_count();
    let (status, headers, body) = chat_as(&app, &alice).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "rate_limited");
    let retry: u64 = headers
        .get("retry-after")
        .expect("a Retry-After header")
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry), "{retry}");
    assert_eq!(
        app.llm.request_count(),
        calls_before,
        "a refused request must not reach the model"
    );

    // Another session has its own budget.
    let (status, _, body) = chat_as(&app, &bob).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The window moves on.
    app.clock().advance_ms(61_000);
    let alice_again = {
        // Sessions last 12 h, so the same cookie still works.
        chat_as(&app, &alice).await
    };
    assert_eq!(alice_again.0, StatusCode::OK, "{}", alice_again.2);
}

#[tokio::test]
async fn a_chat_refused_for_its_budget_is_refused_before_the_body_is_read() {
    // Garbage that would be a 400 is a 429 once the budget is gone: the check comes first.
    let (app, _mock) = app_with(
        "chat_rate_limit_per_minute = 1",
        ScriptedLlm::always("fake", text_response("ok")),
        true,
    )
    .await;
    let (cookie, _) = two_sessions(&app).await;
    assert_eq!(chat_as(&app, &cookie).await.0, StatusCode::OK);
    let (status, _, bytes) = raw_request(
        &app.router,
        "POST",
        "/api/chat",
        &[("cookie", &cookie)],
        Some("{ not json".to_owned()),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "rate_limited");
}

#[tokio::test]
async fn transcription_is_limited_per_session() {
    let (app, _mock) = app_with(
        "transcribe_rate_limit_per_minute = 2",
        ScriptedLlm::new("fake", vec![]),
        true,
    )
    .await;
    let (alice, bob) = two_sessions(&app).await;
    // Voice is off in the test app, so the allowed requests are answered 503 -- but they
    // still spend the budget.
    for _ in 0..2 {
        let (status, body) = request_as(&app, &alice, "POST", "/api/voice/transcribe", None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    }
    let (status, headers, _) = raw_request(
        &app.router,
        "POST",
        "/api/voice/transcribe",
        &[("cookie", &alice)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(headers.contains_key("retry-after"));
    let (status, _) = request_as(&app, &bob, "POST", "/api/voice/transcribe", None).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "the other session is not affected"
    );
    // Speaking is a different route with its own rules.
    let (status, _) = request_as(
        &app,
        &alice,
        "POST",
        "/api/voice/speak",
        Some(json!({"text": "x"})),
    )
    .await;
    assert_ne!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn without_login_the_budget_is_kept_per_address() {
    let (app, _mock) = app_with(
        "chat_rate_limit_per_minute = 2",
        ScriptedLlm::always("fake", text_response("ok")),
        false,
    )
    .await;
    assert_eq!(chat(&app, "hello").await.0, StatusCode::OK);
    assert_eq!(chat(&app, "hello").await.0, StatusCode::OK);
    let (status, body) = chat(&app, "hello").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
}

// ------------------------------------------------------------- CSP (CSP-1)

#[tokio::test]
async fn the_csp_header_allows_connections_to_this_origin_only() {
    let (app, _mock) = app_with("", ScriptedLlm::new("fake", vec![]), false).await;
    for uri in ["/api/health", "/", "/api/nothing-here"] {
        let (_, headers, _) = raw_request(&app.router, "GET", uri, &[], None).await;
        let csp = headers
            .get("content-security-policy")
            .unwrap_or_else(|| panic!("{uri}: no CSP"))
            .to_str()
            .unwrap();
        let connect = csp
            .split(';')
            .map(str::trim)
            .find(|d| d.starts_with("connect-src"))
            .unwrap();
        assert_eq!(connect, "connect-src 'self'", "{uri}: {csp}");
        assert!(!csp.contains("ws:"), "{uri}: {csp}");
    }
}
