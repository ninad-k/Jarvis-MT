//! Command-line client for the JarvisBridge EA (or the mock). Prints replies as pretty JSON.
//!
//! ```text
//! # the key: --key-file <path to bridge.key>, or the JARVIS_BRIDGE_KEY environment variable
//! cargo run -p jarvis-bridge-client --example bridge_cli -- --key-file <bridge.key> ping
//! cargo run -p jarvis-bridge-client --example bridge_cli -- price XAUUSD
//! cargo run -p jarvis-bridge-client --example bridge_cli -- history --days 7 --limit 50
//! cargo run -p jarvis-bridge-client --example bridge_cli -- symbol XAUUSD
//! cargo run -p jarvis-bridge-client --example bridge_cli -- rates XAUUSD M15 --count 300
//! cargo run -p jarvis-bridge-client --example bridge_cli -- calendar --hours 48 --currency USD EUR --importance high
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
    CALENDAR_RANGE_MAX_SECS, CalendarQuery, ExecType, HistoryQuery, Importance, JARVIS_MAGIC,
    OrderCancelArgs, OrderSendArgs, PositionCloseArgs, PositionClosePartialArgs,
    PositionModifyArgs, RatesQuery, Side, SymbolInfo, Timeframe, TradeMode,
};
use jarvis_bridge_client::{BridgeClient, BridgeConfig, BridgeError, BridgeKey, auth};
use serde::Serialize;

