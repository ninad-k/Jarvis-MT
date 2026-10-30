//! The server needs the shared bridge key to start (BR-1, ADR-0016), and the key it loads is
//! the one its bridge client signs with.
mod common;

use std::time::Duration;

use common::{config_toml, temp_db_url};
use jarvis_api::AppState;
use jarvis_api::config::Config;
use jarvis_bridge_client::BridgeKey;
use jarvis_bridge_client::mock::{MockBridge, MockState};

/// Test config pointing at `endpoint`; `key_file` (a path) is added under `[bridge]` when given.
/// Login is off so that no session secret is needed.
fn config(endpoint: &str, key_file: Option<&std::path::Path>) -> Config {
    let mut toml = config_toml("demo", endpoint, "", &[], "", "", "", false);
    if let Some(path) = key_file {
        toml = toml.replacen(
            "[bridge]",
            &format!("[bridge]\nkey_file = '{}'", path.display()),
            1,
        );
    }
    toml = toml.replacen(
        "[symbols]",
        &format!("[database]\nurl = \"{}\"\n\n[symbols]", temp_db_url()),
        1,
    );
    Config::from_toml_str(&toml).expect("test config")
}

fn write_key_file(key: &BridgeKey) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("jarvis-bridge-key-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("bridge.key");
    std::fs::write(&path, format!("{}\n", key.expose_hex())).expect("write key");
    path
}

#[tokio::test]
async fn the_server_refuses_to_start_without_a_bridge_key() {
    if std::env::var("JARVIS_BRIDGE_KEY").is_ok_and(|v| !v.trim().is_empty()) {
        return; // the developer's own environment supplies a key; nothing to refuse
    }
    let mock = MockBridge::start().await;
    let err = AppState::build(config(&mock.endpoint, None))
        .await
        .err()
        .expect("must refuse to start");
    let text = format!("{err:#}");
    assert!(text.contains("refusing to start"), "{text}");
    assert!(text.contains("no bridge key"), "{text}");
    assert!(text.contains("install-ea.ps1"), "{text}");
    assert_eq!(mock.request_count(), 0, "nothing was sent to the bridge");
}

#[tokio::test]
async fn the_server_refuses_to_start_with_an_unusable_key_file() {
    if std::env::var("JARVIS_BRIDGE_KEY").is_ok_and(|v| !v.trim().is_empty()) {
        return;
    }
    let mock = MockBridge::start().await;
    let dir = std::env::temp_dir().join(format!("jarvis-bridge-key-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, content) in [
        ("missing.key", None),
        ("short.key", Some("abc123")),
        ("zeros.key", Some(&"00".repeat(32)[..])),
    ] {
        let path = dir.join(name);
        if let Some(content) = content {
            std::fs::write(&path, content).unwrap();
        }
        let err = AppState::build(config(&mock.endpoint, Some(&path)))
            .await
            .err()
            .unwrap_or_else(|| panic!("{name}: must refuse to start"));
        let text = format!("{err:#}");
        assert!(text.contains("refusing to start"), "{name}: {text}");
        assert!(text.contains("install-ea.ps1"), "{name}: {text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_server_signs_with_the_key_from_the_file() {
    if std::env::var("JARVIS_BRIDGE_KEY").is_ok_and(|v| !v.trim().is_empty()) {
        return; // the environment key would win over the file under test
    }
    let key = BridgeKey::generate().unwrap();
    let mock = MockBridge::start_with_key(MockState::default(), key.clone()).await;
    let path = write_key_file(&key);
    let state = AppState::build(config(&mock.endpoint, Some(&path)))
        .await
        .expect("a valid key file starts the server");
    let info = tokio::time::timeout(Duration::from_secs(5), state.bridge().ping())
        .await
        .expect("ping in time")
        .expect("the mock accepts the key from the file");
    assert_eq!(info.protocol_version, 2);
    assert_eq!(mock.auth_failures(), 0);

    // A different key in the file is accepted by the server but refused by the EA.
    let other = write_key_file(&BridgeKey::generate().unwrap());
    let state = AppState::build(config(&mock.endpoint, Some(&other)))
        .await
        .expect("any valid key starts the server");
    assert!(state.bridge().ping().await.is_err());
    assert_eq!(mock.auth_failures(), 1);
    for file in [path, other] {
        if let Some(dir) = file.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
