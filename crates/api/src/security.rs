//! Response hardening: security headers on every response, the Content-Security-Policy for
//! the SPA, and JSON bodies for errors raised by the framework (413, 405).

use std::path::Path;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use sha2::{Digest, Sha256};

use crate::routes::ApiError;

/// Most HTML files scanned for inline scripts at startup, and the largest one read.
const MAX_HTML_FILES: usize = 64;
const MAX_HTML_BYTES: u64 = 2 * 1024 * 1024;

/// Header values computed once at startup.
pub struct Security {
    csp: HeaderValue,
    hsts: Option<HeaderValue>,
}

impl Security {
    /// `script_hashes`: base64 SHA-256 of the SPA's inline scripts (see
    /// [`collect_inline_script_hashes`]).
    pub fn new(tls: bool, script_hashes: &[String]) -> Self {
        let csp = HeaderValue::from_str(&content_security_policy(tls, script_hashes))
            .unwrap_or_else(|_| HeaderValue::from_static("default-src 'self'"));
        Self {
            csp,
            hsts: tls.then(|| HeaderValue::from_static("max-age=31536000; includeSubDomains")),
        }
    }

    pub fn csp(&self) -> &HeaderValue {
        &self.csp
    }
}

/// The policy for the SPA and the API.
///
/// - Scripts: same origin, plus the hashes of the inline bootstrap script SvelteKit writes
///   into `index.html`. No `unsafe-inline`, no `unsafe-eval`.
/// - Styles: same origin; `style-src-attr 'unsafe-inline'` only for `style="..."`
///   attributes (the SvelteKit root wrapper has one), never for `<style>` blocks.
/// - Connections: this server's own origin and nothing else (CSP-1). The live feed is a
///   WebSocket to the same host and port, and CSP level 3 defines `'self'` to cover it: `ws:`
///   matches a page served over `http:` and `wss:` one served over `https:`. Chrome, Edge
///   (and so the WebView2 of the desktop shell) and Firefox implement that. The bare `ws:` and
///   `wss:` schemes that this policy used to add let a script reach a WebSocket on *any* host;
///   they were there for older Safari, which is not a supported browser. The host of the page
///   varies (`127.0.0.1`, `localhost`, a domain), so naming it in the header would only repeat
///   what `'self'` already means, with the extra risk of building a header from the `Host` of
///   the request.
pub fn content_security_policy(tls: bool, script_hashes: &[String]) -> String {
    let hashes: String = script_hashes
        .iter()
        .map(|h| format!(" 'sha256-{h}'"))
        .collect();
    let mut policy = format!(
        "default-src 'self'; script-src 'self'{hashes}; style-src 'self'; \
         style-src-attr 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob:; \
         font-src 'self'; connect-src 'self'; worker-src 'self'; manifest-src 'self'; \
         object-src 'none'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'"
    );
    if tls {
        policy.push_str("; upgrade-insecure-requests");
    }
    policy
}

/// Base64 SHA-256 of every inline `<script>` body (no `src`) in `html`.
pub fn inline_script_hashes(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut hashes = Vec::new();
    let mut from = 0;
    while let Some(open) = lower[from..].find("<script") {
        let tag_start = from + open;
        let Some(tag_end_rel) = lower[tag_start..].find('>') else {
            break;
        };
        let body_start = tag_start + tag_end_rel + 1;
        let tag = &lower[tag_start..body_start];
        let Some(close_rel) = lower[body_start..].find("</script") else {
            break;
        };
        let body_end = body_start + close_rel;
        let has_src = tag.split_whitespace().any(|a| a.starts_with("src="));
        if !has_src && body_end > body_start {
            let digest = Sha256::digest(&html.as_bytes()[body_start..body_end]);
            let hash = B64.encode(digest);
            if !hashes.contains(&hash) {
                hashes.push(hash);
            }
        }
        from = body_end;
    }
    hashes
}

