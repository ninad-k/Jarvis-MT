//! Standalone mock EA with canned data, for offline development.
//!
//! ```text
//! cargo run -p jarvis-bridge-client --features mock --example mock_server -- tcp://127.0.0.1:5555
//! cargo run -p jarvis-bridge-client --example bridge_cli -- ping
//! ```
use jarvis_bridge_client::mock::{MockBridge, MockState};

#[tokio::main]
async fn main() {
    let endpoint = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "tcp://127.0.0.1:5555".to_owned());
    match MockBridge::start_at(&endpoint, MockState::default()).await {
        Ok(mock) => {
            println!(
                "mock bridge listening on {} (Ctrl+C to stop)",
                mock.endpoint
            );
            let _ = tokio::signal::ctrl_c().await;
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
