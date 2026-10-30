//! Command-line client for the JarvisBridge EA (or the mock). Prints replies as pretty JSON.
//!
//! ```text
//! cargo run -p jarvis-bridge-client --example bridge_cli -- ping
//! cargo run -p jarvis-bridge-client --example bridge_cli -- price XAUUSD
//! cargo run -p jarvis-bridge-client --example bridge_cli -- history --days 7 --limit 50
//! cargo run -p jarvis-bridge-client --example bridge_cli -- symbol XAUUSD
//! ```
//!
//! Write subcommands (`buy`, `sell`, `modify`, `close`, `cancel`) change the account. Each one
//! needs `--yes` and refuses to run unless `ping` reports a demo account. Examples:
//!
//! ```text
//! bridge_cli buy EURUSD 0.01 --sl 1.0800 --tp 1.0900 --yes
//! bridge_cli sell EURUSD 0.01 --limit 1.1000 --yes
//! bridge_cli modify 123456 --sl 0 --tp 1.0950 --yes      # 0 removes a level
//! bridge_cli close 123456 --volume 0.01 --yes            # partial close
//! bridge_cli cancel 123457 --yes
//! ```
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use jarvis_bridge_client::protocol::{
    ExecType, HistoryQuery, JARVIS_MAGIC, OrderCancelArgs, OrderSendArgs, PositionCloseArgs,
    PositionClosePartialArgs, PositionModifyArgs, Side, TradeMode,
};
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
    /// Trading specification of a symbol (lot step, tick value, stops level, ...).
    Symbol { symbol: String },
    /// Open a long position, or a buy limit/stop order. Needs --yes and a demo account.
    Buy(OrderArgs),
    /// Open a short position, or a sell limit/stop order. Needs --yes and a demo account.
    Sell(OrderArgs),
    /// Change SL/TP of an open position (0 removes a level). Needs --yes and a demo account.
    Modify {
        ticket: u64,
        #[arg(long, allow_negative_numbers = true)]
        sl: Option<f64>,
        #[arg(long, allow_negative_numbers = true)]
        tp: Option<f64>,
        #[command(flatten)]
        confirm: Confirm,
    },
    /// Close a position, or part of it with --volume. Needs --yes and a demo account.
    Close {
        ticket: u64,
        /// Volume to close; omit to close the whole position.
        #[arg(long)]
        volume: Option<f64>,
        #[command(flatten)]
        confirm: Confirm,
    },
    /// Cancel a pending order. Needs --yes and a demo account.
    Cancel {
        ticket: u64,
        #[command(flatten)]
        confirm: Confirm,
    },
}

#[derive(clap::Args)]
struct Confirm {
    /// Required for every command that changes the account.
    #[arg(long)]
    yes: bool,
}

#[derive(clap::Args)]
struct OrderArgs {
    symbol: String,
    /// Lots. Sent exactly as given; the EA rejects a volume that is not a multiple of the step.
    volume: f64,
    #[arg(long)]
    sl: Option<f64>,
    #[arg(long)]
    tp: Option<f64>,
    /// Place a pending limit order at this price.
    #[arg(long, conflicts_with = "stop")]
    limit: Option<f64>,
    /// Place a pending stop order at this price.
    #[arg(long, conflicts_with = "limit")]
    stop: Option<f64>,
    #[command(flatten)]
    confirm: Confirm,
}

fn print<T: Serialize>(value: &T) -> Result<(), BridgeError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| BridgeError::Protocol(format!("could not format output: {e}")))?;
    println!("{text}");
    Ok(())
}

/// A trade server round trip can take seconds, so writes get at least this long.
const WRITE_TIMEOUT: Duration = Duration::from_secs(15);

fn refuse(message: &str) -> BridgeError {
    BridgeError::Transport(format!("refusing to send a write command: {message}"))
}

/// Writes need `--yes` and a demo account (checked with `ping` right before sending).
async fn require_demo(client: &BridgeClient, confirm: &Confirm) -> Result<(), BridgeError> {
    if !confirm.yes {
        return Err(refuse("add --yes to confirm"));
    }
    let info = client.ping().await?;
    if info.trade_mode != TradeMode::Demo {
        return Err(refuse(&format!(
            "the account trade mode is {:?}, only demo is allowed from this tool",
            info.trade_mode
        )));
    }
    Ok(())
}

async fn order(client: &BridgeClient, side: Side, a: OrderArgs) -> Result<(), BridgeError> {
    require_demo(client, &a.confirm).await?;
    let (exec, price) = match (a.limit, a.stop) {
        (Some(p), _) => (ExecType::Limit, Some(p)),
        (_, Some(p)) => (ExecType::Stop, Some(p)),
        _ => (ExecType::Market, None),
    };
    print(
        &client
            .order_send(OrderSendArgs {
                symbol: a.symbol,
                side,
                volume: a.volume,
                exec,
                price,
                sl: a.sl,
                tp: a.tp,
                deviation_points: None,
                magic: JARVIS_MAGIC,
                comment: "jarvis:cli".to_owned(),
            })
            .await?,
    )
}

async fn run(cli: Cli) -> Result<(), BridgeError> {
    let is_write = matches!(
        cli.command,
        Cmd::Buy(_) | Cmd::Sell(_) | Cmd::Modify { .. } | Cmd::Close { .. } | Cmd::Cancel { .. }
    );
    let mut timeout = Duration::from_millis(cli.timeout_ms);
    if is_write {
        timeout = timeout.max(WRITE_TIMEOUT);
    }
    let client = BridgeClient::new(BridgeConfig {
        endpoint: cli.endpoint,
        timeout,
    })?;
    match cli.command {
        Cmd::Symbol { symbol } => print(&client.get_symbol(&symbol).await?),
        Cmd::Buy(a) => order(&client, Side::Buy, a).await,
        Cmd::Sell(a) => order(&client, Side::Sell, a).await,
        Cmd::Modify {
            ticket,
            sl,
            tp,
            confirm,
        } => {
            if sl.is_none() && tp.is_none() {
                return Err(refuse("give --sl and/or --tp"));
            }
            require_demo(&client, &confirm).await?;
            print(
                &client
                    .position_modify(PositionModifyArgs { ticket, sl, tp })
                    .await?,
            )
        }
        Cmd::Close {
            ticket,
            volume,
            confirm,
        } => {
            require_demo(&client, &confirm).await?;
            let result = match volume {
                Some(volume) => {
                    client
                        .position_close_partial(PositionClosePartialArgs {
                            ticket,
                            volume,
                            deviation_points: None,
                        })
                        .await?
                }
                None => {
                    client
                        .position_close(PositionCloseArgs {
                            ticket,
                            deviation_points: None,
                        })
                        .await?
                }
            };
            print(&result)
        }
        Cmd::Cancel { ticket, confirm } => {
            require_demo(&client, &confirm).await?;
            print(&client.order_cancel(OrderCancelArgs { ticket }).await?)
        }
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