/// Inline-script hashes of every `*.html` under `dir` (a few levels deep, bounded).
pub fn collect_inline_script_hashes(dir: &Path) -> Vec<String> {
    let mut hashes = Vec::new();
    let mut seen = 0usize;
    let mut stack = vec![(dir.to_path_buf(), 0u8)];
    while let Some((current, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                // The hashed assets hold no HTML.
                if depth < 3 && path.file_name().is_some_and(|n| n != "_app") {
                    stack.push((path, depth + 1));
                }
            } else if meta.len() <= MAX_HTML_BYTES
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("html"))
                && seen < MAX_HTML_FILES
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                seen += 1;
                for hash in inline_script_hashes(&text) {
                    if !hashes.contains(&hash) {
                        hashes.push(hash);
                    }
                }
            }
        }
    }
    hashes.sort();
    hashes
}

const X_CONTENT_TYPE: HeaderName = HeaderName::from_static("x-content-type-options");
const X_FRAME: HeaderName = HeaderName::from_static("x-frame-options");
const REFERRER: HeaderName = HeaderName::from_static("referrer-policy");
const PERMISSIONS: HeaderName = HeaderName::from_static("permissions-policy");
const COOP: HeaderName = HeaderName::from_static("cross-origin-opener-policy");

fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"))
}

// ---------------------------------------------------------------------------
// Request guard: Host (DNS rebinding) and Origin (cross-origin writes)
// ---------------------------------------------------------------------------

/// Is this `Origin` acceptable? Same origin as the `Host` the request was sent to, or one of
/// the configured dev origins (`[server] dev_origins`, empty by default). A missing `Origin`
/// (not a browser) is accepted: the session cookie is still required, and a browser always
/// sends an `Origin` with a cross-origin request.
pub fn origin_allowed(headers: &HeaderMap, dev_origins: &[String]) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    if dev_origins.iter().any(|o| o == origin) {
        return true;
    }
    let authority = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"));
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    matches!((authority, host), (Some(a), Some(h)) if a.eq_ignore_ascii_case(h))
}

