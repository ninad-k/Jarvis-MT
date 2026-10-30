use std::collections::{BTreeSet, HashMap};

use jarvis_bridge_client::protocol::{
    Account, Command, ExecType, JARVIS_MAGIC, Order, OrderCancelArgs, OrderSendArgs, OrderType,
    Position, PositionCloseArgs, PositionClosePartialArgs, PositionModifyArgs, Price, Side,
    SymbolInfo,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{GateError, Limits};

/// What the LLM (via a `propose_*` tool) or a UI button asks for. Symbols are
/// already resolved to broker names by the caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ProposedAction {
    Order(OrderIntent),
    Modify(ModifyIntent),
    Close(CloseSelector),
    Cancel(CancelSelector),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderIntent {
    pub symbol: String,
    pub side: Side,
    pub volume: f64,
    pub exec: ExecType,
    /// Required for limit/stop.
    pub price: Option<f64>,
    pub sl: Option<f64>,
    pub tp: Option<f64>,
}

/// `None` keeps the current value, `Some(0.0)` removes it. At least one must be set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModifyIntent {
    pub ticket: u64,
    pub sl: Option<f64>,
    pub tp: Option<f64>,
}

/// Which positions to close. Exactly one targeting mode:
/// - `tickets` non-empty: those positions (with optional `volume` for a partial close
///   of a single ticket);
/// - otherwise filters `symbol` and/or `side` (both `None` + `all = true` = close all).
///
/// More than one position matched => batch (rule 4).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CloseSelector {
    #[serde(default)]
    pub tickets: Vec<u64>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub side: Option<Side>,
    /// Must be true to close everything when no other filter is given.
    #[serde(default)]
    pub all: bool,
    /// Partial close volume; only with exactly one ticket.
    #[serde(default)]
    pub volume: Option<f64>,
}

/// Which pending orders to cancel (same rules as [`CloseSelector`], no volume).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CancelSelector {
    #[serde(default)]
    pub tickets: Vec<u64>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub all: bool,
}

/// Everything [`evaluate`] needs, fetched by the caller from the bridge just before.
#[derive(Debug, Clone, PartialEq)]
pub struct MarketSnapshot {
    pub account: Account,
    pub positions: Vec<Position>,
    pub orders: Vec<Order>,
    /// Current prices of every symbol involved.
    pub prices: HashMap<String, Price>,
    /// Trading specs of every symbol involved.
    pub symbols: HashMap<String, SymbolInfo>,
    /// Broker server time, Unix seconds.
    pub server_time: i64,
    /// Realised P/L (profit + commission + swap) of deals since server midnight today.
    pub realized_pnl_today: f64,
}

/// Estimated P/L in account currency. For closes: `current` is the floating P/L
/// being realised. For new orders: P/L if SL / TP is hit (from tick value), `None`
/// when not set.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EstPnl {
    pub current: Option<f64>,
    pub at_sl: Option<f64>,
    pub at_tp: Option<f64>,
}

/// What the user sees and confirms (SPEC rule 2). `commands` are the exact bridge
/// writes executed on confirmation, in order; nothing else is ever sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionPreview {
    pub id: Uuid,
    /// One-line, voice-friendly, e.g. "Buy 0.10 XAUUSD at market, SL 3990.00, TP 4020.00".
    pub description: String,
    pub symbols: Vec<String>,
    pub tickets: Vec<u64>,
    pub total_volume: f64,
    pub est_pnl: EstPnl,
    /// True when more than one position/order is affected (needs second confirmation).
    pub is_batch: bool,
    /// Number of bridge writes (positions/orders affected).
    pub count: usize,
    /// Non-blocking notes, e.g. "spread is wide (45 points)".
    pub warnings: Vec<String>,
    pub commands: Vec<Command>,
}

