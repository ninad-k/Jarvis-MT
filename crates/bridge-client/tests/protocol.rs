//! Golden-file tests: every fixture in `tests/fixtures/` must round-trip through the Rust
//! protocol types unchanged, and malformed input must be rejected.
//! `docs/protocol.md` shows the same JSON; keep them in sync.
use std::path::PathBuf;

use jarvis_bridge_client::protocol::{
    Account, Command, HISTORY_LIMIT_MAX, History, HistoryQuery, Orders, PingInfo, Positions, Price,
    Reply, ReplyError, Request, SymbolInfo, SymbolTradeMode, TradeResult,
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
                assert_eq!(info.protocol_version, 2);
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
        ts: None,
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
        ts: None,
        command: Command::Ping {},
    };
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({"id": "abc", "cmd": "ping", "args": {}})
    );
    // Optional arguments are omitted, not sent as null.
    let request = Request {
        id: "abc".into(),
        ts: None,
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
        ts: None,
        command: Command::GetHistory(query),
    };
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({"id": "abc", "cmd": "get_history", "args": {"from": 1, "to": 2}})
    );
}

#[test]
fn request_and_reply_carry_the_clock_when_set() {
    let request = Request {
        id: "abc".into(),
        ts: Some(1_793_341_200_000),
        command: Command::Ping {},
    };
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({"id": "abc", "ts": 1_793_341_200_000_i64, "cmd": "ping", "args": {}})
    );
    let back: Request = serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
    assert_eq!(back, request);
    let reply: Reply = serde_json::from_value(
        json!({"id": "abc", "ok": false, "error": "authentication failed", "retcode": -4, "ts": 5}),
    )
    .unwrap();
    assert_eq!(reply.ts, Some(5));
    assert_eq!(
        reply.retcode,
        jarvis_bridge_client::protocol::RETCODE_AUTH_FAILED
    );
    assert_eq!(jarvis_bridge_client::protocol::PROTOCOL_VERSION, 2);
}

#[test]
fn request_without_id_is_rejected() {
    let value = json!({"cmd": "ping", "args": {}});
    assert!(serde_json::from_value::<Request>(value).is_err());
}

