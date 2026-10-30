use clap::Parser;
use jarvis_api::cli::{Cli, Command, ServiceCommand};
use jarvis_api::{serve, users};
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Service { command }) => service(command, cli.config),
        Some(Command::User { command }) => {
            let _ = dotenvy::dotenv();
            runtime()?.block_on(users::run(cli.config.as_deref(), command))
        }
        Some(Command::Serve) | None => {
            // `.env` first: it may hold RUST_LOG as well as the secrets.
            let _ = dotenvy::dotenv();
            init_tracing();
            runtime()?.block_on(serve::run(cli.config, serve::ctrl_c()))
        }
    }
}

fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?)
}

#[cfg(windows)]
fn service(command: ServiceCommand, config: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    jarvis_api::service::execute(command, config)
}

#[cfg(not(windows))]
fn service(_: ServiceCommand, _: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    anyhow::bail!(
        "the service commands exist on Windows only; use systemd or a container elsewhere"
    )
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,jarvis_api=debug,jarvis_bridge_client=debug"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
