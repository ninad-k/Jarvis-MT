# Spec: assistant actions, alerts, risk sizing, briefing, symbol info

Status: approved 2026-10-30. Builds on the market view feature (spec-market-view.md).
Decision record: ADR-0020.

## Objective

Let the trader do from chat or voice what today needs a click in the HUD or Settings, and
add four assistant features:

1. **Preference tools**: change the watchlist, persona, model and spoken names; engage the
   kill switch and tighten limits. Chat can never loosen a limit or release the kill switch.
2. **Price alerts**: "tell me when gold reaches 3950".
3. **Risk-based lot size**: "buy gold risking 1% with a stop at 3900".
4. **Daily briefing**: "morning brief".
5. **Symbol info**: "what's the spread and minimum lot on gold?", "is the market open?".

## Safety rule change

SPEC rule 1 becomes: "The LLM can call read tools, `propose_*` tools and a closed set of
preference tools. Preference tools change only the user's own preferences or make trading
stricter (engage the kill switch, tighten a limit). No tool executes a trade, loosens a limit
or releases the kill switch." Tests prove each part. Every preference change is audited
(`kind = preference`, tool, before, after) and published as the existing `settings_changed`
or `kill_switch` WebSocket event so the HUD updates at once.

## 1. Preference tools (new closed enum `PreferenceTool`, separate from read and propose)

| Tool | Arguments | Effect | Refusals |
|---|---|---|---|
| `update_watchlist` | `add?: [name]`, `remove?: [name]` | Same validation as `PUT /api/settings` (names resolved to canonical, 20 max, no duplicates) | unknown symbol, over the cap |
| `set_persona` | `persona` (`jarvis` or `friday`, or any installed pack id) | Same as the HUD toggle | unknown persona |
| `set_model` | `provider` (configured provider name, or `default`) | Same as the HUD menu; takes effect from the next turn | unknown or unavailable provider |
| `set_alias` | `spoken`, `symbol` | Adds or replaces a spoken name (same validation as Settings) | invalid name, unknown symbol |
| `remove_alias` | `spoken` | Removes it | not found |
| `engage_kill_switch` | `reason?` | Engages the kill switch (persisted, as the HUD button does) | none |
| `tighten_limit` | `rule`: `max_lot` / `max_positions` / `max_daily_loss`, `value` | Applies a limit override through `limit_overrides::validate_stricter` | not stricter ("loosening is only possible in Settings") |

Results tell the model exactly what changed, in plain words with the new values.

## 2. Price alerts

- Table `price_alerts` (migration 0008): id, settings key (user), canonical, broker_symbol,
  direction (`above` or `below`), price, note, created_at, triggered_at, trigger_price,
  status (`active`, `triggered`, `cancelled`). At most 50 active alerts per user.
- Direction is inferred from the current price when not given (target above the bid →
  `above`). `above` fires when bid ≥ price; `below` fires when ask ≤ price. One-shot.
- Watcher task: every 3 seconds, one `get_price` per symbol with active alerts (deduplicated),
  stops on shutdown, survives bridge errors.
- WebSocket event `alert_triggered {id, symbol, direction, price, trigger_price, note,
  triggered_at}`.
- Tools: `create_price_alert {symbol, price, direction?, note?}`, `list_price_alerts {}`,
  `cancel_price_alert {id?, symbol?}` (`symbol` cancels all active alerts on it). These are
  preference tools.
- HTTP: `GET /api/alerts` (active and the 20 most recent triggered), `POST /api/alerts`
  `{symbol, price, direction?, note?}`, `DELETE /api/alerts/{id}`.

## 3. Risk-based lot size

- `propose_order` gains `risk_percent` (0.1 to `max_risk_percent`), allowed only with a
  `stop_loss` and instead of `volume`. `POST /api/market/prepare` accepts `risk_percent`
  instead of `volume` the same way.
- Volume = (equity × risk% / 100) ÷ (|entry − stop| ÷ tick_size × tick_value), rounded
  DOWN to `volume_step`. Entry is the ask for a buy, the bid for a sell, or the limit price.
  Below `volume_min` → refused with the numbers ("1% risk is 39.20 USD; the smallest lot
  risks 52.10 USD"). Then every gate limit applies as usual.
- New `[limits] max_risk_percent = 2.0` (tighten-only like other limits).
- The preview carries `sizing: {risk_percent, risk_amount, currency, stop_distance, volume}`
  and its description says "1% risk (39.20 USD)". The intent parser recognises "risking 1%",
  "1 percent risk" so the cross-check accepts risk instead of a volume.

## 4. Daily briefing

- Read tool `get_briefing {}` and `GET /api/briefing`: account summary, open positions with
  floating P/L, today's high-impact events for the watchlist currencies (server day, shown in
  UTC), and a short view (bias, confidence, stand aside) for each watchlist symbol, computed
  concurrently, at most 8 symbols, each with a 20-second budget (a failed one is listed as
  unavailable). Returns `{generated_at, account, positions, events, views, summary}`.
- The briefing's views are journaled like any other view.

## 5. Symbol info

- EA 0.6.0: `get_symbol` gains `spread` (points, current), `swap_long`, `swap_short`,
  `trade_mode` (as in the catalog), and `sessions` for the current server weekday:
  `{"quote": [[from_sec, to_sec]], "trade": [[from_sec, to_sec]]}` (seconds since 00:00 server
  time, from `SymbolInfoSessionQuote`/`SymbolInfoSessionTrade`), plus `trade_open_now`.
  Older EAs omit them; the Rust fields are optional.
- Read tool `get_symbol_info {symbol}`: canonical and broker name, digits, spread in points
  and in price, contract size, volume min, max and step, tick size and value, stops level,
  swaps, trade mode, today's sessions in UTC, open now.

## Web

- Alerts: a bell on each watchlist row (create with a price), an Alerts list (active and
  recent, cancel), and on `alert_triggered` a toast, a short sound and, when voice output is
  on, a spoken line through the existing speak endpoint.
- Settings, persona toggle, model menu, watchlist and kill switch already refresh on
  `settings_changed`/`kill_switch`; verify they do when the change comes from chat.
- Prepare trade dialog: "Volume" or "Risk %" choice; previews show the sizing line.
- Briefing: a "Brief me" button that shows the briefing card.
- Tool chip labels for every new tool.

## Success criteria

- Each tool works from chat against the mock, with tests; refusals are clear.
- Tests prove chat cannot loosen a limit, release the kill switch or execute a trade.
- Alerts fire once, survive a restart (persisted) and reach the browser.
- Risk sizing matches hand-computed lots for gold, EURUSD and an index.
- `scripts/check.ps1` stays green.