/// Check `action` against `limits` and the snapshot and build the preview.
///
/// Volumes are rounded DOWN to `volume_step` (and must stay >= `volume_min`), prices
/// to `digits`; SL/TP must be on the correct side and at least `stops_level` points
/// away. New orders get `magic = JARVIS_MAGIC` and comment `"jarvis:<first 8 hex of id>"`.
/// `kill_switch_engaged` makes every action fail with `KillSwitchEngaged`.
///
/// # Order of checks for a new order
///
/// kill switch, `allowed_symbols`, symbol spec and price present, `trade_allowed`,
/// volume (min, broker max, `max_lot`), `max_positions`, `trading_hours`,
/// `max_daily_loss`, then price/SL/TP validation. The first failure is reported.
///
/// # What the limits do not cover
///
/// Daily loss, trading hours, `allowed_symbols` and `max_positions` apply to **new
/// orders only**. Modifying, closing and cancelling reduce or keep risk, so they stay
/// possible in all of those situations (only the kill switch blocks them). `max_lot`
/// is likewise not applied to closes: it caps new exposure, and a batch close removes
/// exposure no matter how large it is.
///
/// # Notes for callers
///
/// - The snapshot needs `symbols` and `prices` for the traded symbol (new orders,
///   modify) and `symbols` for the closed symbol (partial close only). Closing
///   and cancelling work from positions/orders alone.
/// - A ticket that does not exist is [`GateError::Invalid`]; a filter that matches
///   nothing is [`GateError::NothingToDo`].
/// - For new orders an SL/TP of `Some(0.0)` is read as "not set".
pub fn evaluate(
    action: &ProposedAction,
    snapshot: &MarketSnapshot,
    limits: &Limits,
    kill_switch_engaged: bool,
) -> Result<ActionPreview, GateError> {
    if kill_switch_engaged {
        return Err(GateError::KillSwitchEngaged);
    }
    match action {
        ProposedAction::Order(o) => eval_order(o, snapshot, limits),
        ProposedAction::Modify(m) => eval_modify(m, snapshot),
        ProposedAction::Close(c) => eval_close(c, snapshot),
        ProposedAction::Cancel(c) => eval_cancel(c, snapshot),
    }
}

// ---- numeric and text helpers ------------------------------------------------------

/// Tolerance for float comparisons on volumes and step arithmetic.
const EPS: f64 = 1e-9;
/// Spread (in points) above which a preview carries a warning.
const WIDE_SPREAD_POINTS: i64 = 50;

fn pow10(decimals: u32) -> f64 {
    10f64.powi(decimals as i32)
}

fn round_to(x: f64, decimals: u32) -> f64 {
    let m = pow10(decimals);
    (x * m).round() / m
}

/// Number of decimals needed to write `step` exactly (0.01 -> 2, 0.5 -> 1, 1.0 -> 0).
fn step_decimals(step: f64) -> u32 {
    (0..=8)
        .find(|d| {
            let v = step * pow10(*d);
            (v - v.round()).abs() < 1e-7
        })
        .unwrap_or(8)
}

/// Round `volume` DOWN to a multiple of `step`, tolerating binary float noise
/// (0.29 / 0.01 is 28.999999999999996 and must still give 0.29).
fn round_volume_down(volume: f64, step: f64) -> f64 {
    let steps = (volume / step + EPS).floor();
    round_to(steps * step, step_decimals(step))
}

/// "0.10", "1.00", "0.057": at least two decimals, no trailing noise.
fn fmt_volume(v: f64) -> String {
    let mut s = format!("{:.8}", round_to(v, 8));
    while s.ends_with('0') && s.split('.').nth(1).is_some_and(|d| d.len() > 2) {
        s.pop();
    }
    s
}

fn fmt_lots(v: f64) -> String {
    let unit = if (v - 1.0).abs() < EPS { "lot" } else { "lots" };
    format!("{} {unit}", fmt_volume(v))
}

fn fmt_price(p: f64, digits: u32) -> String {
    format!("{:.*}", digits as usize, p)
}

fn fmt_money(v: f64) -> String {
    let r = round_to(v, 2);
    // Avoid printing "-0.00".
    let r = if r == 0.0 { 0.0 } else { r };
    format!("{r:.2}")
}

fn side_word(side: Side) -> &'static str {
    match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

fn direction(side: Side) -> f64 {
    match side {
        Side::Buy => 1.0,
        Side::Sell => -1.0,
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// "XAUUSD", "XAUUSD and US30", "A, B and C".
fn join_symbols(symbols: &[String]) -> String {
    match symbols {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn order_type_text(t: OrderType) -> &'static str {
    match t {
        OrderType::BuyLimit => "buy limit",
        OrderType::SellLimit => "sell limit",
        OrderType::BuyStop => "buy stop",
        OrderType::SellStop => "sell stop",
        OrderType::BuyStopLimit => "buy stop limit",
        OrderType::SellStopLimit => "sell stop limit",
    }
}

fn invalid(msg: impl Into<String>) -> GateError {
    GateError::Invalid(msg.into())
}

fn spec_of<'a>(snapshot: &'a MarketSnapshot, symbol: &str) -> Result<&'a SymbolInfo, GateError> {
    snapshot
        .symbols
        .get(symbol)
        .ok_or_else(|| invalid(format!("no trading specification available for {symbol}")))
}

fn price_of<'a>(snapshot: &'a MarketSnapshot, symbol: &str) -> Result<&'a Price, GateError> {
    snapshot
        .prices
        .get(symbol)
        .ok_or_else(|| invalid(format!("no current price for {symbol}")))
}

