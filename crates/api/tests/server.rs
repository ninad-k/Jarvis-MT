//! Tests that run the real server on a loopback port: WebSocket, shutdown, and the static
//! site (which is plain router work but easier to read next to them).

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use axum::http::StatusCode;
use common::*;
use futures_util::StreamExt;
use jarvis_api::serve;
use jarvis_bridge_client::mock::MockBridge;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// Next text frame, `None` on close or error.
async fn next_text<S>(ws: &mut S) -> Option<String>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    while let Some(message) = ws.next().await {
        match message {
            Ok(Message::Text(t)) => return Some(t.to_string()),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
    None
}

pub struct Running {
    pub addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    pub task: JoinHandle<anyhow::Result<()>>,
}

impl Running {
    pub async fn stop(mut self) -> anyhow::Result<()> {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        tokio::time::timeout(Duration::from_secs(15), self.task)
            .await
            .expect("the server stops in time")
            .expect("task")
    }
}

pub fn start(app: &TestApp) -> Running {
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

async fn auth_app() -> (TestApp, TestUser, MockBridge) {
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock.endpoint,
        ScriptedLlm::new("fake", vec![]),
        Options {
            auth: true,
            ..Options::default()
        },
    )
    .await;
    let user = create_user(&app, "ninad", PASSWORD).await;
    (app, user, mock)
}

fn ws_request(
    addr: SocketAddr,
    cookie: Option<&str>,
    origin: Option<&str>,
) -> tokio_tungstenite::tungstenite::handshake::client::Request {
    let mut req = format!("ws://{addr}/api/ws").into_client_request().unwrap();
    if let Some(c) = cookie {
        req.headers_mut().insert("cookie", c.parse().unwrap());
    }
    if let Some(o) = origin {
        req.headers_mut().insert("origin", o.parse().unwrap());
    }
    req
}

fn http_status(err: tokio_tungstenite::tungstenite::Error) -> StatusCode {
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => resp.status(),
        other => panic!("expected an HTTP refusal, got {other:?}"),
    }
}

// ------------------------------------------------------------- WebSocket

#[tokio::test]
async fn websocket_needs_a_session_and_a_good_origin() {
    let (app, user, _mock) = auth_app().await;
    let server = start(&app);

    let err = tokio_tungstenite::connect_async(ws_request(server.addr, None, None))
        .await
        .unwrap_err();
    assert_eq!(http_status(err), StatusCode::UNAUTHORIZED);
    let err = tokio_tungstenite::connect_async(ws_request(
        server.addr,
        Some("jarvis_session=junk"),
        None,
    ))
    .await
    .unwrap_err();
    assert_eq!(http_status(err), StatusCode::UNAUTHORIZED);

    let cookie = login(&app, &user).await;
    let err = tokio_tungstenite::connect_async(ws_request(
        server.addr,
        Some(&cookie),
        Some("https://evil.example"),
    ))
    .await
    .unwrap_err();
    assert_eq!(http_status(err), StatusCode::FORBIDDEN);

    let same_origin = format!("http://{}", server.addr);
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_request(
        server.addr,
        Some(&cookie),
        Some(&same_origin),
    ))
    .await
    .expect("same-origin upgrade");
    ws.close(None).await.ok();
    server.stop().await.unwrap();
}

#[tokio::test]
async fn websocket_forwards_events_as_json_text_frames() {
    let (app, user, _mock) = auth_app().await;
    let server = start(&app);
    let cookie = login(&app, &user).await;
    let (mut ws, _) =
        tokio_tungstenite::connect_async(ws_request(server.addr, Some(&cookie), None))
            .await
            .unwrap();

    // Engage the kill switch over HTTP: the socket gets the event.
    let (status, _) = request_as(
        &app,
        &cookie,
        "POST",
        "/api/killswitch",
        Some(json!({ "engaged": true, "reason": "test" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let frame = tokio::time::timeout(Duration::from_secs(5), next_text(&mut ws))
        .await
        .expect("an event arrives")
        .expect("text frame");
    let event: Value = serde_json::from_str(&frame).unwrap();
    assert_eq!(
        event,
        json!({ "type": "kill_switch", "engaged": true, "reason": "test" })
    );
    ws.close(None).await.ok();
    server.stop().await.unwrap();
}

#[tokio::test]
async fn shutdown_closes_open_sockets_and_finishes() {
    let (app, user, _mock) = auth_app().await;
    let server = start(&app);
    let cookie = login(&app, &user).await;
    let (mut ws, _) =
        tokio_tungstenite::connect_async(ws_request(server.addr, Some(&cookie), None))
            .await
            .unwrap();
    let started = std::time::Instant::now();
    let stopper = tokio::spawn(server.stop());
    // The client sees a close frame (going away), not a hang.
    let closed = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Close(frame))) => return frame,
                Some(Ok(_)) => continue,
                _ => return None,
            }
        }
    })
    .await
    .expect("the socket is closed during shutdown");
    if let Some(frame) = closed {
        assert_eq!(u16::from(frame.code), 1001);
    }
    stopper.await.unwrap().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "no waiting for the grace period"
    );
}

