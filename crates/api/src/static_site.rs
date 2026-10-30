//! Serves the built SPA (`[server] static_dir`).
//!
//! A path with a file extension is a file or a 404. Any other path that is not a file is a
//! client-side route and gets `index.html`. `/api/*` never reaches this module.

use std::path::Path;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

/// Cache policy for hashed build output: the file name changes with the content.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// Everything else (HTML, the service worker, the manifest, icons) is revalidated.
const REVALIDATE: &str = "no-cache";

#[derive(Clone)]
pub struct StaticSite {
    files: ServeDir,
    spa: ServeDir<ServeFile>,
}

impl StaticSite {
    pub fn new(dir: &Path) -> Self {
        let files = ServeDir::new(dir).precompressed_br().precompressed_gzip();
        let spa = ServeDir::new(dir)
            .precompressed_br()
            .precompressed_gzip()
            .fallback(ServeFile::new(dir.join("index.html")));
        Self { files, spa }
    }

    pub async fn serve(&self, req: Request) -> Response {
        let path = req.uri().path().to_owned();
        let last = path.rsplit('/').next().unwrap_or("");
        let has_extension = last.contains('.');
        let req = req.map(|_| Body::empty());
        let mut response = if has_extension {
            match self.files.clone().oneshot(req).await {
                Ok(r) => r.into_response(),
                Err(never) => match never {},
            }
        } else {
            match self.spa.clone().oneshot(req).await {
                Ok(r) => r.into_response(),
                Err(never) => match never {},
            }
        };
        let cache = if path.starts_with("/_app/immutable/") && response.status().is_success() {
            IMMUTABLE
        } else {
            REVALIDATE
        };
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
        response
    }
}
