//! The real server over HTTPS/WSS with a throw-away self-signed certificate.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use futures_util::StreamExt;
use jarvis_api::serve;
use jarvis_bridge_client::mock::MockBridge;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

struct Pki {
    dir: std::path::PathBuf,
    cert_pem: String,
    cert_der: Vec<u8>,
}

fn make_pki() -> Pki {
    let dir = std::env::temp_dir().join(format!("jarvis-tls-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let key =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned(), "localhost".to_owned()])
            .expect("self-signed certificate");
    let cert_pem = key.cert.pem();
    std::fs::write(dir.join("cert.pem"), &cert_pem).unwrap();
    std::fs::write(dir.join("key.pem"), key.signing_key.serialize_pem()).unwrap();
    Pki {
        dir,
        cert_pem,
        cert_der: key.cert.der().to_vec(),
    }
}

fn tls_lines(pki: &Pki) -> String {
    let p = |f: &str| pki.dir.join(f).to_string_lossy().replace('\\', "/");
    format!(
        "tls_cert = '{}'\ntls_key = '{}'",
        p("cert.pem"),
        p("key.pem")
    )
}

#[tokio::test]
async fn https_login_status_and_wss_work_end_to_end() {
    let pki = make_pki();
    let mock = MockBridge::start().await;
    let server_lines = tls_lines(&pki);
    let app = build_app(
        &mock,
        ScriptedLlm::new("fake", vec![]),
        Options {
            auth: true,
            server: &server_lines,
            ..Options::default()
        },
    )
    .await;
    assert!(app.state.config().cookie_secure());
    let user = create_user(&app, "ninad", PASSWORD).await;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(serve::serve(app.state.clone(), listener, async move {
        let _ = stop_rx.await;
    }));

    let client = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(pki.cert_pem.as_bytes()).unwrap())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let base = format!("https://127.0.0.1:{}", addr.port());

    // Health over TLS: exactly {"ok":true}, with HSTS and the rest of the headers.
    let health = client
        .get(format!("{base}/api/health"))
        .send()
        .await
        .expect("https works");
    assert_eq!(health.status(), 200);
    let headers = health.headers().clone();
    assert_eq!(
        headers.get("strict-transport-security").unwrap(),
        "max-age=31536000; includeSubDomains"
    );
    let csp = headers
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        csp.contains("connect-src 'self';")
            && !csp.contains("wss:")
            && csp.contains("upgrade-insecure-requests"),
        "{csp}"
    );
    assert_eq!(health.text().await.unwrap(), r#"{"ok":true}"#);

    // Not logged in.
    let denied = client
        .get(format!("{base}/api/status"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    // Log in: the cookie is Secure now.
    let code = app.fresh_code(&user);
    let login = client
        .post(format!("{base}/api/auth/login"))
        .json(&json!({ "username": "ninad", "password": PASSWORD, "totp": code }))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let set_cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    for part in ["HttpOnly", "Secure", "SameSite=Strict", "Path=/"] {
        assert!(set_cookie.contains(part), "{set_cookie}");
    }
    let cookie = set_cookie.split(';').next().unwrap().to_owned();

    let status: Value = client
        .get(format!("{base}/api/status"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        status["auth"],
        json!({ "enabled": true, "username": "ninad" })
    );

    // WSS with the same cookie; a kill-switch event comes through.
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(
            pki.cert_der.clone(),
        ))
        .unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut request = format!("wss://127.0.0.1:{}/api/ws", addr.port())
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    request.headers_mut().insert(
        "origin",
        format!("https://127.0.0.1:{}", addr.port())
            .parse()
            .unwrap(),
    );
    let (mut ws, _) = tokio_tungstenite::connect_async_tls_with_config(
        request,
        None,
        false,
        Some(tokio_tungstenite::Connector::Rustls(Arc::new(tls))),
    )
    .await
    .expect("wss works");
    let engaged = client
        .post(format!("{base}/api/killswitch"))
        .header("cookie", &cookie)
        .json(&json!({ "engaged": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(engaged.status(), 200);
    let frame = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(message)) = ws.next().await {
            if let Message::Text(t) = message {
                return Some(t.to_string());
            }
        }
        None
    })
    .await
    .expect("event in time")
    .expect("text frame");
    let event: Value = serde_json::from_str(&frame).unwrap();
    assert_eq!(event["type"], "kill_switch");
    assert_eq!(event["engaged"], true);

    // Plain HTTP to the TLS port gets nothing useful.
    let plain = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/api/health", addr.port()))
        .timeout(Duration::from_secs(3))
        .send()
        .await;
    assert!(plain.is_err() || !plain.unwrap().status().is_success());

    let _ = stop_tx.send(());
    tokio::time::timeout(Duration::from_secs(15), task)
        .await
        .expect("stops")
        .unwrap()
        .unwrap();
    std::fs::remove_dir_all(&pki.dir).ok();
}

#[tokio::test]
async fn a_missing_or_broken_certificate_stops_the_server_with_a_clear_error() {
    let mock = MockBridge::start().await;
    let app = build_app(
        &mock,
        ScriptedLlm::new("fake", vec![]),
        Options {
            server: "tls_cert = 'does/not/exist.pem'\ntls_key = 'nor/this.pem'",
            ..Options::default()
        },
    )
    .await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let err = serve::serve(app.state.clone(), listener, std::future::pending())
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("TLS certificate"), "{err:#}");
}