#[tokio::test]
async fn open_sockets_are_capped() {
    let (app, user, _mock) = auth_app().await;
    let server = start(&app);
    let cookie = login(&app, &user).await;
    let mut open = Vec::new();
    for _ in 0..jarvis_api::ws::MAX_SOCKETS {
        let (ws, _) =
            tokio_tungstenite::connect_async(ws_request(server.addr, Some(&cookie), None))
                .await
                .expect("within the cap");
        open.push(ws);
    }
    let err = tokio_tungstenite::connect_async(ws_request(server.addr, Some(&cookie), None))
        .await
        .unwrap_err();
    assert_eq!(http_status(err), StatusCode::TOO_MANY_REQUESTS);
    drop(open);
    server.stop().await.unwrap();
}

// ----------------------------------------------------------- static site

fn write_site(dir: &std::path::Path, inline_script: &str) {
    std::fs::create_dir_all(dir.join("_app/immutable/entry")).unwrap();
    std::fs::write(
        dir.join("index.html"),
        format!(
            "<!doctype html><html><head><script src=\"/_app/immutable/entry/start.js\"></script>\
             </head><body><div style=\"display: contents\">app</div><script>{inline_script}</script></body></html>"
        ),
    )
    .unwrap();
    std::fs::write(dir.join("_app/immutable/entry/start.js"), "console.log(1)").unwrap();
    std::fs::write(dir.join("service-worker.js"), "// sw").unwrap();
    std::fs::write(dir.join("manifest.webmanifest"), "{}").unwrap();
}

async fn site_app(dir: &std::path::Path, auth: bool) -> (TestApp, MockBridge) {
    let mock = MockBridge::start().await;
    let server = format!(
        "static_dir = '{}'",
        dir.to_string_lossy().replace('\\', "/")
    );
    let app = build_app(
        &mock.endpoint,
        ScriptedLlm::new("fake", vec![]),
        Options {
            server: &server,
            auth,
            ..Options::default()
        },
    )
    .await;
    (app, mock)
}

#[tokio::test]
async fn the_spa_is_served_with_a_fallback_and_cache_rules() {
    let dir = std::env::temp_dir().join(format!("jarvis-site-{}", uuid::Uuid::new_v4()));
    write_site(&dir, "window.__boot = 1;");
    let (app, _mock) = site_app(&dir, false).await;
    let get = |path: &'static str| {
        let router = app.router.clone();
        async move { raw_request(&router, "GET", path, &[], None).await }
    };

    for path in ["/", "/some/route", "/positions"] {
        let (status, headers, body) = get(path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(
            String::from_utf8_lossy(&body).contains("<div"),
            "{path} serves index.html"
        );
        assert_eq!(headers.get("cache-control").unwrap(), "no-cache", "{path}");
        assert!(
            headers
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("text/html"),
            "{path}"
        );
        // Security headers on static responses too.
        assert!(headers.get("content-security-policy").is_some());
        assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    }

    let (status, headers, body) = get("/_app/immutable/entry/start.js").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"console.log(1)");
    assert_eq!(
        headers.get("cache-control").unwrap(),
        "public, max-age=31536000, immutable"
    );
    for path in ["/service-worker.js", "/manifest.webmanifest"] {
        let (status, headers, _) = get(path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(headers.get("cache-control").unwrap(), "no-cache", "{path}");
    }

    // A missing file is a 404 (not index.html, which would hide a broken build); an unknown
    // API path is a JSON 404 (not index.html).
    let (status, _, body) = get("/_app/immutable/entry/gone.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!String::from_utf8_lossy(&body).contains("<div"));
    let (status, headers, body) = get("/api/unknown").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "not_found");
    // Path traversal stays inside the directory.
    let (status, _, body) = get("/..%2f..%2fCargo.toml").await;
    assert!(status == StatusCode::NOT_FOUND || status == StatusCode::OK);
    assert!(!String::from_utf8_lossy(&body).contains("[package]"));
    let (_, _, body) = get("/%2e%2e/%2e%2e/Cargo.toml").await;
    assert!(!String::from_utf8_lossy(&body).contains("[package]"));

    std::fs::remove_dir_all(dir).ok();
}

