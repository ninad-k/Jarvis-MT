//! Starting the server: bind, TLS, graceful shutdown.

use std::future::Future;
use std::io;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use anyhow::Context;
use axum_server::Handle;
use axum_server::accept::Accept;
use axum_server::tls_rustls::RustlsConfig;
use hyper_util::rt::TokioTimer;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

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

    for name in config.providers_slower_than_chat_timeout() {
        tracing::warn!(
            provider = name,
            "this provider's timeout_secs is not shorter than server.long_request_timeout_secs ({} s): \
             a chat turn that waits for it can be cut short with a 408",
            config.server.long_request_timeout_secs
        );
    }
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

    let limits = ConnectionLimits::from_config(&state.config().server);
    let make_service = crate::app(state).into_make_service_with_connect_info::<SocketAddr>();
    match tls {
        Some(tls) => {
            tracing::info!("listening on https://{addr}");
            let mut server = axum_server::from_tcp_rustls(listener, tls)?
                .handle(handle)
                .map(|acceptor| ConnectionLimit::new(acceptor, limits.max_connections));
            limits.apply(server.http_builder());
            server.serve(make_service).await
        }
        None => {
            tracing::info!("listening on http://{addr}");
            let mut server = axum_server::from_tcp(listener)?
                .handle(handle)
                .map(|acceptor| ConnectionLimit::new(acceptor, limits.max_connections));
            limits.apply(server.http_builder());
            server.serve(make_service).await
        }
    }
    .context("server error")
}

// ---------------------------------------------------------------------------
// Connection limits (NET-1)
// ---------------------------------------------------------------------------

/// How long a client may take, and how many may connect.
struct ConnectionLimits {
    header_read_timeout: Duration,
    max_connections: usize,
}

impl ConnectionLimits {
    fn from_config(server: &crate::config::ServerConfig) -> Self {
        Self {
            header_read_timeout: Duration::from_secs(server.header_read_timeout_secs),
            max_connections: server.max_connections,
        }
    }

    /// HTTP/1: the client has `header_read_timeout` to send a request's headers. hyper starts
    /// that clock whenever it waits for the next request, so it also closes keep-alive
    /// connections that sit idle for that long, and a slow-header ("slowloris") client gets
    /// nowhere. HTTP/2 (negotiated only over TLS): idle or dead peers are found with pings, and
    /// a connection may have few streams open.
    fn apply(
        &self,
        builder: &mut hyper_util::server::conn::auto::Builder<hyper_util::rt::TokioExecutor>,
    ) {
        builder
            .http1()
            .timer(TokioTimer::new())
            .header_read_timeout(self.header_read_timeout);
        builder
            .http2()
            .timer(TokioTimer::new())
            .keep_alive_interval(Some(HTTP2_PING_EVERY))
            .keep_alive_timeout(HTTP2_PING_TIMEOUT)
            .max_concurrent_streams(HTTP2_MAX_STREAMS);
    }
}

const HTTP2_PING_EVERY: Duration = Duration::from_secs(20);
const HTTP2_PING_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP2_MAX_STREAMS: u32 = 64;

/// Wraps the acceptor: a connection beyond `max_connections` is closed before anything is
/// read from it (before the TLS handshake, when there is one). A permit is held for as long as
/// the connection lives.
#[derive(Clone)]
pub struct ConnectionLimit<A> {
    inner: A,
    permits: Arc<Semaphore>,
}

impl<A> ConnectionLimit<A> {
    pub fn new(inner: A, max_connections: usize) -> Self {
        Self {
            inner,
            permits: Arc::new(Semaphore::new(max_connections)),
        }
    }
}

type AcceptFuture<S, V> = Pin<Box<dyn Future<Output = io::Result<(S, V)>> + Send>>;

impl<A, I, S> Accept<I, S> for ConnectionLimit<A>
where
    A: Accept<I, S> + Clone + Send + 'static,
    A::Future: Send,
    A::Stream: Send,
    A::Service: Send,
    I: Send + 'static,
    S: Send + 'static,
{
    type Stream = Counted<A::Stream>;
    type Service = A::Service;
    type Future = AcceptFuture<Self::Stream, Self::Service>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let Ok(permit) = Arc::clone(&self.permits).try_acquire_owned() else {
            tracing::warn!("connection refused: the connection limit is reached");
            return Box::pin(std::future::ready(Err(io::Error::other(
                "too many connections",
            ))));
        };
        let inner = self.inner.accept(stream, service);
        Box::pin(async move {
            let (stream, service) = inner.await?;
            Ok((
                Counted {
                    stream,
                    _permit: permit,
                },
                service,
            ))
        })
    }
}

/// A connection that holds one permit of the limit until it is dropped.
pub struct Counted<T> {
    stream: T,
    _permit: OwnedSemaphorePermit,
}

impl<T: AsyncRead + Unpin> AsyncRead for Counted<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Counted<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
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