#[test]
fn request_with_unknown_command_is_rejected() {
    for cmd in ["order_sen", "get_pricee", "PING", "", "get_symbols"] {
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

// ---- Sprint 2: get_symbol and the write commands ---------------------------------------------------

/// `(fixture base name, wire command)` for every request/reply pair added in Sprint 2.
const WRITE_FIXTURES: [(&str, &str); 7] = [
    ("get_symbol", "get_symbol"),
    ("order_send", "order_send"),
    ("order_send_limit", "order_send"),
    ("position_modify", "position_modify"),
    ("position_close", "position_close"),
    ("position_close_partial", "position_close_partial"),
    ("order_cancel", "order_cancel"),
];

#[test]
fn write_and_symbol_request_fixtures_round_trip() {
    for (base, cmd) in WRITE_FIXTURES
        .iter()
        .copied()
        .chain([("order_send_invalid_volume", "order_send")])
    {
        let name = format!("{base}.request.json");
        let request: Request = round_trip(&name, &fixture(&name));
        assert_eq!(request.command.name(), cmd, "{name}");
        assert_eq!(request.command.is_write(), cmd != "get_symbol", "{name}");
    }
}

#[test]
fn symbol_reply_of_an_older_ea_still_parses_with_the_new_fields_unset() {
    // EA 0.5.0 and older send the thirteen original fields and nothing else.
    let name = "get_symbol_legacy.reply.json";
    let value = fixture(name);
    let reply: Reply = round_trip(name, &value);
    let spec: SymbolInfo = round_trip(name, &value["data"]);
    assert_eq!(spec.symbol, "XAUUSD");
    assert_eq!(
        (spec.spread, spec.swap_long, spec.swap_short, spec.swap_mode),
        (None, None, None, None)
    );
    assert_eq!(
        (spec.trade_mode, spec.sessions, spec.trade_open_now),
        (None, None, None)
    );
    // Decoding through the reply envelope works too, and serialising does not invent fields.
    let again: SymbolInfo = reply.decode().expect("decode");
    let json = serde_json::to_value(&again).unwrap();
    for key in [
        "spread",
        "swap_long",
        "swap_short",
        "swap_mode",
        "trade_mode",
        "sessions",
        "trade_open_now",
    ] {
        assert!(json.get(key).is_none(), "{key} must be omitted when unset");
    }
}

#[test]
fn symbol_reply_may_carry_only_some_of_the_new_fields() {
    let mut data = fixture("get_symbol_legacy.reply.json")["data"].clone();
    data["spread"] = json!(12);
    data["trade_open_now"] = json!(false);
    let spec: SymbolInfo = round_trip("partial symbol reply", &data);
    assert_eq!(spec.spread, Some(12));
    assert_eq!(spec.trade_open_now, Some(false));
    assert!(spec.sessions.is_none() && spec.swap_mode.is_none());
}

#[test]
fn symbol_reply_with_no_session_today_has_empty_lists() {
    let mut data = fixture("get_symbol.reply.json")["data"].clone();
    data["sessions"] = json!({"quote": [], "trade": []});
    data["trade_open_now"] = json!(false);
    let spec: SymbolInfo = round_trip("weekend symbol reply", &data);
    let sessions = spec.sessions.expect("sessions");
    assert!(sessions.quote.is_empty() && sessions.trade.is_empty());
    assert!(!sessions.trade_open_at(43_200));
}

#[test]
fn symbol_reply_with_a_bad_new_field_is_rejected() {
    let base = fixture("get_symbol.reply.json")["data"].clone();
    for (key, bad) in [
        ("spread", json!("wide")),
        ("swap_long", json!("x")),
        ("trade_mode", json!("halfway")),
        ("trade_open_now", json!("yes")),
        ("sessions", json!({"quote": [[0, 1, 2]], "trade": []})),
        ("sessions", json!({"quote": [], "trade": [["a", "b"]]})),
        ("sessions", json!({"quote": []})),
    ] {
        let mut data = base.clone();
        data[key] = bad.clone();
        assert!(
            serde_json::from_value::<SymbolInfo>(data).is_err(),
            "{key} = {bad}"
        );
    }
}

#[test]
fn write_and_symbol_reply_fixtures_round_trip_and_decode() {
    for (base, cmd) in WRITE_FIXTURES {
        let name = format!("{base}.reply.json");
        let value = fixture(&name);
        let reply: Reply = round_trip(&name, &value);
        assert!(reply.ok, "{name}");
        let data = value["data"].clone();
        if cmd == "get_symbol" {
            assert_eq!(reply.retcode, 0, "{name}");
            let spec: SymbolInfo = round_trip(&name, &data);
            assert_eq!(spec.stops_level, 10);
            assert!(spec.trade_allowed);
            // The fixture is an EA 0.6.0 reply: all the later fields are there.
            assert_eq!(spec.spread, Some(28));
            assert_eq!(spec.swap_long, Some(-38.5));
            assert_eq!(spec.swap_short, Some(14.2));
            assert_eq!(spec.swap_mode.as_deref(), Some("points"));
            assert_eq!(spec.trade_mode, Some(SymbolTradeMode::Full));
            assert_eq!(spec.trade_open_now, Some(true));
            let sessions = spec.sessions.expect("sessions");
            assert_eq!(sessions.trade, vec![[0, 79_200], [82_800, 86_400]]);
            assert_eq!(sessions.quote, sessions.trade);
        } else {
            // A trade reply carries the trade retcode in the envelope, equal to the data's.
            let result: TradeResult = round_trip(&name, &data);
            assert_eq!(reply.retcode, result.retcode, "{name}");
            assert!(
                matches!(result.retcode, 10008..=10010),
                "{name}: a success reply has a success retcode"
            );
            assert!(!result.comment.is_empty());
        }
    }
}

#[test]
fn failed_write_fixtures_map_to_remote_errors_without_data() {
    for (name, retcode, part) in [
        ("order_send_refused.reply.json", -3, "InpAllowReal"),
        ("order_send_no_money.reply.json", 10019, "no money"),
        ("order_send_invalid_volume.reply.json", -1, "volume step"),
    ] {
        let value = fixture(name);
        let reply: Reply = round_trip(name, &value);
        assert!(
            !reply.ok && reply.data.is_none(),
            "{name}: error replies omit data"
        );
        match reply.decode::<TradeResult>() {
            Err(ReplyError::Remote {
                message,
                retcode: got,
            }) => {
                assert_eq!(got, retcode, "{name}");
                assert!(message.contains(part), "{name}: {message}");
            }
            other => panic!("{name}: expected Remote, got {other:?}"),
        }
    }
}

#[test]
fn write_arguments_are_serialised_without_nulls() {
    use jarvis_bridge_client::protocol::{
        ExecType, JARVIS_MAGIC, OrderCancelArgs, OrderSendArgs, PositionCloseArgs,
        PositionClosePartialArgs, PositionModifyArgs, Side,
    };
    let wire = |command: Command| {
        serde_json::to_value(Request {
            id: "i".into(),
            ts: None,
            command,
        })
        .unwrap()
    };

    assert_eq!(
        wire(Command::OrderSend(OrderSendArgs {
            symbol: "EURUSD".into(),
            side: Side::Sell,
            volume: 0.5,
            exec: ExecType::Stop,
            price: Some(1.07),
            sl: None,
            tp: None,
            deviation_points: None,
            magic: JARVIS_MAGIC,
            comment: String::new(),
        })),
        json!({"id": "i", "cmd": "order_send", "args": {
            "symbol": "EURUSD", "side": "sell", "volume": 0.5, "type": "stop", "price": 1.07,
            "magic": 770077, "comment": ""
        }})
    );
    // Absent = keep the current value, 0 = remove it: both are distinct on the wire.
    assert_eq!(
        wire(Command::PositionModify(PositionModifyArgs {
            ticket: 7,
            sl: Some(0.0),
            tp: None
        })),
        json!({"id": "i", "cmd": "position_modify", "args": {"ticket": 7, "sl": 0.0}})
    );
    assert_eq!(
        wire(Command::PositionClose(PositionCloseArgs {
            ticket: 7,
            deviation_points: None
        })),
        json!({"id": "i", "cmd": "position_close", "args": {"ticket": 7}})
    );
    assert_eq!(
        wire(Command::PositionClosePartial(PositionClosePartialArgs {
            ticket: 7,
            volume: 0.1,
            deviation_points: Some(30)
        })),
        json!({"id": "i", "cmd": "position_close_partial", "args": {"ticket": 7, "volume": 0.1, "deviation_points": 30}})
    );
    assert_eq!(
        wire(Command::OrderCancel(OrderCancelArgs { ticket: 9 })),
        json!({"id": "i", "cmd": "order_cancel", "args": {"ticket": 9}})
    );
    assert_eq!(
        wire(Command::GetSymbol {
            symbol: "XAUUSD".into()
        }),
        json!({"id": "i", "cmd": "get_symbol", "args": {"symbol": "XAUUSD"}})
    );
}

#[test]
fn only_the_five_write_commands_are_writes() {
    for cmd in COMMANDS.iter().copied().chain(["get_symbol"]) {
        let request: Request =
            serde_json::from_value(fixture(&format!("{cmd}.request.json"))).unwrap();
        assert!(!request.command.is_write(), "{cmd}");
    }
    for (base, _) in WRITE_FIXTURES.iter().skip(1) {
        let request: Request =
            serde_json::from_value(fixture(&format!("{base}.request.json"))).unwrap();
        assert!(request.command.is_write(), "{base}");
    }
}

#[test]
fn write_requests_with_bad_arguments_are_rejected() {
    for value in [
        json!({"id": "x", "cmd": "order_send", "args": {}}),
        json!({"id": "x", "cmd": "order_send", "args": {"symbol": "EURUSD", "side": "long", "volume": 0.1, "type": "market", "magic": 1, "comment": ""}}),
        json!({"id": "x", "cmd": "order_send", "args": {"symbol": "EURUSD", "side": "buy", "volume": 0.1, "type": "fok", "magic": 1, "comment": ""}}),
        json!({"id": "x", "cmd": "order_send", "args": {"symbol": "EURUSD", "side": "buy", "volume": "0.1", "type": "market", "magic": 1, "comment": ""}}),
        json!({"id": "x", "cmd": "order_send", "args": {"symbol": "EURUSD", "side": "buy", "volume": 0.1, "type": "market", "comment": ""}}),
        json!({"id": "x", "cmd": "position_modify", "args": {"sl": 1.0}}),
        json!({"id": "x", "cmd": "position_modify", "args": {"ticket": -1, "sl": 1.0}}),
        json!({"id": "x", "cmd": "position_close", "args": {"ticket": "7"}}),
        json!({"id": "x", "cmd": "position_close_partial", "args": {"ticket": 7}}),
        json!({"id": "x", "cmd": "order_cancel", "args": {}}),
    ] {
        assert!(
            serde_json::from_value::<Request>(value.clone()).is_err(),
            "{value}"
        );
    }
}

// ---- market view: get_rates and get_calendar -------------------------------------------------

mod market {
    use jarvis_bridge_client::protocol::{
        CALENDAR_RANGE_MAX_SECS, Calendar, CalendarQuery, Command, HISTORY_NOT_READY_RETCODES,
        Importance, RATES_COUNT_DEFAULT, RATES_COUNT_MAX, Rates, RatesQuery, Reply, ReplyError,
        Request, Timeframe,
    };
    use serde_json::json;

    use super::{fixture, round_trip};

    #[test]
    fn rates_and_calendar_requests_round_trip() {
        for (name, cmd) in [
            ("get_rates.request.json", "get_rates"),
            ("get_calendar.request.json", "get_calendar"),
            ("get_calendar_minimal.request.json", "get_calendar"),
        ] {
            let request: Request = round_trip(name, &fixture(name));
            assert_eq!(request.command.name(), cmd, "{name}");
            assert!(!request.command.is_write(), "{name}: a read command");
        }
    }

    #[test]
    fn rates_request_carries_the_documented_arguments() {
        let request: Request = serde_json::from_value(fixture("get_rates.request.json")).unwrap();
        assert_eq!(
            request.command,
            Command::GetRates(RatesQuery {
                symbol: "XAUUSD_".into(),
                timeframe: Timeframe::M15,
                count: Some(3),
            })
        );
    }

    #[test]
    fn rates_reply_round_trips_with_bars_oldest_first() {
        let value = fixture("get_rates.reply.json");
        let reply: Reply = round_trip("get_rates.reply.json", &value);
        assert!(reply.ok);
        let rates: Rates = round_trip("get_rates.reply.json data", &value["data"]);
        assert_eq!(rates.timeframe, Timeframe::M15);
        assert_eq!(rates.digits, 2);
        assert_eq!(rates.bars.len(), 3);
        assert!(rates.bars.windows(2).all(|w| w[0].time < w[1].time));
        for bar in &rates.bars {
            assert!(bar.high >= bar.open.max(bar.close), "{bar:?}");
            assert!(bar.low <= bar.open.min(bar.close), "{bar:?}");
        }
    }

    #[test]
    fn calendar_reply_round_trips_with_null_values() {
        let value = fixture("get_calendar.reply.json");
        let _: Reply = round_trip("get_calendar.reply.json", &value);
        let calendar: Calendar = round_trip("get_calendar.reply.json data", &value["data"]);
        assert_eq!(calendar.events.len(), 4);
        let released = &calendar.events[0];
        assert_eq!(released.actual, Some(87.9));
        let upcoming = &calendar.events[1];
        assert_eq!((upcoming.actual, upcoming.unit.as_str()), (None, "K"));
        let speech = &calendar.events[2];
        assert_eq!(
            (speech.actual, speech.forecast, speech.previous),
            (None, None, None)
        );
        assert!(calendar.events.windows(2).all(|w| w[0].time <= w[1].time));
    }

    #[test]
    fn calendar_nulls_are_sent_as_null_not_omitted() {
        let calendar: Calendar =
            serde_json::from_value(fixture("get_calendar.reply.json")["data"].clone()).unwrap();
        let wire = serde_json::to_value(&calendar.events[2]).unwrap();
        assert_eq!(wire["actual"], json!(null));
        let object = wire.as_object().unwrap();
        assert!(object.contains_key("forecast") && object.contains_key("previous"));
    }

    #[test]
    fn history_not_ready_reply_is_a_remote_error_with_the_mt5_code() {
        let value = fixture("get_rates_not_ready.reply.json");
        let reply: Reply = round_trip("get_rates_not_ready.reply.json", &value);
        match reply.decode::<Rates>() {
            Err(ReplyError::Remote { retcode, .. }) => {
                assert_eq!(retcode, 4401);
                assert!(HISTORY_NOT_READY_RETCODES.contains(&retcode));
            }
            other => panic!("expected Remote, got {other:?}"),
        }
    }

    #[test]
    fn optional_arguments_are_left_out_of_the_request() {
        let wire = |command: Command| {
            serde_json::to_value(Request {
                id: "i".into(),
                ts: None,
                command,
            })
            .unwrap()
        };
        assert_eq!(
            wire(Command::GetRates(RatesQuery {
                symbol: "EURUSD".into(),
                timeframe: Timeframe::H4,
                count: None,
            })),
            json!({"id": "i", "cmd": "get_rates", "args": {"symbol": "EURUSD", "timeframe": "H4"}})
        );
        assert_eq!(
            wire(Command::GetCalendar(CalendarQuery {
                from: 1,
                to: 2,
                currencies: vec![],
                min_importance: None,
            })),
            json!({"id": "i", "cmd": "get_calendar", "args": {"from": 1, "to": 2}})
        );
    }

    #[test]
    fn timeframe_and_importance_use_the_documented_strings() {
        for (tf, text) in [
            (Timeframe::M1, "M1"),
            (Timeframe::M5, "M5"),
            (Timeframe::M15, "M15"),
            (Timeframe::M30, "M30"),
            (Timeframe::H1, "H1"),
            (Timeframe::H4, "H4"),
            (Timeframe::D1, "D1"),
        ] {
            assert_eq!(serde_json::to_value(tf).unwrap(), json!(text));
            assert_eq!(
                serde_json::from_value::<Timeframe>(json!(text)).unwrap(),
                tf
            );
            assert_eq!(text.parse::<Timeframe>().unwrap(), tf);
        }
        // The wire is exact; only the command-line parser is case-insensitive.
        for text in ["m15", "W1", "H2", ""] {
            assert!(serde_json::from_value::<Timeframe>(json!(text)).is_err());
        }
        assert_eq!("h1".parse::<Timeframe>().unwrap(), Timeframe::H1);
        assert!("W1".parse::<Timeframe>().is_err());

        for (imp, text) in [
            (Importance::Low, "low"),
            (Importance::Moderate, "moderate"),
            (Importance::High, "high"),
        ] {
            assert_eq!(serde_json::to_value(imp).unwrap(), json!(text));
            assert_eq!(
                serde_json::from_value::<Importance>(json!(text)).unwrap(),
                imp
            );
        }
        assert!(serde_json::from_value::<Importance>(json!("none")).is_err());
        assert!(Importance::High >= Importance::Moderate);
        assert!(Importance::Moderate > Importance::Low);
        assert!(Importance::Low < Importance::High);
    }

    #[test]
    fn limits_match_the_documented_contract() {
        assert_eq!(RATES_COUNT_MAX, 1000);
        assert_eq!(RATES_COUNT_DEFAULT, 300);
        assert_eq!(CALENDAR_RANGE_MAX_SECS, 14 * 86_400);
    }

    #[test]
    fn rates_query_validation() {
        let query = |symbol: &str, count: Option<u32>| RatesQuery {
            symbol: symbol.into(),
            timeframe: Timeframe::M15,
            count,
        };
        assert!(query("XAUUSD", None).validate().is_ok());
        assert!(query("XAUUSD", Some(1)).validate().is_ok());
        assert!(query("XAUUSD", Some(1000)).validate().is_ok());
        let zero = query("XAUUSD", Some(0)).validate().unwrap_err();
        assert!(zero.contains("between 1 and 1000"), "{zero}");
        assert!(query("XAUUSD", Some(1001)).validate().is_err());
        assert!(query("  ", None).validate().unwrap_err().contains("symbol"));
    }

    #[test]
    fn calendar_query_validation() {
        let query = |from: i64, to: i64, currencies: &[&str]| CalendarQuery {
            from,
            to,
            currencies: currencies.iter().map(|c| (*c).to_owned()).collect(),
            min_importance: None,
        };
        assert!(query(100, 101, &[]).validate().is_ok());
        assert!(
            query(0, CALENDAR_RANGE_MAX_SECS, &["USD", "eur"])
                .validate()
                .is_ok()
        );
        assert!(
            query(100, 100, &[])
                .validate()
                .unwrap_err()
                .contains("after")
        );
        assert!(query(100, 50, &[]).validate().is_err());
        assert!(
            query(0, CALENDAR_RANGE_MAX_SECS + 1, &[])
                .validate()
                .unwrap_err()
                .contains("14 days")
        );
        for bad in ["US", "USDX", "U5D", ""] {
            assert!(query(0, 10, &[bad]).validate().is_err(), "{bad:?}");
        }
    }
}
