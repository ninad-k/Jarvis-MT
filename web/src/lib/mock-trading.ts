/**
 * Mock of the action gate: previews, one-time tokens with a countdown, the batch double
 * confirmation, expiry, the kill switch and the WebSocket event stream.
 */
import {
	ApiError,
	type ConfirmResponse,
	type KillSwitchState,
	type PreviewDto,
	type Side,
	type TradeResult,
	type WsEvent
} from './api';
import {
	book,
	fillPrice,
	floatingProfit,
	newTicket,
	pending,
	round,
	symInfo,
	type BookPosition
} from './mock-market';

// ---- Events --------------------------------------------------------------

type Listener = (e: WsEvent) => void;
const listeners = new Set<Listener>();

export function onMockEvent(fn: Listener): () => void {
	listeners.add(fn);
	return () => listeners.delete(fn);
}
function emit(e: WsEvent): void {
	for (const fn of listeners) fn(e);
}

// ---- Settings ------------------------------------------------------------

export const config = { ttlMs: 15_000, capMs: 30_000, outcomeUnknown: false };

// ---- Kill switch ---------------------------------------------------------

let kill: KillSwitchState = { engaged: false, reason: null, changed_at_ms: null };

export const killState = (): KillSwitchState => ({ ...kill });

export function setKill(engaged: boolean, reason: string | null): KillSwitchState {
	kill = { engaged, reason: engaged ? (reason ?? 'manual') : null, changed_at_ms: Date.now() };
	if (engaged) {
		// Engaging revokes every live token immediately.
		for (const t of tokens.values()) if (t.state === 'live') revoke(t, 'cancelled');
	}
	emit({ type: 'kill_switch', engaged: kill.engaged, reason: kill.reason });
	return killState();
}

export function assertWritable(): void {
	if (kill.engaged) throw new ApiError('kill_switch', 'kill switch engaged', 423);
}

// ---- Tokens --------------------------------------------------------------

type Action =
	| { kind: 'close'; items: { ticket: number; volume: number | null }[] }
	| { kind: 'cancel_orders'; tickets: number[] }
	| { kind: 'open'; symbol: string; side: Side; volume: number; sl: number | null; tp: number | null };

interface Tok {
	token: string;
	preview: PreviewDto;
	action: Action;
	state: 'live' | 'used' | 'cancelled' | 'expired';
	expires: number;
	sibling: string | null;
	timer: ReturnType<typeof setTimeout> | undefined;
}

const tokens = new Map<string, Tok>();

function hex(bytes: number): string {
	return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (b) =>
		b.toString(16).padStart(2, '0')
	).join('');
}

function arm(t: Tok): void {
	clearTimeout(t.timer);
	t.timer = setTimeout(
		() => {
			if (t.state !== 'live') return;
			t.state = 'expired';
			emit({ type: 'preview_expired', token: t.token });
		},
		Math.max(0, t.expires - Date.now()) + 20
	);
}

function revoke(t: Tok, as: 'used' | 'cancelled'): void {
	clearTimeout(t.timer);
	t.state = as;
	if (as === 'cancelled') emit({ type: 'preview_expired', token: t.token });
}

/** True while some preview could still be confirmed (the mock transcriber uses this). */
export function hasLivePreview(): boolean {
	const now = Date.now();
	return [...tokens.values()].some((t) => t.state === 'live' && t.expires > now);
}

function issue(
	base: Omit<PreviewDto, 'token' | 'expires_at_ms' | 'stage' | 'timing'>,
	action: Action,
	voice: boolean
): PreviewDto {
	const token = hex(32);
	const afterReadback = voice;
	const expires = Date.now() + (afterReadback ? config.capMs : config.ttlMs);
	const preview: PreviewDto = {
		...base,
		token,
		stage: 'first',
		expires_at_ms: expires,
		timing: afterReadback ? 'after_readback' : 'immediate'
	};
	const t: Tok = { token, preview, action, state: 'live', expires, sibling: null, timer: undefined };
	tokens.set(token, t);
	arm(t);
	return { ...preview };
}

export function readback(token: string): { expires_at_ms: number } {
	const t = tokens.get(token);
	if (!t) throw new ApiError('token_unknown', 'unknown confirmation token', 404);
	if (t.state !== 'live') throw new ApiError('token_used', 'token already used or cancelled', 409);
	if (t.expires <= Date.now()) throw new ApiError('token_expired', 'confirmation expired', 410);
	// 15 s from now, never beyond the 30 s cap from issue.
	t.expires = Math.min(Date.now() + config.ttlMs, t.expires);
	arm(t);
	return { expires_at_ms: t.expires };
}