/// P/L in account currency of moving from `entry` to `exit` with `volume` lots.
fn pnl_between(
    spec: &SymbolInfo,
    side: Side,
    entry: f64,
    exit: f64,
    volume: f64,
) -> Result<f64, GateError> {
    if !(spec.tick_size > 0.0 && spec.tick_value.is_finite() && spec.tick_value >= 0.0) {
        return Err(invalid(format!(
            "{} has no usable tick size/value",
            spec.symbol
        )));
    }
    let raw = (exit - entry) * direction(side) / spec.tick_size * spec.tick_value * volume;
    Ok(round_to(raw, 2))
}

/// Validate and round a user-supplied price level. `None` stays `None`.
fn clean_price(name: &str, value: Option<f64>, digits: u32) -> Result<Option<f64>, GateError> {
    match value {
        None => Ok(None),
        Some(v) if !v.is_finite() || v < 0.0 => {
            Err(invalid(format!("{name} {v} is not a valid price")))
        }
        Some(v) => Ok(Some(round_to(v, digits))),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LevelKind {
    StopLoss,
    TakeProfit,
}

impl LevelKind {
    fn name(self) -> &'static str {
        match self {
            LevelKind::StopLoss => "stop loss",
            LevelKind::TakeProfit => "take profit",
        }
    }
}

/// Where a protective level is measured from.
struct LevelCheck<'a> {
    spec: &'a SymbolInfo,
    side: Side,
    /// Price the trade is (or will be) opened at; the level must be on the right side.
    entry: f64,
    /// Price MT5 measures the minimum distance from.
    reference: f64,
}

impl LevelCheck<'_> {
    fn min_distance(&self) -> f64 {
        self.spec.stops_level.max(0) as f64 * self.spec.point
    }

    /// Half a point of slack so a level exactly `stops_level` away passes despite
    /// float noise in the subtraction.
    fn tolerance(&self) -> f64 {
        self.spec.point * 0.5
    }

    fn check(&self, kind: LevelKind, level: f64) -> Result<(), GateError> {
        let digits = self.spec.digits;
        let below_entry = match (kind, self.side) {
            (LevelKind::StopLoss, Side::Buy) | (LevelKind::TakeProfit, Side::Sell) => true,
            (LevelKind::StopLoss, Side::Sell) | (LevelKind::TakeProfit, Side::Buy) => false,
        };
        let on_right_side = if below_entry {
            level < self.entry
        } else {
            level > self.entry
        };
        if !on_right_side {
            return Err(invalid(format!(
                "{} {} must be {} the entry price {} for a {}",
                kind.name(),
                fmt_price(level, digits),
                if below_entry { "below" } else { "above" },
                fmt_price(self.entry, digits),
                side_word(self.side),
            )));
        }
        let distance = if below_entry {
            self.reference - level
        } else {
            level - self.reference
        };
        if distance < self.min_distance() - self.tolerance() {
            return Err(invalid(format!(
                "{} {} is too close to the market (minimum distance {})",
                kind.name(),
                fmt_price(level, digits),
                fmt_price(self.min_distance(), digits),
            )));
        }
        Ok(())
    }
}

/// Targeting mode shared by close and cancel.
enum Targets {
    Tickets(Vec<u64>),
    Filter,
}

fn targets(
    tickets: &[u64],
    has_filter: bool,
    all: bool,
    ambiguous_question: &str,
) -> Result<Targets, GateError> {
    if !tickets.is_empty() {
        if has_filter || all {
            return Err(invalid(
                "give either specific tickets or filters (symbol, side, all), not both",
            ));
        }
        let mut sorted = tickets.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        return Ok(Targets::Tickets(sorted));
    }
    if !has_filter && !all {
        return Err(GateError::Ambiguous {
            question: ambiguous_question.to_owned(),
        });
    }
    Ok(Targets::Filter)
}

fn missing_tickets_error(missing: &[u64], noun: &str) -> GateError {
    let list = missing
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    invalid(format!("no {noun} with ticket {list}"))
}

// ---- new orders --------------------------------------------------------------------

