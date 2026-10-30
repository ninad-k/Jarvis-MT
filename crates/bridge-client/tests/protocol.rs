//! Golden-file tests: every fixture in `tests/fixtures/` must round-trip through the Rust
//! protocol types unchanged, and malformed input must be rejected.
//! `docs/protocol.md` shows the same JSON; keep them in sync.
use std::path::PathBuf;

use jarvis_bridge_client::protocol::{
    Account, Command, HISTORY_LIMIT_MAX, History, HistoryQuery, Orders, PingInfo, Positions, Price,
    Reply, ReplyError, Request,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

const COMMANDS: [&str; 6] = [
    "ping",
    "get_price",
    "get_account",
    "get_positions",
    "get_orders",
    "get_history",
];

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read fixture {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("fixture {name} is not JSON: {e}"))
}

/// Deserialize `value` into `T`, serialize it back and require identical JSON.
fn round_trip<T: Serialize + DeserializeOwned>(what: &str, value: &Value) -> T {
    let typed: T = serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("{what}: cannot deserialize: {e}"));
    let back = serde_json::to_value(&typed).expect("serialize");
    assert_eq!(&back, value, "{what}: round trip changed the JSON");
    typed
}

#[test]
fn every_request_fixture_round_trips() {
    for cmd in COMMANDS {
        let name = format!("{cmd}.request.json");
        let value = fixture(&name);
        let request: Request = round_trip(&name, &value);
        assert_eq!(request.command.name(), cmd, "{name}: command name");
    }
}

#[test]
fn every_reply_fixture_round_trips_and_decodes_to_typed_data() {
    for cmd in COMMANDS {
        let name = format!("{cmd}.reply.json");
        let value = fixture(&name);
        let reply: Reply = round_trip(&name, &value);
        assert!(reply.ok, "{name}: should be a success reply");
        assert_eq!(reply.retcode, 0, "{name}: retcode");
        let data = value["data"].clone();
        match cmd {
            "ping" => {
                let info: PingInfo = round_trip(&name, &data);
                assert_eq!(info.protocol_version, 1);
            }
            "get_price" => {
                let _: Price = round_trip(&name, &data);
            }
            "get_account" => {
                let _: Account = round_trip(&name, &data);
            }
            "get_positions" => {
                let p: Positions = round_trip(&name, &data);
                assert_eq!(p.positions.len(), 2);
                assert!(p.positions[0].sl.is_none() && p.positions[0].tp.is_some());
            }
            "get_orders" => {
                let _: Orders = round_trip(&name, &data);
            }
            "get_history" => {
                let h: History = round_trip(&name, &data);
                assert_eq!(h.deals.len(), 3);
                assert!(!h.truncated);
            }
            other => panic!("no typed check for {other}"),
        }
        // And through the same path the client uses.
        assert!(reply.decode::<Value>().is_ok());
    }
}

#[test]
fn error_reply_fixture_maps_to_remote_error() {
    let value = fixture("error.reply.json");
    let reply: Reply = round_trip("error.reply.json", &value);
    assert!(!reply.ok);
    match reply.decode::<Price>() {
        Err(ReplyError::Remote { message, retcode }) => {
            assert_eq!(retcode, 4301);
            assert_eq!(message, "unknown symbol: XAUUS");
        }
        other => panic!("expected Remote error, got {other:?}"),
    }
}

#[test]
fn request_wire_format_matches_the_contract() {
    let request = Request {
        id: "abc".into(),
        command: Command::GetPrice {
            symbol: "XAUUSD".into(),
        },
    };
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({"id": "abc", "cmd": "get_price", "args": {"symbol": "XAUUSD"}})
    );
    // Commands without arguments still carry an empty `args` object.
    let request = Request {
        id: "abc".into(),
        command: Command::Ping {},
    };
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({"id": "abc", "cmd": "ping", "args": {}})
    );
    // Optional arguments are omitted, not sent as null.
    let request = Request {
        id: "abc".into(),
        command: Command::GetPositions { symbol: None },
    };
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({"id": "abc", "cmd": "get_positions", "args": {}})
    );
    let query = HistoryQuery {
        from: 1,
        to: 2,
        symbol: None,
        limit: None,
    };
    let request = Request {
        id: "abc".into(),
        command: Command::GetHistory(query),
    };
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({"id": "abc", "cmd": "get_history", "args": {"from": 1, "to": 2}})
    );
}

#[test]
fn request_without_id_is_rejected() {
    let value = json!({"cmd": "ping", "args": {}});
    assert!(serde_json::from_value::<Request>(value).is_err());
}

