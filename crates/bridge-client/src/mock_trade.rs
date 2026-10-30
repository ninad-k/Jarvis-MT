//! Write-command engine of the mock EA.
//!
//! Mirrors `mql5/Include/Jarvis/TradeHandlers.mqh`: the same checks in the same order with the
//! same error text, the same safety gate (`-3`), the same 64-entry reply cache keyed by request
//! `id`, and the same reply shapes. Unlike the EA it executes against [`MockState`] instead of
//! a trade server, so a successful write really adds, changes or removes positions and orders.
use std::collections::VecDeque;

use crate::mock::{MockState, err_reply, unix_now};
use crate::protocol::{
    Command, Deal, DealEntry, DealType, ExecType, Order, OrderCancelArgs, OrderSendArgs, OrderType,
    Position, PositionCloseArgs, PositionClosePartialArgs, PositionModifyArgs,
    RETCODE_INVALID_REQUEST, RETCODE_WRITE_REFUSED, Reply, Side, SymbolInfo, TRADE_RETCODE_DONE,
    TRADE_RETCODE_DONE_PARTIAL, TRADE_RETCODE_PLACED, TradeMode, TradeResult,
};

/// Replies remembered for idempotency (same as the EA).
const CACHE_SIZE: usize = 64;
const VOLUME_TOLERANCE: f64 = 1e-8;
const MAX_COMMENT_CHARS: usize = 31;
const MAX_DEVIATION: u32 = 10_000;

/// A rejected request: the envelope retcode and the message.
type Rejection = (i64, String);
type Checked<T> = Result<T, Rejection>;

fn invalid<T>(message: impl Into<String>) -> Checked<T> {
    Err((RETCODE_INVALID_REQUEST, message.into()))
}

/// Ticket counters, the reply cache and execution statistics.
#[derive(Debug)]
pub(crate) struct Engine {
    next_order: u64,
    next_deal: u64,
    cache: VecDeque<(String, Reply)>,
    executed: usize,
    hits: usize,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            next_order: 4_000_001,
            next_deal: 6_000_001,
            cache: VecDeque::new(),
            executed: 0,
            hits: 0,
        }
    }
}

impl Engine {
    pub(crate) fn executed(&self) -> usize {
        self.executed
    }

    pub(crate) fn cache_hits(&self) -> usize {
        self.hits
    }

    /// Answer one write command. A repeated request `id` returns the cached reply verbatim and
    /// does not touch the state.
    pub(crate) fn write(&mut self, state: &mut MockState, id: &str, command: Command) -> Reply {
        if let Some((_, cached)) = self.cache.iter().find(|(key, _)| key == id) {
            self.hits += 1;
            return cached.clone();
        }
        let reply = match self.execute(state, command) {
            Ok(result) => trade_reply(id, &result),
            Err((retcode, message)) => err_reply(id, retcode, &message),
        };
        if self.cache.len() == CACHE_SIZE {
            self.cache.pop_front();
        }
        self.cache.push_back((id.to_owned(), reply.clone()));
        reply
    }

    fn execute(&mut self, state: &mut MockState, command: Command) -> Checked<TradeResult> {
        match command {
            Command::OrderSend(args) => self.order_send(state, args),
            Command::PositionModify(args) => self.position_modify(state, &args),
            Command::PositionClose(args) => self.position_close(state, &args),
            Command::PositionClosePartial(args) => self.position_close_partial(state, &args),
            Command::OrderCancel(args) => self.order_cancel(state, &args),
            _ => Err((
                crate::protocol::RETCODE_UNKNOWN_COMMAND,
                "not a write command".to_owned(),
            )),
        }
    }

    // ---- order_send ------------------------------------------------------------------