fn eval_order(
    o: &OrderIntent,
    snapshot: &MarketSnapshot,
    limits: &Limits,
) -> Result<ActionPreview, GateError> {
    // 1. allowed symbols (exact broker names)
    if !limits.allowed_symbols.iter().any(|s| s == &o.symbol) {
        return Err(GateError::LimitExceeded {
            rule: "allowed_symbols",
            detail: format!(
                "{} is not on the allowed list ({})",
                o.symbol,
                if limits.allowed_symbols.is_empty() {
                    "no symbols are allowed".to_owned()
                } else {
                    limits.allowed_symbols.join(", ")
                }
            ),
        });
    }

    // 2. snapshot completeness and tradability
    let spec = spec_of(snapshot, &o.symbol)?;
    let market = price_of(snapshot, &o.symbol)?;
    if !spec.trade_allowed {
        return Err(invalid(format!(
            "{} is not open for trading (close-only or disabled)",
            o.symbol
        )));
    }

    // 3. volume: step, broker min/max, max_lot
    let mut warnings = Vec::new();
    let volume = checked_volume(o.volume, spec, limits, &mut warnings)?;

    // 4. max_positions: open positions + pending orders after this one
    let after = snapshot.positions.len() + snapshot.orders.len() + 1;
    if after > limits.max_positions as usize {
        return Err(GateError::LimitExceeded {
            rule: "max_positions",
            detail: format!(
                "this would make {after} open positions and pending orders; the limit is {}",
                limits.max_positions
            ),
        });
    }

    // 5. trading hours (broker server time)
    if let Some(hours) = &limits.trading_hours
        && !hours.contains(snapshot.server_time)
    {
        return Err(GateError::LimitExceeded {
            rule: "trading_hours",
            detail: format!(
                "new orders are only allowed {}-{} server time on the configured days",
                hours.start, hours.end
            ),
        });
    }

    // 6. daily loss: realised today + floating
    let floating: f64 = snapshot.positions.iter().map(|p| p.profit).sum();
    let today = snapshot.realized_pnl_today + floating;
    if today <= -limits.max_daily_loss {
        let currency = &snapshot.account.currency;
        return Err(GateError::LimitExceeded {
            rule: "max_daily_loss",
            detail: format!(
                "today's P/L is {} {currency}, at or beyond the daily loss limit of {} {currency}",
                fmt_money(today),
                fmt_money(limits.max_daily_loss),
            ),
        });
    }

    // 7. price, SL and TP
    let digits = spec.digits;
    let (pending_price, entry, reference) = match o.exec {
        ExecType::Market => {
            if o.price.is_some() {
                return Err(invalid(
                    "a market order takes no price; use a limit or stop order for a specific price",
                ));
            }
            // Fill at the ask (buy) or bid (sell); MT5 measures SL/TP distance from the
            // price the position would be closed at.
            match o.side {
                Side::Buy => (None, market.ask, market.bid),
                Side::Sell => (None, market.bid, market.ask),
            }
        }
        ExecType::Limit | ExecType::Stop => {
            let price = clean_price("price", o.price, digits)?
                .filter(|p| *p > 0.0)
                .ok_or_else(|| invalid("a limit or stop order needs a price"))?;
            check_pending(o.exec, o.side, price, market, spec)?;
            (Some(price), price, price)
        }
    };
    // Zero means "not set" for a new order.
    let sl = clean_price("stop loss", o.sl, digits)?.filter(|v| *v > 0.0);
    let tp = clean_price("take profit", o.tp, digits)?.filter(|v| *v > 0.0);
    let check = LevelCheck {
        spec,
        side: o.side,
        entry,
        reference,
    };
    if let Some(v) = sl {
        check.check(LevelKind::StopLoss, v)?;
    }
    if let Some(v) = tp {
        check.check(LevelKind::TakeProfit, v)?;
    }

    // est_pnl at SL / TP
    let at_sl = sl
        .map(|v| pnl_between(spec, o.side, entry, v, volume))
        .transpose()?;
    let at_tp = tp
        .map(|v| pnl_between(spec, o.side, entry, v, volume))
        .transpose()?;

    // warnings
    if market.spread_points > WIDE_SPREAD_POINTS {
        warnings.push(format!("Spread is wide ({} points)", market.spread_points));
    }
    if sl.is_none() {
        warnings.push("No stop loss set".to_owned());
    }

    // description
    let mut description = match pending_price {
        None => format!(
            "{} {} {} at market",
            capitalize(side_word(o.side)),
            fmt_volume(volume),
            o.symbol
        ),
        Some(p) => format!(
            "{} {} {} {} at {}",
            capitalize(side_word(o.side)),
            exec_word(o.exec),
            fmt_volume(volume),
            o.symbol,
            fmt_price(p, digits)
        ),
    };
    if let Some(v) = sl {
        description.push_str(&format!(", SL {}", fmt_price(v, digits)));
    }
    if let Some(v) = tp {
        description.push_str(&format!(", TP {}", fmt_price(v, digits)));
    }

    let id = Uuid::new_v4();
    let command = Command::OrderSend(OrderSendArgs {
        symbol: o.symbol.clone(),
        side: o.side,
        volume,
        exec: o.exec,
        price: pending_price,
        sl,
        tp,
        // The EA default applies (20 points); callers may override per config later.
        deviation_points: None,
        magic: JARVIS_MAGIC,
        comment: format!("jarvis:{}", &id.simple().to_string()[..8]),
    });
    Ok(ActionPreview {
        id,
        description,
        symbols: vec![o.symbol.clone()],
        tickets: vec![],
        total_volume: volume,
        est_pnl: EstPnl {
            current: None,
            at_sl,
            at_tp,
        },
        is_batch: false,
        count: 1,
        warnings,
        commands: vec![command],
    })
}

