//! Live tests against a real `JarvisBridge` EA on an MT5 **demo** account.
//!
//! Run explicitly: `cargo test -p jarvis-bridge-client --test live -- --ignored --test-threads=1`
//!
//! Environment:
//! - `JARVIS_BRIDGE_ENDPOINT` (default `tcp://127.0.0.1:5555`)
//! - `JARVIS_LIVE_SYMBOL` (default `EURUSD`; use your broker's name, e.g. `XAUUSD.m`)
//!
//! These tests only read. They never send trade commands.
use std::time::Duration;

use jarvis_bridge_client::protocol::{HistoryQuery, PROTOCOL_VERSION, TradeMode};
use jarvis_bridge_client::{BridgeClient, BridgeConfig};

fn client() -> BridgeClient {
    let endpoint = std::env::var("JARVIS_BRIDGE_ENDPOINT")
        .unwrap_or_else(|_| BridgeConfig::default().endpoint);
    BridgeClient::new(BridgeConfig {
        endpoint,
        timeout: Duration::from_secs(5),
    })
    .expect("valid loopback endpoint")
}

fn symbol() -> String {
    std::env::var("JARVIS_LIVE_SYMBOL").unwrap_or_else(|_| "EURUSD".to_owned())
}

#[tokio::test]
#[ignore = "needs MT5 demo terminal with the JarvisBridge EA running"]
async fn live_ping_reports_protocol_1_and_demo_mode() {
    let info = client().ping().await.expect("ping");
    assert_eq!(info.protocol_version, PROTOCOL_VERSION);
    assert_eq!(PROTOCOL_VERSION, 1);
    assert_eq!(
        info.trade_mode,
        TradeMode::Demo,
        "refusing to treat a non-demo account as OK"
    );
    println!("{info:#?}");
}

#[tokio::test]
#[ignore = "needs MT5 demo terminal with the JarvisBridge EA running"]
async fn live_get_account() {
    let account = client().get_account().await.expect("get_account");
    assert!(!account.currency.is_empty());
    assert!(account.equity.is_finite() && account.balance.is_finite());
    println!("{account:#?}");
}

#[tokio::test]
#[ignore = "needs MT5 demo terminal with the JarvisBridge EA running"]
async fn live_get_price() {
    let symbol = symbol();
    let price = client().get_price(&symbol).await.expect("get_price");
    assert_eq!(price.symbol, symbol);
    assert!(price.bid > 0.0 && price.ask >= price.bid, "{price:?}");
    println!("{price:#?}");
}

#[tokio::test]
#[ignore = "needs MT5 demo terminal with the JarvisBridge EA running"]
async fn live_get_price_unknown_symbol_is_a_remote_error() {
    let err = client()
        .get_price("NOT_A_SYMBOL_XYZ")
        .await
        .expect_err("must fail");
    assert!(
        matches!(err, jarvis_bridge_client::BridgeError::Remote { .. }),
        "{err:?}"
    );
}

#[tokio::test]
#[ignore = "needs MT5 demo terminal with the JarvisBridge EA running"]
async fn live_positions_orders_and_history_decode() {
    let client = client();
    let positions = client.get_positions(None).await.expect("get_positions");
    let orders = client.get_orders(None).await.expect("get_orders");
    println!(
        "{} positions, {} orders",
        positions.positions.len(),
        orders.orders.len()
    );

    // MT5 timestamps are server time; anchor on the terminal's own clock.
    let now = client.ping().await.expect("ping").server_time;
    let history = client
        .get_history(HistoryQuery {
            from: now - 7 * 86_400,
            to: now,
            symbol: None,
            limit: Some(100),
        })
        .await
        .expect("get_history");
    println!(
        "{} deals (truncated: {})",
        history.deals.len(),
        history.truncated
    );
}