    fn order_send(&mut self, state: &mut MockState, args: OrderSendArgs) -> Checked<TradeResult> {
        // Structurally invalid requests are rejected before the safety gate.
        check_deviation(args.deviation_points)?;
        if args.magic < 0 {
            return invalid("argument 'magic' must not be negative");
        }
        if args.comment.chars().count() > MAX_COMMENT_CHARS {
            return invalid(format!(
                "comment is longer than {MAX_COMMENT_CHARS} characters"
            ));
        }
        let market = args.exec == ExecType::Market;
        if market && args.price.is_some() {
            return invalid("market orders must not carry a price");
        }
        if !market && args.price.is_none() {
            return invalid("limit and stop orders need a price");
        }
        if args.sl.is_some_and(|v| v < 0.0) {
            return invalid("sl must not be negative");
        }
        if args.tp.is_some_and(|v| v < 0.0) {
            return invalid("tp must not be negative");
        }
        gate(state)?;

        let spec = symbol_spec(state, &args.symbol)?;
        let (bid, ask) = quotes(state, &args.symbol)?;
        if !spec.trade_allowed {
            return invalid(format!(
                "trading is not allowed for symbol (disabled or close-only): {}",
                args.symbol
            ));
        }
        check_volume(&spec, args.volume)?;
        let digits = spec.digits;
        if let Some(price) = args.price {
            check_price_format(&spec, "price", price)?;
        }
        let sl = args.sl.unwrap_or(0.0);
        let tp = args.tp.unwrap_or(0.0);
        if sl > 0.0 {
            check_price_format(&spec, "sl", sl)?;
        }
        if tp > 0.0 {
            check_price_format(&spec, "tp", tp)?;
        }

        let is_buy = args.side == Side::Buy;
        let (level_ref, level_name) = match args.exec {
            ExecType::Market => {
                if is_buy {
                    (bid, "bid")
                } else {
                    (ask, "ask")
                }
            }
            ExecType::Limit | ExecType::Stop => {
                let price = args.price.unwrap_or(0.0);
                let below = (is_buy && args.exec == ExecType::Limit)
                    || (!is_buy && args.exec == ExecType::Stop);
                let (reference, ref_name) = if is_buy { (ask, "ask") } else { (bid, "bid") };
                let dist = min_distance(&spec);
                let eps = spec.point * 0.001;
                let points = dist_points(&spec);
                let side = side_word(args.side);
                let kind = if args.exec == ExecType::Limit {
                    "limit"
                } else {
                    "stop"
                };
                if below && price > reference - dist + eps {
                    return invalid(format!(
                        "{side} {kind} price {} must be below the {ref_name} {} by at least {points} points (stops level)",
                        fmt_price(price, digits),
                        fmt_price(reference, digits)
                    ));
                }
                if !below && price < reference + dist - eps {
                    return invalid(format!(
                        "{side} {kind} price {} must be above the {ref_name} {} by at least {points} points (stops level)",
                        fmt_price(price, digits),
                        fmt_price(reference, digits)
                    ));
                }
                (price, "order price")
            }
        };
        check_levels(&spec, is_buy, level_ref, level_name, sl, tp)?;

        self.begin_execution(state)?;
        let now = unix_now();
        let order_id = self.take_order();
        match args.exec {
            ExecType::Market => {
                let fill = if is_buy { ask } else { bid };
                let close_price = if is_buy { bid } else { ask };
                let deal_id = self.take_deal();
                state.positions.push(Position {
                    ticket: order_id,
                    symbol: args.symbol.clone(),
                    side: args.side,
                    volume: args.volume,
                    price_open: fill,
                    price_current: close_price,
                    sl: (sl > 0.0).then_some(sl),
                    tp: (tp > 0.0).then_some(tp),
                    profit: pnl(&spec, args.side, fill, close_price, args.volume),
                    swap: 0.0,
                    magic: args.magic,
                    comment: args.comment.clone(),
                    time: now,
                });
                state.deals.push(Deal {
                    ticket: deal_id,
                    order: order_id,
                    position_id: order_id,
                    symbol: args.symbol.clone(),
                    deal_type: deal_type(args.side),
                    entry: DealEntry::In,
                    volume: args.volume,
                    price: fill,
                    profit: 0.0,
                    commission: 0.0,
                    swap: 0.0,
                    time: now,
                    comment: args.comment,
                });
                refresh_account(state);
                Ok(done(
                    format!("done at {}", fmt_price(fill, digits)),
                    order_id,
                    deal_id,
                    args.volume,
                    fill,
                ))
            }
            ExecType::Limit | ExecType::Stop => {
                let price = args.price.unwrap_or(0.0);
                state.orders.push(Order {
                    ticket: order_id,
                    symbol: args.symbol,
                    order_type: pending_type(args.side, args.exec),
                    volume_initial: args.volume,
                    volume_current: args.volume,
                    price_open: price,
                    sl: (sl > 0.0).then_some(sl),
                    tp: (tp > 0.0).then_some(tp),
                    time_setup: now,
                    comment: args.comment,
                });
                Ok(done("done".to_owned(), order_id, 0, args.volume, 0.0))
            }
        }
    }