fn exec_word(exec: ExecType) -> &'static str {
    match exec {
        ExecType::Market => "market",
        ExecType::Limit => "limit",
        ExecType::Stop => "stop",
    }
}

/// Round `requested` down to the step and check it against the broker's and the
/// configured limits. Pushes a warning when rounding changed the volume.
fn checked_volume(
    requested: f64,
    spec: &SymbolInfo,
    limits: &Limits,
    warnings: &mut Vec<String>,
) -> Result<f64, GateError> {
    if !requested.is_finite() || requested <= 0.0 {
        return Err(invalid(format!("volume {requested} is not valid")));
    }
    if !(spec.volume_step.is_finite() && spec.volume_step > 0.0) {
        return Err(invalid(format!("{} has no valid volume step", spec.symbol)));
    }
    let volume = round_volume_down(requested, spec.volume_step);
    if volume < spec.volume_min - EPS {
        return Err(invalid(format!(
            "volume {} is below the minimum of {} lots for {} (step {})",
            fmt_volume(requested),
            fmt_volume(spec.volume_min),
            spec.symbol,
            fmt_volume(spec.volume_step),
        )));
    }
    if volume > spec.volume_max + EPS {
        return Err(invalid(format!(
            "volume {} is above the broker maximum of {} lots for {}",
            fmt_volume(volume),
            fmt_volume(spec.volume_max),
            spec.symbol,
        )));
    }
    if volume > limits.max_lot + EPS {
        return Err(GateError::LimitExceeded {
            rule: "max_lot",
            detail: format!(
                "volume {} exceeds the maximum of {} lots per order",
                fmt_volume(volume),
                fmt_volume(limits.max_lot),
            ),
        });
    }
    if volume < requested - EPS {
        warnings.push(format!(
            "Volume rounded down from {} to {} to fit the lot step",
            fmt_volume(requested),
            fmt_volume(volume)
        ));
    }
    Ok(volume)
}

/// A pending price must be on the correct side of the market and at least
/// `stops_level` away from it.
fn check_pending(
    exec: ExecType,
    side: Side,
    price: f64,
    market: &Price,
    spec: &SymbolInfo,
) -> Result<(), GateError> {
    let digits = spec.digits;
    // (must the price be below the reference, reference price, reference name)
    let (below, reference, ref_name) = match (exec, side) {
        (ExecType::Limit, Side::Buy) => (true, market.ask, "ask"),
        (ExecType::Limit, Side::Sell) => (false, market.bid, "bid"),
        (ExecType::Stop, Side::Buy) => (false, market.ask, "ask"),
        (ExecType::Stop, Side::Sell) => (true, market.bid, "bid"),
        (ExecType::Market, _) => return Ok(()),
    };
    let right_side = if below {
        price < reference
    } else {
        price > reference
    };
    if !right_side {
        return Err(invalid(format!(
            "a {} {} order must be priced {} the current {ref_name} {}, got {}",
            side_word(side),
            exec_word(exec),
            if below { "below" } else { "above" },
            fmt_price(reference, digits),
            fmt_price(price, digits),
        )));
    }
    let distance = if below {
        reference - price
    } else {
        price - reference
    };
    let min = spec.stops_level.max(0) as f64 * spec.point;
    if distance < min - spec.point * 0.5 {
        return Err(invalid(format!(
            "the {} {} price {} is too close to the market (minimum distance {})",
            side_word(side),
            exec_word(exec),
            fmt_price(price, digits),
            fmt_price(min, digits),
        )));
    }
    Ok(())
}

// ---- modify ------------------------------------------------------------------------

/// What a modify request does to one protective level.
#[derive(Clone, Copy, PartialEq)]
enum LevelChange {
    Keep,
    Set(f64),
    Remove,
}

impl LevelChange {
    fn wire(self) -> Option<f64> {
        match self {
            LevelChange::Keep => None,
            LevelChange::Set(v) => Some(v),
            LevelChange::Remove => Some(0.0),
        }
    }
}

fn level_change(
    kind: LevelKind,
    requested: Option<f64>,
    current: Option<f64>,
    spec: &SymbolInfo,
) -> Result<LevelChange, GateError> {
    let Some(v) = clean_price(kind.name(), requested, spec.digits)? else {
        return Ok(LevelChange::Keep);
    };
    let current = current.filter(|c| *c > 0.0);
    if v == 0.0 {
        return Ok(if current.is_some() {
            LevelChange::Remove
        } else {
            LevelChange::Keep
        });
    }
    let unchanged = current.is_some_and(|c| (c - v).abs() < spec.point * 0.5);
    Ok(if unchanged {
        LevelChange::Keep
    } else {
        LevelChange::Set(v)
    })
}