#[test]
fn request_with_unknown_command_is_rejected() {
    for cmd in ["order_send", "get_pricee", "PING", ""] {
        let value = json!({"id": "x", "cmd": cmd, "args": {}});
        assert!(
            serde_json::from_value::<Request>(value).is_err(),
            "cmd {cmd:?} must be rejected"
        );
    }
}

#[test]
fn request_with_wrong_argument_types_is_rejected() {
    for value in [
        json!({"id": "x", "cmd": "get_price", "args": {}}),
        json!({"id": "x", "cmd": "get_price", "args": {"symbol": 5}}),
        json!({"id": "x", "cmd": "get_history", "args": {"from": "yesterday", "to": 2}}),
        json!({"id": "x", "cmd": "get_history", "args": {"from": 1}}),
        json!({"id": 7, "cmd": "ping", "args": {}}),
    ] {
        assert!(
            serde_json::from_value::<Request>(value.clone()).is_err(),
            "{value}"
        );
    }
}

#[test]
fn reply_without_id_or_retcode_is_rejected() {
    assert!(serde_json::from_value::<Reply>(json!({"ok": true, "retcode": 0})).is_err());
    assert!(serde_json::from_value::<Reply>(json!({"id": "x", "ok": true})).is_err());
    assert!(serde_json::from_value::<Reply>(json!({"id": "x", "retcode": 0})).is_err());
}

#[test]
fn wrong_enum_strings_are_rejected() {
    let mut account = fixture("get_account.reply.json")["data"].clone();
    account["trade_mode"] = json!("paper");
    assert!(serde_json::from_value::<Account>(account).is_err());

    let mut ping = fixture("ping.reply.json")["data"].clone();
    ping["trade_mode"] = json!("DEMO");
    assert!(serde_json::from_value::<PingInfo>(ping).is_err());

    let mut positions = fixture("get_positions.reply.json")["data"].clone();
    positions["positions"][0]["side"] = json!("long");
    assert!(serde_json::from_value::<Positions>(positions).is_err());

    let mut orders = fixture("get_orders.reply.json")["data"].clone();
    orders["orders"][0]["type"] = json!("buy_market");
    assert!(serde_json::from_value::<Orders>(orders).is_err());

    let mut history = fixture("get_history.reply.json")["data"].clone();
    history["deals"][1]["type"] = json!("purchase");
    assert!(serde_json::from_value::<History>(history.clone()).is_err());
    history["deals"][1]["type"] = json!("buy");
    history["deals"][1]["entry"] = json!("open");
    assert!(serde_json::from_value::<History>(history).is_err());
}

#[test]
fn all_enum_values_round_trip() {
    use jarvis_bridge_client::protocol::{DealEntry, DealType, OrderType, Side, TradeMode};
    macro_rules! check {
        ($ty:ty, $($variant:expr => $text:literal),+ $(,)?) => {$(
            assert_eq!(serde_json::to_value($variant).unwrap(), json!($text));
            assert_eq!(serde_json::from_value::<$ty>(json!($text)).unwrap(), $variant);
        )+};
    }
    check!(TradeMode, TradeMode::Demo => "demo", TradeMode::Contest => "contest", TradeMode::Real => "real");
    check!(Side, Side::Buy => "buy", Side::Sell => "sell");
    check!(
        OrderType,
        OrderType::BuyLimit => "buy_limit",
        OrderType::SellLimit => "sell_limit",
        OrderType::BuyStop => "buy_stop",
        OrderType::SellStop => "sell_stop",
        OrderType::BuyStopLimit => "buy_stop_limit",
        OrderType::SellStopLimit => "sell_stop_limit",
    );
    check!(
        DealType,
        DealType::Buy => "buy",
        DealType::Sell => "sell",
        DealType::Balance => "balance",
        DealType::Credit => "credit",
        DealType::Other => "other",
    );
    check!(
        DealEntry,
        DealEntry::In => "in",
        DealEntry::Out => "out",
        DealEntry::Inout => "inout",
        DealEntry::OutBy => "out_by",
    );
}

#[test]
fn integers_are_accepted_where_floats_are_expected() {
    // MQL5 may print 10000 rather than 10000.0; serde_json widens integers to f64.
    let mut account = fixture("get_account.reply.json")["data"].clone();
    account["balance"] = json!(10000);
    account["margin"] = json!(0);
    let parsed: Account = serde_json::from_value(account).unwrap();
    assert!((parsed.balance - 10000.0).abs() < f64::EPSILON);
}

#[test]
fn limits_match_the_documented_contract() {
    assert_eq!(HISTORY_LIMIT_MAX, 500);
}