/// Nothing outside `static_dir` is ever served, whatever the encoding (Windows separators,
/// drive letters, double encoding).
#[tokio::test]
async fn static_files_cannot_escape_the_directory() {
    let root = std::env::temp_dir().join(format!("jarvis-trav-{}", uuid::Uuid::new_v4()));
    let dir = root.join("site");
    write_site(&dir, "window.__boot = 1;");
    std::fs::write(root.join("secret.txt"), "TOP-SECRET-MARKER").unwrap();
    std::fs::write(root.join("secret"), "TOP-SECRET-MARKER").unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let (app, _mock) = site_app(&dir, false).await;
    let abs_secret = root
        .join("secret.txt")
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
        .replace(':', "%3A");

    let vectors = [
        "/..%2fsecret.txt",
        "/..%2Fsecret.txt",
        "/%2e%2e/secret.txt",
        "/%2e%2e%2fsecret.txt",
        "/..%5csecret.txt",
        "/..%5Csecret.txt",
        "/%2e%2e%5csecret.txt",
        "/sub/..%2f..%2fsecret.txt",
        "/sub/..%5c..%5csecret.txt",
        "/sub/%2e%2e/%2e%2e/secret.txt",
        "/%252e%252e/secret.txt",
        "/%252e%252e%252fsecret.txt",
        "/..%2f..%2f..%2f..%2fWindows/win.ini",
        "/..%5c..%5c..%5c..%5cWindows%5cwin.ini",
        "/C:/Windows/win.ini",
        "/C%3A/Windows/win.ini",
        "/C%3A%5CWindows%5Cwin.ini",
        "//secret.txt",
        "///secret.txt",
        "/%5c%5csecret.txt",
        "/%00/secret.txt",
        "/secret.txt%00.html",
        "/..%2fsecret",
        "/..%5csecret",
    ];
    for v in vectors
        .iter()
        .map(|s| (*s).to_owned())
        .chain([format!("/{abs_secret}"), format!("/..%2f{abs_secret}")])
    {
        // A malformed URI is rejected before it reaches the server; that is fine too.
        let Ok(uri) = v.parse::<axum::http::Uri>() else {
            continue;
        };
        let (status, _, body) = raw_request(&app.router, "GET", &uri.to_string(), &[], None).await;
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("TOP-SECRET-MARKER") && !text.contains("[fonts]"),
            "{v} -> {status} leaked a file outside static_dir"
        );
    }
    // The control: a file inside the directory is served.
    let (status, _, body) = raw_request(&app.router, "GET", "/service-worker.js", &[], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"// sw");
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn the_csp_allows_exactly_the_inline_script_of_the_build() {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let dir = std::env::temp_dir().join(format!("jarvis-site-{}", uuid::Uuid::new_v4()));
    write_site(&dir, "window.__boot = 1;");
    let (app, _mock) = site_app(&dir, false).await;
    let (_, headers, _) = raw_request(&app.router, "GET", "/", &[], None).await;
    let csp = headers
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let hash =
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(b"window.__boot = 1;"));
    assert!(
        csp.contains(&format!("script-src 'self' 'sha256-{hash}'")),
        "{csp}"
    );
    assert!(!csp.contains("script-src 'self' 'unsafe-inline'"), "{csp}");
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn the_spa_shell_is_public_but_the_api_behind_it_is_not() {
    let dir = std::env::temp_dir().join(format!("jarvis-site-{}", uuid::Uuid::new_v4()));
    write_site(&dir, "1");
    let (app, _mock) = site_app(&dir, true).await;
    // The login page must load without a session.
    let (status, _, _) = raw_request(&app.router, "GET", "/login", &[], None).await;
    assert_eq!(status, StatusCode::OK);
    // An unknown API path asks for a login before it says 404.
    let (status, _, _) = raw_request(&app.router, "GET", "/api/unknown", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn without_a_static_dir_non_api_paths_are_a_json_404() {
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock.endpoint,
        ScriptedLlm::new("fake", vec![]),
        Options::default(),
    )
    .await;
    let (status, body) = request(&app.router, "GET", "/", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn orders_are_listed_for_the_hud() {
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock.endpoint,
        ScriptedLlm::new("fake", vec![]),
        Options::default(),
    )
    .await;
    let (status, body) = request(&app.router, "GET", "/api/orders", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["orders"].is_array(), "{body}");
}