fn eval_modify(m: &ModifyIntent, snapshot: &MarketSnapshot) -> Result<ActionPreview, GateError> {
    let position = snapshot
        .positions
        .iter()
        .find(|p| p.ticket == m.ticket)
        .ok_or_else(|| missing_tickets_error(&[m.ticket], "open position"))?;
    if m.sl.is_none() && m.tp.is_none() {
        return Err(invalid("give a new stop loss, a new take profit, or both"));
    }
    let spec = spec_of(snapshot, &position.symbol)?;
    let market = price_of(snapshot, &position.symbol)?;

    let sl = level_change(LevelKind::StopLoss, m.sl, position.sl, spec)?;
    let tp = level_change(LevelKind::TakeProfit, m.tp, position.tp, spec)?;
    if sl == LevelChange::Keep && tp == LevelChange::Keep {
        return Err(GateError::NothingToDo(
            "the stop loss and take profit are already at those levels".to_owned(),
        ));
    }

    // A position is closed at the bid (buy) or ask (sell); levels are checked against it.
    let current = match position.side {
        Side::Buy => market.bid,
        Side::Sell => market.ask,
    };
    let check = LevelCheck {
        spec,
        side: position.side,
        entry: current,
        reference: current,
    };
    let mut est_pnl = EstPnl::default();
    for (kind, change, slot) in [
        (LevelKind::StopLoss, sl, &mut est_pnl.at_sl),
        (LevelKind::TakeProfit, tp, &mut est_pnl.at_tp),
    ] {
        if let LevelChange::Set(v) = change {
            check.check(kind, v)?;
            *slot = Some(pnl_between(
                spec,
                position.side,
                position.price_open,
                v,
                position.volume,
            )?);
        }
    }

    let mut sets = Vec::new();
    let mut removes = Vec::new();
    for (kind, change) in [(LevelKind::StopLoss, sl), (LevelKind::TakeProfit, tp)] {
        match change {
            LevelChange::Set(v) => {
                sets.push(format!("{} to {}", kind.name(), fmt_price(v, spec.digits)))
            }
            LevelChange::Remove => removes.push(kind.name().to_owned()),
            LevelChange::Keep => {}
        }
    }
    let mut action = String::new();
    if !sets.is_empty() {
        action.push_str(&format!("Set {}", sets.join(" and ")));
    }
    if !removes.is_empty() {
        let prefix = if action.is_empty() {
            "Remove "
        } else {
            " and remove "
        };
        action.push_str(prefix);
        action.push_str(&removes.join(" and "));
    }
    let description = format!(
        "{action} on your {} {} position",
        position.symbol,
        side_word(position.side)
    );

    Ok(ActionPreview {
        id: Uuid::new_v4(),
        description,
        symbols: vec![position.symbol.clone()],
        tickets: vec![position.ticket],
        total_volume: position.volume,
        est_pnl,
        is_batch: false,
        count: 1,
        warnings: vec![],
        commands: vec![Command::PositionModify(PositionModifyArgs {
            ticket: position.ticket,
            sl: sl.wire(),
            tp: tp.wire(),
        })],
    })
}

// ---- close -------------------------------------------------------------------------