export function cancelToken(token: string): void {
	const t = tokens.get(token);
	if (!t) throw new ApiError('token_unknown', 'unknown confirmation token', 404);
	if (t.state === 'used') throw new ApiError('token_used', 'token already used', 409);
	revoke(t, 'cancelled');
	if (t.sibling) {
		const s = tokens.get(t.sibling);
		if (s && s.state === 'live') revoke(s, 'cancelled');
	}
}

// ---- Previews ------------------------------------------------------------

const money = (v: number) =>
	`${v < 0 ? '−' : v > 0 ? '+' : ''}${Math.abs(v).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? '' : 's'}`;

export function closePreview(
	targets: BookPosition[],
	opts: { volume?: number | null; batch: boolean; voice?: boolean }
): PreviewDto {
	if (targets.length === 0)
		throw new ApiError('gate_rejected', 'no open positions match that selection', 422);
	const partial = opts.volume != null && targets.length === 1 && opts.volume < targets[0].volume;
	const vol = partial ? (opts.volume as number) : targets.reduce((a, p) => a + p.volume, 0);
	const floating = round(
		targets.reduce((a, p) => a + floatingProfit(p) * (partial ? vol / p.volume : 1), 0),
		2
	);
	const symbols = [...new Set(targets.map((p) => p.symbol))];
	const is_batch = opts.batch && targets.length > 1;
	const description =
		targets.length === 1
			? `${partial ? `Close ${vol.toFixed(2)} of` : 'Close'} ${targets[0].side} ${targets[0].volume.toFixed(2)} ${targets[0].symbol} (#${targets[0].ticket}), floating P/L ${money(floating)} USD`
			: `Close ${plural(targets.length, 'position')} on ${symbols.join(', ')} (${vol.toFixed(2)} lots), floating P/L ${money(floating)} USD`;
	return issue(
		{
			id: crypto.randomUUID(),
			description,
			symbols,
			tickets: targets.map((p) => p.ticket),
			total_volume: round(vol, 3),
			est_pnl: { current: floating, at_sl: null, at_tp: null },
			is_batch,
			count: targets.length,
			warnings: targets.some((p) => p.symbol === 'NAS100')
				? ['NAS100: market is closed, this position may be rejected']
				: []
		},
		{ kind: 'close', items: targets.map((p) => ({ ticket: p.ticket, volume: partial ? vol : null })) },
		opts.voice ?? false
	);
}

export function cancelOrdersPreview(tickets: number[], voice = false): PreviewDto {
	const orders = pending.filter((o) => tickets.includes(o.ticket));
	if (orders.length === 0) throw new ApiError('gate_rejected', 'no pending orders match', 422);
	const symbols = [...new Set(orders.map((o) => o.symbol))];
	const vol = orders.reduce((a, o) => a + o.volume_current, 0);
	return issue(
		{
			id: crypto.randomUUID(),
			description:
				orders.length === 1
					? `Cancel pending ${orders[0].type.replace('_', ' ')} ${orders[0].volume_current.toFixed(2)} ${orders[0].symbol} @ ${orders[0].price_open} (#${orders[0].ticket})`
					: `Cancel ${plural(orders.length, 'pending order')} on ${symbols.join(', ')}`,
			symbols,
			tickets: orders.map((o) => o.ticket),
			total_volume: round(vol, 3),
			est_pnl: { current: null, at_sl: null, at_tp: null },
			is_batch: false,
			count: orders.length,
			warnings: []
		},
		{ kind: 'cancel_orders', tickets: orders.map((o) => o.ticket) },
		voice
	);
}

export function openPreview(
	o: { symbol: string; side: Side; volume: number; sl: number | null; tp: number | null },
	voice: boolean
): PreviewDto {
	const s = symInfo(o.symbol);
	if (!s) throw new ApiError('gate_rejected', `symbol ${o.symbol} is not allowed`, 422);
	if (o.volume > 1)
		throw new ApiError('gate_rejected', `volume ${o.volume} exceeds max_lot 1.0`, 422);
	const entry = fillPrice(o.symbol, o.side);
	const dir = o.side === 'buy' ? 1 : -1;
	const pl = (price: number) => {
		const raw = (price - entry) * dir * o.volume * s.contract;
		return round(o.symbol === 'USDJPY' ? raw / price : raw, 2);
	};
	const warnings: string[] = [];
	if (o.sl === null) warnings.push('No stop loss set');
	return issue(
		{
			id: crypto.randomUUID(),
			description: `${o.side === 'buy' ? 'Buy' : 'Sell'} ${o.volume.toFixed(2)} ${o.symbol} at market (~${entry.toFixed(s.digits)})${o.sl !== null ? `, SL ${o.sl}` : ''}${o.tp !== null ? `, TP ${o.tp}` : ''}`,
			symbols: [o.symbol],
			tickets: [],
			total_volume: o.volume,
			est_pnl: {
				current: 0,
				at_sl: o.sl !== null ? pl(o.sl) : null,
				at_tp: o.tp !== null ? pl(o.tp) : null
			},
			is_batch: false,
			count: 1,
			warnings
		},
		{ kind: 'open', ...o },
		voice
	);
}