    // ---- position_modify -------------------------------------------------------------

    fn position_modify(
        &mut self,
        state: &mut MockState,
        args: &PositionModifyArgs,
    ) -> Checked<TradeResult> {
        check_ticket(args.ticket)?;
        if args.sl.is_none() && args.tp.is_none() {
            return invalid("at least one of 'sl' and 'tp' is required");
        }
        if args.sl.is_some_and(|v| v < 0.0) || args.tp.is_some_and(|v| v < 0.0) {
            return invalid("sl and tp must not be negative (use 0 to remove)");
        }
        gate(state)?;
        let index = find_position(state, args.ticket)?;
        let position = state.positions[index].clone();
        let spec = symbol_spec(state, &position.symbol)?;
        let (bid, ask) = quotes(state, &position.symbol)?;

        let eps = spec.point * 0.001;
        let current_sl = position.sl.unwrap_or(0.0);
        let current_tp = position.tp.unwrap_or(0.0);
        // Values that are not changing are not re-validated against the moving market.
        let sl_changes = args.sl.filter(|v| (v - current_sl).abs() > eps);
        let tp_changes = args.tp.filter(|v| (v - current_tp).abs() > eps);
        if let Some(sl) = sl_changes.filter(|v| *v > 0.0) {
            check_price_format(&spec, "sl", sl)?;
        }
        if let Some(tp) = tp_changes.filter(|v| *v > 0.0) {
            check_price_format(&spec, "tp", tp)?;
        }
        let is_buy = position.side == Side::Buy;
        let (reference, ref_name) = if is_buy { (bid, "bid") } else { (ask, "ask") };
        check_levels(
            &spec,
            is_buy,
            reference,
            ref_name,
            sl_changes.unwrap_or(0.0),
            tp_changes.unwrap_or(0.0),
        )?;

        self.begin_execution(state)?;
        let target = &mut state.positions[index];
        if let Some(sl) = args.sl {
            target.sl = (sl > 0.0).then_some(sl);
        }
        if let Some(tp) = args.tp {
            target.tp = (tp > 0.0).then_some(tp);
        }
        Ok(done("done".to_owned(), 0, 0, 0.0, 0.0))
    }

    // ---- position_close / position_close_partial ----------------------------------------

    fn position_close(
        &mut self,
        state: &mut MockState,
        args: &PositionCloseArgs,
    ) -> Checked<TradeResult> {
        check_ticket(args.ticket)?;
        check_deviation(args.deviation_points)?;
        gate(state)?;
        let index = find_position(state, args.ticket)?;
        let position = state.positions[index].clone();
        let spec = symbol_spec(state, &position.symbol)?;
        let (bid, ask) = quotes(state, &position.symbol)?;
        self.begin_execution(state)?;
        let close_price = if position.side == Side::Buy { bid } else { ask };
        let result = self.close_volume(
            state,
            &spec,
            &position,
            position.volume,
            close_price,
            Some(index),
        );
        Ok(result)
    }

    fn position_close_partial(
        &mut self,
        state: &mut MockState,
        args: &PositionClosePartialArgs,
    ) -> Checked<TradeResult> {
        check_ticket(args.ticket)?;
        check_deviation(args.deviation_points)?;
        gate(state)?;
        let index = find_position(state, args.ticket)?;
        let position = state.positions[index].clone();
        let spec = symbol_spec(state, &position.symbol)?;
        let (bid, ask) = quotes(state, &position.symbol)?;
        check_volume(&spec, args.volume)?;
        if args.volume >= position.volume - VOLUME_TOLERANCE {
            return invalid(format!(
                "volume {} must be less than the position volume {} (use position_close to close it fully)",
                num(args.volume),
                num(position.volume)
            ));
        }
        if position.volume - args.volume < spec.volume_min - VOLUME_TOLERANCE {
            return invalid(format!(
                "the remaining volume {} would be below the minimum {}",
                num(position.volume - args.volume),
                num(spec.volume_min)
            ));
        }
        self.begin_execution(state)?;
        let close_price = if position.side == Side::Buy { bid } else { ask };
        Ok(self.close_volume(state, &spec, &position, args.volume, close_price, None))
    }