#[derive(Parser)]
#[command(name = "bridge_cli", about = "Talk to the JarvisBridge EA over ZeroMQ")]
struct Cli {
    /// EA endpoint (loopback only).
    #[arg(long, default_value = "tcp://127.0.0.1:5555")]
    endpoint: String,
    /// File that holds the shared bridge key (hex), the `bridge.key` that `scripts/install-ea.ps1`
    /// created in the terminal's `MQL5\Files\JarvisMT`. Without it the key is read from the
    /// `JARVIS_BRIDGE_KEY` environment variable.
    #[arg(long, value_name = "PATH")]
    key_file: Option<std::path::PathBuf>,
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
    /// Which broker and server the terminal is logged into.
    Broker,
    /// Price bars: prints the bar count and the first and last bar.
    Rates {
        symbol: String,
        /// M1 M5 M15 M30 H1 H4 or D1.
        timeframe: Timeframe,
        /// 1 to 1000 (default 300).
        #[arg(long)]
        count: Option<u32>,
    },
    /// Economic calendar events from now (terminal server time) to --hours ahead.
    Calendar {
        /// How far ahead to look, 1 to 336 (default 24).
        #[arg(long, default_value_t = 24)]
        hours: u32,
        /// Only these currencies, e.g. `--currency USD EUR`. Default: all.
        #[arg(long = "currency", num_args = 1..)]
        currencies: Vec<String>,
        /// Lowest importance to show: low, moderate (default) or high.
        #[arg(long)]
        importance: Option<Importance>,
    },
    /// The broker's whole symbol catalog (every page); --filter keeps names or descriptions
    /// containing the text (case-insensitive).
    Symbols {
        #[arg(long)]
        filter: Option<String>,
        /// Print only the number of symbols and how many are visible in Market Watch.
        #[arg(long)]
        count: bool,
    },
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

/// `HH:MM-HH:MM, ...` for a list of sessions in seconds since 00:00; `none` when empty.
fn format_sessions(sessions: &[[i64; 2]]) -> String {
    if sessions.is_empty() {
        return "none today".to_owned();
    }
    let clock = |s: i64| format!("{:02}:{:02}", s / 3600, s % 3600 / 60);
    sessions
        .iter()
        .map(|[from, to]| format!("{}-{}", clock(*from), clock(*to)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The fields EA 0.6.0 added to `get_symbol`, one line each. An older EA sends none of them.
fn symbol_summary(info: &SymbolInfo) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(spread) = info.spread {
        lines.push(format!("spread: {spread} points"));
    }
    if let (Some(long), Some(short)) = (info.swap_long, info.swap_short) {
        let mode = info.swap_mode.as_deref().unwrap_or("?");
        lines.push(format!("swap: long {long}, short {short} ({mode})"));
    }
    if let Some(mode) = info.trade_mode {
        // The wire name, as in the JSON above.
        let name = serde_json::to_value(mode)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        lines.push(format!("trade mode: {name}"));
    }
    if let Some(sessions) = &info.sessions {
        lines.push(format!(
            "trade sessions today (server time): {}",
            format_sessions(&sessions.trade)
        ));
        lines.push(format!(
            "quote sessions today (server time): {}",
            format_sessions(&sessions.quote)
        ));
    }
    if let Some(open) = info.trade_open_now {
        lines.push(format!(
            "trade open now: {}",
            if open { "yes" } else { "no" }
        ));
    }
    if lines.is_empty() {
        lines.push("(this EA is older than 0.6.0: no spread, swaps or sessions)".to_owned());
    }
    lines
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

/// The key from `--key-file`, else from `JARVIS_BRIDGE_KEY`. Never printed.
fn load_key(key_file: Option<&std::path::Path>) -> Result<BridgeKey, BridgeError> {
    let key = match key_file {
        Some(path) => BridgeKey::from_file(path),
        None => match std::env::var(auth::KEY_ENV) {
            Ok(hex) => BridgeKey::from_hex(&hex),
            Err(_) => {
                return Err(BridgeError::Transport(format!(
                    "no bridge key: pass --key-file <path to bridge.key> or set {}",
                    auth::KEY_ENV
                )));
            }
        },
    };
    key.map_err(|e| BridgeError::Transport(e.to_string()))
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
    let key = load_key(cli.key_file.as_deref())?;
    let client = BridgeClient::new(BridgeConfig::new(cli.endpoint, timeout, key))?;
    match cli.command {
        Cmd::Symbol { symbol } => {
            let info = client.get_symbol(&symbol).await?;
            print(&info)?;
            // The JSON on stdout stays machine-readable; the short read-out goes to stderr.
            for line in symbol_summary(&info) {
                eprintln!("{line}");
            }
            Ok(())
        }
        Cmd::Broker => print(&client.get_broker().await?),
        Cmd::Rates {
            symbol,
            timeframe,
            count,
        } => {
            let rates = client
                .get_rates(RatesQuery {
                    symbol,
                    timeframe,
                    count,
                })
                .await?;
            println!(
                "{} {}: {} bars, {} digits",
                rates.symbol,
                rates.timeframe,
                rates.bars.len(),
                rates.digits
            );
            for (label, bar) in [("first", rates.bars.first()), ("last ", rates.bars.last())] {
                match bar.map(serde_json::to_string) {
                    Some(Ok(text)) => println!("{label} {text}"),
                    Some(Err(e)) => {
                        return Err(BridgeError::Protocol(format!("could not format bar: {e}")));
                    }
                    None => println!("{label} (no bars)"),
                }
            }
            Ok(())
        }
        Cmd::Calendar {
            hours,
            currencies,
            importance,
        } => {
            let max_hours = u32::try_from(CALENDAR_RANGE_MAX_SECS / 3600).unwrap_or(336);
            if !(1..=max_hours).contains(&hours) {
                return Err(BridgeError::Transport(format!(
                    "--hours must be between 1 and {max_hours}"
                )));
            }
            // MT5 times are server time, so anchor "now" on the terminal's clock.
            let now = client.ping().await?.server_time;
            print(
                &client
                    .get_calendar(CalendarQuery {
                        from: now,
                        to: now + i64::from(hours) * 3600,
                        currencies,
                        min_importance: importance,
                    })
                    .await?,
            )
        }
        Cmd::Symbols { filter, count } => {
            let all = client.list_all_symbols().await?;
            if count {
                let visible = all.iter().filter(|s| s.visible).count();
                println!("{} symbols, {visible} visible in Market Watch", all.len());
                return Ok(());
            }
            let needle = filter.map(|f| f.to_lowercase());
            let shown: Vec<_> = all
                .iter()
                .filter(|s| {
                    needle.as_ref().is_none_or(|n| {
                        s.name.to_lowercase().contains(n)
                            || s.description.to_lowercase().contains(n)
                    })
                })
                .collect();
            print(&shown)
        }
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