/// Is the host the request was addressed to a loopback name (`localhost`, `*.localhost`,
/// `127.x.x.x`, `[::1]`)? A request with no `Host` at all (not a browser, or an in-process
/// test) is accepted.
pub fn host_is_loopback(headers: &HeaderMap, uri: &Uri) -> bool {
    let from_header = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let authority = match (from_header, uri.authority()) {
        (Some(h), _) => h,
        (None, Some(a)) => a.as_str(),
        (None, None) => return true,
    };
    let host = match authority.strip_prefix('[') {
        // "[::1]:8787" -> "::1"
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// What [`request_guard`] enforces on `/api/*`.
pub struct RequestGuard {
    dev_origins: Vec<String>,
    loopback_hosts_only: bool,
}

impl RequestGuard {
    /// `loopback_hosts_only`: refuse any request whose `Host` is not a loopback name. Set when
    /// login is off: with no session cookie in play, a web page on another domain whose DNS
    /// answer is switched to 127.0.0.1 ("DNS rebinding") would otherwise be same-origin with
    /// the API and could drive it.
    pub fn new(dev_origins: &[String], loopback_hosts_only: bool) -> Self {
        Self {
            dev_origins: dev_origins.to_vec(),
            loopback_hosts_only,
        }
    }
}

/// Refuses, for `/api/*` only:
///
/// - any request to a non-loopback `Host` while login is off (DNS rebinding);
/// - any state-changing request (not GET/HEAD/OPTIONS) whose `Origin` is neither this server's
///   own origin nor a configured dev origin. `SameSite=Strict` already stops other *sites*; this
///   also stops other *origins of the same site* (another port on `localhost`, a sibling
///   sub-domain), which browsers treat as same-site.
pub async fn request_guard(
    State(guard): State<Arc<RequestGuard>>,
    req: Request,
    next: Next,
) -> Response {
    if is_api_path(req.uri().path()) {
        if guard.loopback_hosts_only && !host_is_loopback(req.headers(), req.uri()) {
            tracing::warn!("request refused: non-loopback Host while login is disabled");
            return ApiError::Forbidden("host not allowed".to_owned()).into_response();
        }
        let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
        if !safe && !origin_allowed(req.headers(), &guard.dev_origins) {
            tracing::warn!("state-changing request refused: foreign Origin");
            return ApiError::Forbidden("origin not allowed".to_owned()).into_response();
        }
    }
    next.run(req).await
}

/// The outermost middleware.
pub async fn apply(State(security): State<Arc<Security>>, req: Request, next: Next) -> Response {
    let api = is_api_path(req.uri().path());
    let mut response = next.run(req).await;

    // Errors raised by axum itself (body too large, wrong method) get the standard shape.
    if api
        && (response.status().is_client_error() || response.status().is_server_error())
        && !is_json(response.headers())
    {
        response = normalise_error(response);
    }

    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, security.csp.clone());
    headers.insert(X_CONTENT_TYPE, HeaderValue::from_static("nosniff"));
    headers.insert(X_FRAME, HeaderValue::from_static("DENY"));
    headers.insert(REFERRER, HeaderValue::from_static("no-referrer"));
    headers.insert(
        PERMISSIONS,
        HeaderValue::from_static("microphone=(self), camera=(), geolocation=()"),
    );
    headers.insert(COOP, HeaderValue::from_static("same-origin"));
    if let Some(hsts) = &security.hsts {
        headers.insert(header::STRICT_TRANSPORT_SECURITY, hsts.clone());
    }
    if api && !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

fn normalise_error(response: Response) -> Response {
    let error = match response.status() {
        StatusCode::PAYLOAD_TOO_LARGE => ApiError::PayloadTooLarge,
        StatusCode::REQUEST_TIMEOUT => ApiError::RequestTimeout,
        StatusCode::METHOD_NOT_ALLOWED => ApiError::MethodNotAllowed,
        StatusCode::NOT_FOUND => ApiError::NotFound("no such endpoint".to_owned()),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
        | StatusCode::BAD_REQUEST
        | StatusCode::UNPROCESSABLE_ENTITY => {
            ApiError::BadRequest("the request could not be read".to_owned())
        }
        s if s.is_server_error() => ApiError::Internal,
        _ => return response,
    };
    let (parts, _) = response.into_parts();
    let mut replacement = error.into_response();
    // Keep what the framework added (for example `Allow` on a 405).
    for (name, value) in &parts.headers {
        if name != header::CONTENT_TYPE && name != header::CONTENT_LENGTH {
            replacement
                .headers_mut()
                .append(name.clone(), value.clone());
        }
    }
    replacement
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_scripts_are_hashed_and_external_ones_are_not() {
        let html = r#"<html><head>
            <script src="/a.js"></script>
            <SCRIPT type="module" src='/b.js'></SCRIPT>
            </head><body><script>
                const x = 1;
            </script>
            <script type="module">import('/c.js')</script></body></html>"#;
        let hashes = inline_script_hashes(html);
        assert_eq!(hashes.len(), 2, "{hashes:?}");
        let expected = B64.encode(Sha256::digest(
            b"\n                const x = 1;\n            ",
        ));
        assert!(hashes.contains(&expected), "{hashes:?} vs {expected}");
        assert!(inline_script_hashes("<p>no scripts</p>").is_empty());
        assert!(inline_script_hashes("<script>unterminated").is_empty());
    }

    #[test]
    fn policy_is_strict_and_depends_on_tls() {
        let tls = content_security_policy(true, &["abc".to_owned()]);
        for part in [
            "default-src 'self'",
            "script-src 'self' 'sha256-abc'",
            "style-src 'self';",
            "style-src-attr 'unsafe-inline'",
            "img-src 'self' data: blob:",
            "media-src 'self' blob:",
            "connect-src 'self';",
            "worker-src 'self'",
            "frame-ancestors 'none'",
            "base-uri 'self'",
            "form-action 'self'",
            "upgrade-insecure-requests",
        ] {
            assert!(tls.contains(part), "{tls} lacks {part}");
        }
        assert!(!tls.contains("unsafe-eval"));
        assert!(!tls.contains("script-src 'self' 'unsafe-inline'"));
        let plain = content_security_policy(false, &[]);
        assert!(plain.contains("connect-src 'self';"));
        assert!(!plain.contains("upgrade-insecure-requests"));
        assert!(plain.contains("script-src 'self';"));
    }

    /// CSP-1: connections are limited to this server's own origin. No bare `ws:`/`wss:` (any host),
    /// no wildcard, no other host, with or without TLS.
    #[test]
    fn connect_src_is_this_origin_only() {
        for tls in [false, true] {
            let policy = content_security_policy(tls, &["abc".to_owned()]);
            let connect = policy
                .split(';')
                .map(str::trim)
                .find(|d| d.starts_with("connect-src"))
                .expect("a connect-src directive");
            assert_eq!(connect, "connect-src 'self'", "tls = {tls}: {policy}");
            for scheme in ["ws:", "wss:", "http:", "https:", "*"] {
                assert!(
                    !policy
                        .split_whitespace()
                        .any(|t| t == scheme || t == format!("{scheme};")),
                    "{scheme} must not appear as a source (tls = {tls}): {policy}"
                );
            }
        }
    }

    fn headers(origin: Option<&str>, host: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(o) = origin {
            h.insert(header::ORIGIN, HeaderValue::from_str(o).unwrap());
        }
        if let Some(v) = host {
            h.insert(header::HOST, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn origin_rules() {
        let none: &[String] = &[];
        // Same origin, either scheme, any case.
        for (origin, host) in [
            ("https://pc.local:8787", "pc.local:8787"),
            ("http://127.0.0.1:8787", "127.0.0.1:8787"),
            ("https://PC.local:8787", "pc.local:8787"),
        ] {
            assert!(origin_allowed(&headers(Some(origin), Some(host)), none));
        }
        // Foreign, different port, no host, garbage, the desktop shell's own scheme.
        for (origin, host) in [
            ("https://evil.example", Some("pc.local:8787")),
            ("https://pc.local:9999", Some("pc.local:8787")),
            ("https://pc.local:8787", None),
            ("null", Some("pc.local:8787")),
            ("tauri://localhost", Some("pc.local:8787")),
            // Another port on the same host is a different origin (but the same *site*).
            ("http://localhost:5173", Some("localhost:8787")),
        ] {
            assert!(
                !origin_allowed(&headers(Some(origin), host), none),
                "{origin} vs {host:?}"
            );
        }
        // No Origin: a non-browser client; the cookie is still required.
        assert!(origin_allowed(&headers(None, Some("pc.local:8787")), none));
    }

    #[test]
    fn dev_origins_are_only_what_the_config_lists() {
        let dev = vec!["http://localhost:5173".to_owned()];
        let h = headers(Some("http://localhost:5173"), Some("127.0.0.1:8787"));
        assert!(origin_allowed(&h, &dev));
        assert!(!origin_allowed(&h, &[]));
        let other = headers(Some("http://localhost:5174"), Some("127.0.0.1:8787"));
        assert!(!origin_allowed(&other, &dev));
    }

    #[test]
    fn loopback_host_rules() {
        let uri: Uri = "/api/status".parse().unwrap();
        for host in [
            "localhost",
            "localhost:8787",
            "LOCALHOST.:8787",
            "127.0.0.1:8787",
            "127.1.2.3",
            "[::1]:8787",
            "[::1]",
            "jarvis.localhost:8787",
        ] {
            assert!(host_is_loopback(&headers(None, Some(host)), &uri), "{host}");
        }
        for host in [
            "evil.example",
            "evil.example:8787",
            "127.0.0.1.evil.example:8787",
            "localhost.evil.example",
            "192.168.1.5:8787",
            "[2001:db8::1]:8787",
            "",
        ] {
            assert!(
                !host_is_loopback(&headers(None, Some(host)), &uri),
                "{host:?}"
            );
        }
        // No Host at all (an in-process request): accepted.
        assert!(host_is_loopback(&HeaderMap::new(), &uri));
    }

    #[test]
    fn the_built_spa_index_is_covered() {
        let index = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/build");
        if !index.join("index.html").exists() {
            return; // web/build is not committed
        }
        assert!(!collect_inline_script_hashes(&index).is_empty());
    }
}