    /// Close `volume` of `position` at `close_price`. `remove` is the index to drop when the
    /// whole position goes; a partial close keeps the position with a smaller volume.
    fn close_volume(
        &mut self,
        state: &mut MockState,
        spec: &SymbolInfo,
        position: &Position,
        volume: f64,
        close_price: f64,
        remove: Option<usize>,
    ) -> TradeResult {
        let profit = round2(position.profit * volume / position.volume);
        let order_id = self.take_order();
        let deal_id = self.take_deal();
        state.deals.push(Deal {
            ticket: deal_id,
            order: order_id,
            position_id: position.ticket,
            symbol: position.symbol.clone(),
            deal_type: deal_type(opposite(position.side)),
            entry: DealEntry::Out,
            volume,
            price: close_price,
            profit,
            commission: 0.0,
            swap: 0.0,
            time: unix_now(),
            comment: String::new(),
        });
        state.account.balance = round2(state.account.balance + profit);
        if let Some(index) = remove {
            state.positions.remove(index);
        } else if let Some(target) = state
            .positions
            .iter_mut()
            .find(|p| p.ticket == position.ticket)
        {
            target.volume = round8(target.volume - volume);
            target.profit = round2(target.profit - profit);
        }
        refresh_account(state);
        let text = format!("done at {}", fmt_price(close_price, spec.digits));
        done(text, order_id, deal_id, volume, close_price)
    }

    // ---- order_cancel ----------------------------------------------------------------

    fn order_cancel(
        &mut self,
        state: &mut MockState,
        args: &OrderCancelArgs,
    ) -> Checked<TradeResult> {
        check_ticket(args.ticket)?;
        gate(state)?;
        let Some(index) = state.orders.iter().position(|o| o.ticket == args.ticket) else {
            return invalid(format!("pending order not found: {}", args.ticket));
        };
        self.begin_execution(state)?;
        let order = state.orders.remove(index);
        Ok(done("done".to_owned(), order.ticket, 0, 0.0, 0.0))
    }

    // ---- bookkeeping -----------------------------------------------------------------

    /// The point of no return: count the write and apply `fail_next_write`.
    fn begin_execution(&mut self, state: &mut MockState) -> Checked<()> {
        self.executed += 1;
        let forced = state.fail_next_write.take();
        match forced {
            Some(retcode) if !is_success_retcode(retcode) => Err((
                retcode,
                format!("trade failed: {} ({retcode})", retcode_description(retcode)),
            )),
            _ => Ok(()),
        }
    }

    fn take_order(&mut self) -> u64 {
        let ticket = self.next_order;
        self.next_order += 1;
        ticket
    }

    fn take_deal(&mut self) -> u64 {
        let ticket = self.next_deal;
        self.next_deal += 1;
        ticket
    }
}

// ---- checks shared by the commands ---------------------------------------------------------

/// The safety gate: real accounts (unless `allow_real`), simulated refusals, Algo Trading off.
fn gate(state: &MockState) -> Checked<()> {
    let real = state.ping.trade_mode == TradeMode::Real && !state.allow_real;
    if real || state.refuse_writes {
        return Err((
            RETCODE_WRITE_REFUSED,
            "writes refused on a real account (InpAllowReal is false)".to_owned(),
        ));
    }
    if !state.ping.trade_allowed {
        return Err((
            RETCODE_WRITE_REFUSED,
            "writes refused: Algo Trading is disabled in the terminal (enable Algo Trading)"
                .to_owned(),
        ));
    }
    Ok(())
}

fn check_ticket(ticket: u64) -> Checked<()> {
    if ticket == 0 {
        return invalid("argument 'ticket' must be a positive integer");
    }
    Ok(())
}

fn check_deviation(deviation: Option<u32>) -> Checked<()> {
    if deviation.is_some_and(|d| d > MAX_DEVIATION) {
        return invalid(format!(
            "argument 'deviation_points' must be between 0 and {MAX_DEVIATION}"
        ));
    }
    Ok(())
}

