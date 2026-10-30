//! Command-line client for the JarvisBridge EA (or the mock). Prints replies as pretty JSON.
//!
//! ```text
//! cargo run -p jarvis-bridge-client --example bridge_cli -- ping
//! cargo run -p jarvis-bridge-client --example bridge_cli -- price XAUUSD
//! cargo run -p jarvis-bridge-client --example bridge_cli -- history --days 7 --limit 50
//! ```
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use jarvis_bridge_client::protocol::HistoryQuery;
use jarvis_bridge_client::{BridgeClient, BridgeConfig, BridgeError};
use serde::Serialize;

#[derive(Parser)]
#[command(name = "bridge_cli", about = "Talk to the JarvisBridge EA over ZeroMQ")]
struct Cli {
    /// EA endpoint (loopback only).
    #[arg(long, default_value = "tcp://127.0.0.1:5555")]
    endpoint: String,
    /// Per-request timeout in milliseconds.
    #[arg(long, default_value_t = 3000)]
    timeout_ms: u64,
    /// Log bridge calls (debug level) to stderr.
    #[arg(short, long)]
    verbose: bool,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Protocol version, EA version, trade mode.
    Ping,
    /// Bid/ask for one symbol.
    Price { symbol: String },
    /// Account balance, equity and margin (no login or name).
    Account,
    /// Open positions.
    Positions {
        #[arg(long)]
        symbol: Option<String>,
    },
    /// Pending orders.
    Orders {
        #[arg(long)]
        symbol: Option<String>,
    },
    /// Deals from the last N days (relative to the terminal's server time).
    History {
        #[arg(long, default_value_t = 1)]
        days: u32,
        #[arg(long)]
        symbol: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
}

fn print<T: Serialize>(value: &T) -> Result<(), BridgeError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| BridgeError::Protocol(format!("could not format output: {e}")))?;
    println!("{text}");
    Ok(())
}

async fn run(cli: Cli) -> Result<(), BridgeError> {
    let client = BridgeClient::new(BridgeConfig {
        endpoint: cli.endpoint,
        timeout: Duration::from_millis(cli.timeout_ms),
    })?;
    match cli.command {
        Cmd::Ping => print(&client.ping().await?),
        Cmd::Price { symbol } => print(&client.get_price(&symbol).await?),
        Cmd::Account => print(&client.get_account().await?),
        Cmd::Positions { symbol } => print(&client.get_positions(symbol.as_deref()).await?),
        Cmd::Orders { symbol } => print(&client.get_orders(symbol.as_deref()).await?),
        Cmd::History {
            days,
            symbol,
            limit,
        } => {
            // MT5 timestamps are server time, so anchor "now" on the terminal's clock.
            let now = client.ping().await?.server_time;
            let query = HistoryQuery {
                from: now - i64::from(days) * 86_400,
                to: now,
                symbol,
                limit,
            };
            print(&client.get_history(query).await?)
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.verbose {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(std::io::stderr)
            .init();
    }
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
