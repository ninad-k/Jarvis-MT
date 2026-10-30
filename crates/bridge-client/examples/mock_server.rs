//! Standalone mock EA with canned data, for offline development.
//!
//! ```text
//! cargo run -p jarvis-bridge-client --features mock --example mock_server -- tcp://127.0.0.1:5555
//! cargo run -p jarvis-bridge-client --example bridge_cli -- --key-file target/mock-bridge.key ping
//! ```
//!
//! The mock enforces the same authentication as the EA. It uses the key in `JARVIS_BRIDGE_KEY`
//! when that is set, otherwise it creates a random key, writes it to `target/mock-bridge.key`
//! (or the path in `JARVIS_MOCK_KEY_FILE`) and prints the path, never the key. Point
//! `[bridge] key_file` of the API at that file to run the whole stack without MetaTrader.
use jarvis_bridge_client::BridgeKey;
use jarvis_bridge_client::auth::KEY_ENV;
use jarvis_bridge_client::mock::{MockBridge, MockState};

fn key() -> Result<(BridgeKey, Option<std::path::PathBuf>), String> {
    if let Ok(hex) = std::env::var(KEY_ENV) {
        return BridgeKey::from_hex(&hex)
            .map(|key| (key, None))
            .map_err(|e| e.to_string());
    }
    let key = BridgeKey::generate().map_err(|e| e.to_string())?;
    let path = std::env::var_os("JARVIS_MOCK_KEY_FILE").map_or_else(
        || std::path::PathBuf::from("target/mock-bridge.key"),
        Into::into,
    );
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, format!("{}\n", key.expose_hex()))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok((key, Some(path)))
}

#[tokio::main]
async fn main() {
    let endpoint = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "tcp://127.0.0.1:5555".to_owned());
    let (key, key_file) = match key() {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    match MockBridge::start_at_with_key(&endpoint, MockState::default(), key).await {
        Ok(mock) => {
            println!(
                "mock bridge listening on {} (Ctrl+C to stop)",
                mock.endpoint
            );
            match key_file {
                Some(path) => println!("bridge key written to {}", path.display()),
                None => println!("bridge key taken from {KEY_ENV}"),
            }
            let _ = tokio::signal::ctrl_c().await;
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