fn find_position(state: &MockState, ticket: u64) -> Checked<usize> {
    match state.positions.iter().position(|p| p.ticket == ticket) {
        Some(index) => Ok(index),
        None => invalid(format!("position not found: {ticket}")),
    }
}

fn symbol_spec(state: &MockState, symbol: &str) -> Checked<SymbolInfo> {
    match state.symbols.iter().find(|s| s.symbol == symbol) {
        Some(spec) => Ok(spec.clone()),
        None => invalid(format!("unknown symbol: {symbol}")),
    }
}

/// `(bid, ask)`
fn quotes(state: &MockState, symbol: &str) -> Checked<(f64, f64)> {
    match state.prices.iter().find(|p| p.symbol == symbol) {
        Some(price) if price.bid > 0.0 && price.ask > 0.0 => Ok((price.bid, price.ask)),
        _ => invalid(format!("no quotes available for symbol: {symbol}")),
    }
}

/// Within `[min, max]` and a whole multiple of the step. Never rounded.
fn check_volume(spec: &SymbolInfo, volume: f64) -> Checked<()> {
    if !volume.is_finite() || volume <= 0.0 {
        return invalid("volume must be a positive number");
    }
    let (min, max, step) = (spec.volume_min, spec.volume_max, spec.volume_step);
    if min <= 0.0 || step <= 0.0 || max < min {
        return invalid(format!(
            "volume limits are not available for {}",
            spec.symbol
        ));
    }
    if volume < min - VOLUME_TOLERANCE {
        return invalid(format!(
            "volume {} is below the minimum {}",
            num(volume),
            num(min)
        ));
    }
    if volume > max + VOLUME_TOLERANCE {
        return invalid(format!(
            "volume {} is above the maximum {}",
            num(volume),
            num(max)
        ));
    }
    if (volume - (volume / step).round() * step).abs() > VOLUME_TOLERANCE {
        return invalid(format!(
            "volume {} is not a multiple of the volume step {}",
            num(volume),
            num(step)
        ));
    }
    Ok(())
}

/// A price must be positive and already expressed in the symbol's digits.
fn check_price_format(spec: &SymbolInfo, name: &str, price: f64) -> Checked<()> {
    if !price.is_finite() || price <= 0.0 {
        return invalid(format!("{name} must be a positive number"));
    }
    if (price - normalize(price, spec.digits)).abs() > spec.point * 0.001 {
        return invalid(format!(
            "{name} has more than {} decimals for {}",
            spec.digits, spec.symbol
        ));
    }
    Ok(())
}

/// Minimum distance from the reference price: the stops level, at least one point.
fn min_distance(spec: &SymbolInfo) -> f64 {
    spec.stops_level.max(1) as f64 * spec.point
}

fn dist_points(spec: &SymbolInfo) -> i64 {
    spec.stops_level.max(1)
}

/// sl/tp against the reference price (0 = not set). Buy: sl below, tp above. Sell: the reverse.
fn check_levels(
    spec: &SymbolInfo,
    is_buy: bool,
    reference: f64,
    ref_name: &str,
    sl: f64,
    tp: f64,
) -> Checked<()> {
    let dist = min_distance(spec);
    let eps = spec.point * 0.001;
    let digits = spec.digits;
    let need = format!("by at least {} points (stops level)", dist_points(spec));
    let r = fmt_price(reference, digits);
    if sl > 0.0 {
        if is_buy && sl > reference - dist + eps {
            return invalid(format!(
                "sl {} must be below the {ref_name} {r} {need}",
                fmt_price(sl, digits)
            ));
        }
        if !is_buy && sl < reference + dist - eps {
            return invalid(format!(
                "sl {} must be above the {ref_name} {r} {need}",
                fmt_price(sl, digits)
            ));
        }
    }
    if tp > 0.0 {
        if is_buy && tp < reference + dist - eps {
            return invalid(format!(
                "tp {} must be above the {ref_name} {r} {need}",
                fmt_price(tp, digits)
            ));
        }
        if !is_buy && tp > reference - dist + eps {
            return invalid(format!(
                "tp {} must be below the {ref_name} {r} {need}",
                fmt_price(tp, digits)
            ));
        }
    }
    Ok(())
}

// ---- small helpers ---------------------------------------------------------------------------

fn side_word(side: Side) -> &'static str {
    match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