fn eval_close(c: &CloseSelector, snapshot: &MarketSnapshot) -> Result<ActionPreview, GateError> {
    let mode = targets(
        &c.tickets,
        c.symbol.is_some() || c.side.is_some(),
        c.all,
        "Which positions should I close? Tell me a symbol, a direction (buys or sells), \
         specific tickets, or say \"all\".",
    )?;

    let mut matched: Vec<&Position> = match &mode {
        Targets::Tickets(tickets) => {
            let missing: Vec<u64> = tickets
                .iter()
                .copied()
                .filter(|t| !snapshot.positions.iter().any(|p| p.ticket == *t))
                .collect();
            if !missing.is_empty() {
                return Err(missing_tickets_error(&missing, "open position"));
            }
            snapshot
                .positions
                .iter()
                .filter(|p| tickets.contains(&p.ticket))
                .collect()
        }
        Targets::Filter => snapshot
            .positions
            .iter()
            .filter(|p| c.symbol.as_ref().is_none_or(|s| s == &p.symbol))
            .filter(|p| c.side.is_none_or(|s| s == p.side))
            .collect(),
    };
    matched.sort_by_key(|p| p.ticket);
    matched.dedup_by_key(|p| p.ticket);
    if matched.is_empty() {
        return Err(GateError::NothingToDo(nothing_to_close_text(c)));
    }

    // Partial close: exactly one ticket, volume rounded down to the lot step.
    let mut warnings = Vec::new();
    let partial_volume = match c.volume {
        None => None,
        Some(requested) => {
            let single = matches!(&mode, Targets::Tickets(t) if t.len() == 1);
            if !single {
                return Err(invalid(
                    "a partial close needs exactly one position ticket; give the ticket and the volume",
                ));
            }
            let position = matched[0];
            let spec = spec_of(snapshot, &position.symbol)?;
            if !requested.is_finite() || requested <= 0.0 {
                return Err(invalid(format!("close volume {requested} is not valid")));
            }
            if !(spec.volume_step.is_finite() && spec.volume_step > 0.0) {
                return Err(invalid(format!("{} has no valid volume step", spec.symbol)));
            }
            let volume = round_volume_down(requested, spec.volume_step);
            if volume < spec.volume_min - EPS {
                return Err(invalid(format!(
                    "close volume {} is below the minimum of {} lots for {}",
                    fmt_volume(requested),
                    fmt_volume(spec.volume_min),
                    spec.symbol
                )));
            }
            if volume >= position.volume - EPS {
                return Err(invalid(format!(
                    "close volume {} is not smaller than the position ({} lots); close the whole position instead",
                    fmt_volume(volume),
                    fmt_volume(position.volume)
                )));
            }
            if volume < requested - EPS {
                warnings.push(format!(
                    "Volume rounded down from {} to {} to fit the lot step",
                    fmt_volume(requested),
                    fmt_volume(volume)
                ));
            }
            Some(volume)
        }
    };

    let count = matched.len();
    let closing_volume = match partial_volume {
        Some(v) => v,
        None => round_to(matched.iter().map(|p| p.volume).sum::<f64>(), 8),
    };
    let floating = match partial_volume {
        Some(v) => matched[0].profit * v / matched[0].volume,
        None => matched.iter().map(|p| p.profit).sum(),
    };
    let floating = round_to(floating, 2);
    let symbols: Vec<String> = matched
        .iter()
        .map(|p| p.symbol.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let currency = &snapshot.account.currency;
    let pnl_text = format!("floating P/L {} {currency}", fmt_money(floating));

    let description = if count == 1 {
        let p = matched[0];
        match partial_volume {
            Some(v) => format!(
                "Close {} of {} {} {}, {pnl_text}",
                fmt_lots(v),
                side_word(p.side),
                fmt_volume(p.volume),
                p.symbol
            ),
            None => format!(
                "Close {} {} {}, {pnl_text}",
                side_word(p.side),
                fmt_volume(p.volume),
                p.symbol
            ),
        }
    } else {
        let everything = matches!(mode, Targets::Filter) && c.symbol.is_none() && c.side.is_none();
        let which = if everything {
            format!("all {count}")
        } else {
            count.to_string()
        };
        let side = c
            .side
            .map(|s| format!("{} ", side_word(s)))
            .unwrap_or_default();
        format!(
            "Close {which} {side}positions on {} ({}), {pnl_text}",
            join_symbols(&symbols),
            fmt_lots(closing_volume)
        )
    };

    let commands = matched
        .iter()
        .map(|p| match partial_volume {
            Some(volume) => Command::PositionClosePartial(PositionClosePartialArgs {
                ticket: p.ticket,
                volume,
                deviation_points: None,
            }),
            None => Command::PositionClose(PositionCloseArgs {
                ticket: p.ticket,
                deviation_points: None,
            }),
        })
        .collect();

    Ok(ActionPreview {
        id: Uuid::new_v4(),
        description,
        symbols,
        tickets: matched.iter().map(|p| p.ticket).collect(),
        total_volume: closing_volume,
        est_pnl: EstPnl {
            current: Some(floating),
            at_sl: None,
            at_tp: None,
        },
        is_batch: count > 1,
        count,
        warnings,
        commands,
    })
}

fn nothing_to_close_text(c: &CloseSelector) -> String {
    if c.symbol.is_none() && c.side.is_none() {
        return "there are no open positions".to_owned();
    }
    let side = c
        .side
        .map(|s| format!("{} ", side_word(s)))
        .unwrap_or_default();
    match &c.symbol {
        Some(symbol) => format!("there are no open {side}positions on {symbol}"),
        None => format!("there are no open {side}positions"),
    }
}

// ---- cancel ------------------------------------------------------------------------

fn eval_cancel(c: &CancelSelector, snapshot: &MarketSnapshot) -> Result<ActionPreview, GateError> {
    let mode = targets(
        &c.tickets,
        c.symbol.is_some(),
        c.all,
        "Which pending orders should I cancel? Tell me a symbol, specific tickets, or say \"all\".",
    )?;

    let mut matched: Vec<&Order> = match &mode {
        Targets::Tickets(tickets) => {
            let missing: Vec<u64> = tickets
                .iter()
                .copied()
                .filter(|t| !snapshot.orders.iter().any(|o| o.ticket == *t))
                .collect();
            if !missing.is_empty() {
                return Err(missing_tickets_error(&missing, "pending order"));
            }
            snapshot
                .orders
                .iter()
                .filter(|o| tickets.contains(&o.ticket))
                .collect()
        }
        Targets::Filter => snapshot
            .orders
            .iter()
            .filter(|o| c.symbol.as_ref().is_none_or(|s| s == &o.symbol))
            .collect(),
    };
    matched.sort_by_key(|o| o.ticket);
    matched.dedup_by_key(|o| o.ticket);
    if matched.is_empty() {
        return Err(GateError::NothingToDo(match &c.symbol {
            Some(symbol) => format!("there are no pending orders on {symbol}"),
            None => "there are no pending orders".to_owned(),
        }));
    }

    let count = matched.len();
    let total_volume = round_to(matched.iter().map(|o| o.volume_current).sum::<f64>(), 8);
    let symbols: Vec<String> = matched
        .iter()
        .map(|o| o.symbol.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let description = if count == 1 {
        let o = matched[0];
        let at = match snapshot.symbols.get(&o.symbol) {
            Some(spec) => fmt_price(o.price_open, spec.digits),
            None => o.price_open.to_string(),
        };
        format!(
            "Cancel {} {} {} at {at}",
            order_type_text(o.order_type),
            fmt_volume(o.volume_current),
            o.symbol
        )
    } else {
        let everything = matches!(mode, Targets::Filter) && c.symbol.is_none();
        let n = plural(count, "pending order", "pending orders");
        let n = if everything { format!("all {n}") } else { n };
        format!(
            "Cancel {n} on {} ({})",
            join_symbols(&symbols),
            fmt_lots(total_volume)
        )
    };

    Ok(ActionPreview {
        id: Uuid::new_v4(),
        description,
        symbols,
        tickets: matched.iter().map(|o| o.ticket).collect(),
        total_volume,
        est_pnl: EstPnl::default(),
        is_batch: count > 1,
        count,
        warnings: vec![],
        commands: matched
            .iter()
            .map(|o| Command::OrderCancel(OrderCancelArgs { ticket: o.ticket }))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_decimals_cases() {
        for (step, expected) in [
            (1.0, 0),
            (0.5, 1),
            (0.1, 1),
            (0.01, 2),
            (0.001, 3),
            (0.00001, 5),
        ] {
            assert_eq!(step_decimals(step), expected, "step {step}");
        }
    }

    #[test]
    fn round_volume_down_is_floating_safe() {
        // (volume, step, expected)
        let cases = [
            (0.29, 0.01, 0.29),
            (0.57, 0.01, 0.57),
            (0.58, 0.01, 0.58),
            (0.3 + 1e-15, 0.01, 0.3),
            (0.999, 0.01, 0.99),
            (1.0, 0.01, 1.0),
            (0.14, 0.05, 0.1),
            (0.15, 0.05, 0.15),
            (7.9, 1.0, 7.0),
            (0.0099, 0.01, 0.0),
            (1.2345, 0.001, 1.234),
        ];
        for (volume, step, expected) in cases {
            assert_eq!(
                round_volume_down(volume, step),
                expected,
                "{volume} / {step}"
            );
        }
    }

    #[test]
    fn fmt_volume_keeps_two_decimals_and_drops_noise() {
        for (v, expected) in [
            (0.1, "0.10"),
            (1.0, "1.00"),
            (0.05, "0.05"),
            (0.057, "0.057"),
            (0.1 + 0.2, "0.30"),
            (12.5, "12.50"),
            (0.001, "0.001"),
        ] {
            assert_eq!(fmt_volume(v), expected, "{v}");
        }
    }

    #[test]
    fn fmt_lots_uses_singular_for_one() {
        assert_eq!(fmt_lots(1.0), "1.00 lot");
        assert_eq!(fmt_lots(0.3), "0.30 lots");
    }

    #[test]
    fn fmt_money_never_prints_negative_zero() {
        assert_eq!(fmt_money(-0.001), "0.00");
        assert_eq!(fmt_money(-0.0), "0.00");
        assert_eq!(fmt_money(-42.1), "-42.10");
        assert_eq!(fmt_money(1234.567), "1234.57");
    }

    #[test]
    fn join_symbols_reads_naturally() {
        let s = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(join_symbols(&s(&[])), "");
        assert_eq!(join_symbols(&s(&["A"])), "A");
        assert_eq!(join_symbols(&s(&["A", "B"])), "A and B");
        assert_eq!(join_symbols(&s(&["A", "B", "C"])), "A, B and C");
    }

    #[test]
    fn capitalize_handles_empty_and_unicode() {
        assert_eq!(capitalize(""), "");
        assert_eq!(capitalize("buy"), "Buy");
        assert_eq!(capitalize("élan"), "Élan");
    }
}