// ---- Confirmation --------------------------------------------------------

const OK = { retcode: 10009, retcode_description: 'Request executed' };

function execute(a: Action): TradeResult[] {
	if (a.kind === 'open') {
		const ticket = newTicket();
		const price = fillPrice(a.symbol, a.side);
		book.push({
			ticket,
			symbol: a.symbol,
			side: a.side,
			volume: a.volume,
			price_open: price,
			sl: a.sl,
			tp: a.tp,
			swap: 0,
			time: Math.floor(Date.now() / 1000)
		});
		return [{ ticket, ok: true, ...OK, order: ticket + 1, deal: ticket + 2, price, volume: a.volume, error: null }];
	}
	if (a.kind === 'cancel_orders') {
		return a.tickets.map((ticket) => {
			const i = pending.findIndex((o) => o.ticket === ticket);
			if (i >= 0) pending.splice(i, 1);
			return { ticket, ok: i >= 0, ...(i >= 0 ? OK : { retcode: 10013, retcode_description: 'Invalid request' }), order: ticket, deal: null, price: null, volume: null, error: i >= 0 ? null : 'order not found' };
		});
	}
	return a.items.map(({ ticket, volume }) => {
		const i = book.findIndex((p) => p.ticket === ticket);
		if (i < 0)
			return { ticket, ok: false, retcode: 10013, retcode_description: 'Invalid request', error: 'position not found' };
		const p = book[i];
		if (p.symbol === 'NAS100')
			return { ticket, ok: false, retcode: 10018, retcode_description: 'Market is closed', error: 'Market is closed' };
		const closeVol = volume ?? p.volume;
		const price = fillPrice(p.symbol, p.side === 'buy' ? 'sell' : 'buy');
		if (closeVol < p.volume) p.volume = round(p.volume - closeVol, 3);
		else book.splice(i, 1);
		return { ticket, ok: true, ...OK, order: ticket + 7, deal: ticket + 8, price, volume: closeVol, error: null };
	});
}

export function confirm(token: string): ConfirmResponse {
	assertWritable();
	const t = tokens.get(token);
	if (!t) throw new ApiError('token_unknown', 'unknown confirmation token', 404);
	if (t.state === 'used' || t.state === 'cancelled')
		throw new ApiError('token_used', 'token already used or cancelled', 409);
	if (t.state === 'expired' || t.expires <= Date.now()) {
		t.state = 'expired';
		throw new ApiError('token_expired', 'confirmation expired', 410);
	}

	if (t.preview.is_batch && t.preview.stage === 'first') {
		revoke(t, 'used');
		const second = hex(32);
		const expires = Date.now() + config.ttlMs;
		const next: Tok = {
			...t,
			token: second,
			preview: { ...t.preview, token: second, stage: 'second', expires_at_ms: expires },
			state: 'live',
			expires,
			sibling: t.token,
			timer: undefined
		};
		t.sibling = second;
		tokens.set(second, next);
		arm(next);
		return {
			status: 'needs_second_confirmation',
			preview_id: t.preview.id,
			token: second,
			stage: 'second',
			expires_at_ms: expires,
			count: t.preview.count,
			message: `Confirm again to close ${plural(t.preview.count, 'position')}`
		};
	}

	if (config.outcomeUnknown) {
		revoke(t, 'used');
		throw new ApiError('outcome_unknown', 'bridge timed out after the request was sent', 504);
	}
	revoke(t, 'used');
	if (t.sibling) {
		const s = tokens.get(t.sibling);
		if (s && s.state === 'live') revoke(s, 'used');
	}
	const results = execute(t.action);
	emit({ type: 'trade_executed', preview_id: t.preview.id, results });
	emit({ type: 'positions_changed' });
	return { status: 'executed', preview_id: t.preview.id, results };
}