fn opposite(side: Side) -> Side {
    match side {
        Side::Buy => Side::Sell,
        Side::Sell => Side::Buy,
    }
}

fn deal_type(side: Side) -> DealType {
    match side {
        Side::Buy => DealType::Buy,
        Side::Sell => DealType::Sell,
    }
}

fn pending_type(side: Side, exec: ExecType) -> OrderType {
    match (side, exec) {
        (Side::Buy, ExecType::Stop) => OrderType::BuyStop,
        (Side::Sell, ExecType::Stop) => OrderType::SellStop,
        (Side::Buy, _) => OrderType::BuyLimit,
        (Side::Sell, _) => OrderType::SellLimit,
    }
}

/// Floating profit of `volume` lots moving from `open` to `close`.
fn pnl(spec: &SymbolInfo, side: Side, open: f64, close: f64, volume: f64) -> f64 {
    let diff = match side {
        Side::Buy => close - open,
        Side::Sell => open - close,
    };
    if spec.tick_size <= 0.0 {
        return 0.0;
    }
    round2(diff / spec.tick_size * spec.tick_value * volume)
}

/// Keep `account.profit`/`equity` consistent with the positions after a state change.
fn refresh_account(state: &mut MockState) {
    let profit: f64 = state.positions.iter().map(|p| p.profit).sum();
    state.account.profit = round2(profit);
    state.account.equity = round2(state.account.balance + state.account.profit);
    state.account.free_margin = round2(state.account.equity - state.account.margin);
    state.account.margin_level = if state.account.margin > 0.0 {
        round2(state.account.equity / state.account.margin * 100.0)
    } else {
        0.0
    };
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn round8(v: f64) -> f64 {
    (v * 1e8).round() / 1e8
}

fn normalize(price: f64, digits: u32) -> f64 {
    let scale = 10f64.powi(i32::try_from(digits).unwrap_or(0));
    (price * scale).round() / scale
}

/// A price with the symbol's number of decimals.
fn fmt_price(price: f64, digits: u32) -> String {
    format!("{price:.*}", digits as usize)
}

/// A number with up to 8 decimals and no trailing zeros.
fn num(value: f64) -> String {
    let text = format!("{value:.8}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

fn is_success_retcode(retcode: i64) -> bool {
    matches!(
        retcode,
        TRADE_RETCODE_PLACED | TRADE_RETCODE_DONE | TRADE_RETCODE_DONE_PARTIAL
    )
}

fn done(description: String, order: u64, deal: u64, volume: f64, price: f64) -> TradeResult {
    TradeResult {
        retcode: TRADE_RETCODE_DONE,
        retcode_description: description,
        order,
        deal,
        volume,
        price,
        comment: "Request executed".to_owned(),
    }
}

/// Success envelope of a write: the envelope retcode is the trade retcode.
fn trade_reply(id: &str, result: &TradeResult) -> Reply {
    Reply {
        id: id.to_owned(),
        ok: true,
        data: serde_json::to_value(result).ok(),
        error: None,
        retcode: result.retcode,
        ts: None,
    }
}

/// Short text for the common MT5 trade server retcodes.
fn retcode_description(retcode: i64) -> &'static str {
    match retcode {
        10004 => "requote",
        10006 => "request rejected",
        10007 => "request canceled by trader",
        10011 => "request processing error",
        10012 => "request timed out",
        10013 => "invalid request",
        10014 => "invalid volume",
        10015 => "invalid price",
        10016 => "invalid stops",
        10017 => "trade disabled",
        10018 => "market closed",
        10019 => "no money",
        10020 => "prices changed",
        10021 => "no quotes to process the request",
        10022 => "invalid order expiration",
        10023 => "order state changed",
        10024 => "too many requests",
        10025 => "no changes in request",
        10026 => "autotrading disabled by server",
        10027 => "autotrading disabled by client terminal",
        10028 => "request locked for processing",
        10029 => "order or position frozen",
        10030 => "invalid order filling type",
        10031 => "no connection with the trade server",
        10032 => "operation allowed only for live accounts",
        10033 => "number of pending orders reached the limit",
        10034 => "volume of orders and positions for the symbol reached the limit",
        10035 => "incorrect or prohibited order type",
        10036 => "position with the specified ticket has already been closed",
        _ => "trade error",
    }
}
