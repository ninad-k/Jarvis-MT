//! Backend URL rules: what may be configured, what the window may navigate to,
//! and how to probe a server. Pure logic is unit-tested below.

use std::error::Error as _;
use std::net::IpAddr;
use std::time::Duration;

use url::{Host, Url};

/// Where the API serves the SPA by default (`[server] static_dir`).
pub const DEFAULT_URL: &str = "https://127.0.0.1:8787";

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => IpAddr::V4(ip).is_loopback(),
        Some(Host::Ipv6(ip)) => IpAddr::V6(ip).is_loopback(),
        None => false,
    }
}

/// Validates user input and reduces it to an origin (`scheme://host[:port]/`).
///
/// `https` is required, except `http` on a loopback host (the API refuses to
/// bind elsewhere without TLS, so plain http can only ever be local).
pub fn normalize(input: &str) -> Result<Url, String> {
    let mut url = Url::parse(input.trim())
        .map_err(|_| "Enter a full URL such as https://127.0.0.1:8787".to_string())?;
    if url.host().is_none() {
        return Err("The URL needs a host name or IP address".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Do not put credentials in the URL".into());
    }
    match url.scheme() {
        "https" => {}
        "http" if is_loopback(&url) => {}
        "http" => return Err("Plain http is only allowed for localhost. Use https".into()),
        _ => return Err("Only https URLs are supported".into()),
    }
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

/// May the main window navigate to `target`?
///
/// Allowed: the bundled pages, the configured backend origin, and (dev only)
/// the Vite dev server. Everything else is refused.
pub fn navigation_allowed(target: &Url, backend: Option<&Url>, dev: Option<&Url>) -> bool {
    let bundled = match target.scheme() {
        "tauri" => target.host_str() == Some("localhost"),
        "http" | "https" => target.host_str() == Some("tauri.localhost"),
        _ => false,
    };
    bundled
        || backend.is_some_and(|b| b.origin() == target.origin())
        || dev.is_some_and(|d| d.origin() == target.origin())
}

/// Turns a transport error into text the user can act on.
fn describe(err: &reqwest::Error, url: &Url) -> String {
    let mut chain = String::new();
    let mut source = err.source();
    while let Some(s) = source {
        chain.push_str(&s.to_string().to_lowercase());
        chain.push(' ');
        source = s.source();
    }
    let host = url.host_str().unwrap_or("the server");
    if err.is_timeout() {
        format!("{host} did not answer within 5 seconds")
    } else if chain.contains("cert") || chain.contains("unknownissuer") || chain.contains("trust") {
        "The TLS certificate is not trusted by this computer. Create a trusted local \
         certificate with mkcert (see docs/desktop.md)"
            .into()
    } else if err.is_connect() {
        format!("Could not connect to {host}. Is jarvis-api running on that address?")
    } else {
        format!("Request to {host} failed: {err}")
    }
}

/// Checks that `base` answers like an Arc MT server (`GET /api/health`).
pub async fn probe(base: &Url) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let health = base.join("api/health").map_err(|e| e.to_string())?;
    let resp = client
        .get(health)
        .send()
        .await
        .map_err(|e| describe(&e, base))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let ok = status.is_success()
        && serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("ok").and_then(serde_json::Value::as_bool))
            == Some(true);
    if ok {
        Ok(())
    } else {
        Err(format!(
            "Reached {} but it did not answer like Arc MT (HTTP {})",
            base.host_str().unwrap_or("the server"),
            status.as_u16()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn normalizes_to_origin() {
        assert_eq!(
            normalize(" https://127.0.0.1:8787/some/path?x=1#f ")
                .unwrap()
                .as_str(),
            "https://127.0.0.1:8787/"
        );
    }

    #[test]
    fn accepts_http_only_on_loopback() {
        assert!(normalize("http://localhost:8787").is_ok());
        assert!(normalize("http://127.0.0.1:8787").is_ok());
        assert!(normalize("http://[::1]:8787").is_ok());
        assert!(normalize("http://192.168.1.5:8787").is_err());
        assert!(normalize("http://example.com").is_err());
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "",
            "not a url",
            "ftp://x.com",
            "file:///c:/x",
            "javascript:alert(1)",
            "https://u:p@host/",
        ] {
            assert!(normalize(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn navigation_is_restricted() {
        let backend = u("https://127.0.0.1:8787/");
        let b = Some(&backend);
        assert!(navigation_allowed(
            &u("https://127.0.0.1:8787/chat"),
            b,
            None
        ));
        assert!(navigation_allowed(
            &u("http://tauri.localhost/index.html"),
            b,
            None
        ));
        assert!(navigation_allowed(
            &u("tauri://localhost/index.html"),
            b,
            None
        ));
        assert!(!navigation_allowed(&u("https://127.0.0.1:9999/"), b, None));
        assert!(!navigation_allowed(&u("http://127.0.0.1:8787/"), b, None));
        assert!(!navigation_allowed(&u("https://evil.example/"), b, None));
        assert!(!navigation_allowed(
            &u("https://tauri.localhost.evil.example/"),
            b,
            None
        ));
        assert!(!navigation_allowed(
            &u("file:///c:/windows/win.ini"),
            b,
            None
        ));
        assert!(!navigation_allowed(
            &u("https://127.0.0.1:8787/"),
            None,
            None
        ));
    }

    #[test]
    fn dev_server_only_when_configured() {
        let dev = u("http://localhost:5173/");
        assert!(navigation_allowed(
            &u("http://localhost:5173/x"),
            None,
            Some(&dev)
        ));
        assert!(!navigation_allowed(
            &u("http://localhost:5173/x"),
            None,
            None
        ));
    }
}
