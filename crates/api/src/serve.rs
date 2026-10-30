//! Starting the server: bind, TLS, graceful shutdown.

use std::future::Future;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use axum_server::Handle;
use axum_server::tls_rustls::RustlsConfig;

use crate::config::Config;
use crate::state::AppState;

/// How long open connections get to finish after a stop request.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Load the configuration and run the server until `shutdown` completes. Logging must
/// already be set up by the caller.
pub async fn run(
    config_path: Option<PathBuf>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let config = Config::load_with(config_path.as_deref())?;
    let addr = config.bind_addr()?;
    tracing::info!(
        mode = config.mode.as_str(),
        bridge = %config.bridge.endpoint,
        providers = config.llm.providers.len(),
        auth = config.auth.enabled,
        tls = config.tls_enabled(),
        "jarvis-api {} starting",
        env!("CARGO_PKG_VERSION")
    );

    let state = AppState::build(config).await?;
    state.startup_check().await?;
    let _background = state.spawn_background_tasks();

    let listener = TcpListener::bind(addr)
        .with_context(|| format!("binding {addr} (is another jarvis-api running?)"))?;
    serve(state, listener, shutdown).await?;
    tracing::info!("shut down cleanly");
    Ok(())
}

/// Serve `state` on `listener` (HTTPS when the config has a certificate and key) until
/// `shutdown` completes. Open WebSockets are told to close; other connections get
/// [`SHUTDOWN_GRACE`] to finish.
pub async fn serve(
    state: AppState,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let addr = listener.local_addr()?;
    listener
        .set_nonblocking(true)
        .context("setting the listener non-blocking")?;
    let tls = match (
        &state.config().server.tls_cert,
        &state.config().server.tls_key,
    ) {
        (Some(cert), Some(key)) => Some(load_tls(cert, key).await?),
        _ => None,
    };

    let handle: Handle<SocketAddr> = Handle::new();
    {
        let handle = handle.clone();
        let state = state.clone();
        tokio::spawn(async move {
            shutdown.await;
            tracing::info!("stop requested, shutting down");
            state.shutdown().trigger();
            handle.graceful_shutdown(Some(SHUTDOWN_GRACE));
        });
    }

    let make_service = crate::app(state).into_make_service_with_connect_info::<SocketAddr>();
    match tls {
        Some(tls) => {
            tracing::info!("listening on https://{addr}");
            axum_server::from_tcp_rustls(listener, tls)?
                .handle(handle)
                .serve(make_service)
                .await
        }
        None => {
            tracing::info!("listening on http://{addr}");
            axum_server::from_tcp(listener)?
                .handle(handle)
                .serve(make_service)
                .await
        }
    }
    .context("server error")
}

async fn load_tls(cert: &std::path::Path, key: &std::path::Path) -> anyhow::Result<RustlsConfig> {
    // One crypto provider for the whole process; a second install is harmless.
    let _ = rustls::crypto::ring::default_provider().install_default();
    RustlsConfig::from_pem_file(cert, key).await.with_context(|| {
        format!(
            "loading the TLS certificate {} and key {} (PEM files; the key must be unencrypted)",
            cert.display(),
            key.display()
        )
    })
}

/// Resolves on Ctrl+C.
pub async fn ctrl_c() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!("could not listen for Ctrl+C: {e}");
        std::future::pending::<()>().await;
    }
    tracing::info!("Ctrl+C received");
}
