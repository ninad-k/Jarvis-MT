use anyhow::Context;
use jarvis_api::config::Config;
use jarvis_api::{AppState, app};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::load()?;
    init_tracing();

    let addr = config.bind_addr()?;
    tracing::info!(
        mode = config.mode.as_str(),
        bridge = %config.bridge.endpoint,
        providers = config.llm.providers.len(),
        "jarvis-api {} starting",
        env!("CARGO_PKG_VERSION")
    );

    let state = AppState::build(config).await?;
    state.startup_check().await?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr} (is another jarvis-api running?)"))?;
    tracing::info!("listening on http://{}", listener.local_addr()?);
    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;
    tracing::info!("shut down cleanly");
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,jarvis_api=debug,jarvis_bridge_client=debug"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn shutdown_signal() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!("could not listen for Ctrl+C: {e}");
        std::future::pending::<()>().await;
    }
    tracing::info!("Ctrl+C received, shutting down");
}
