/** Simulated market for mock mode: random-walk prices and a mutable book of positions/orders. */
import type { PendingOrder, Position, PriceTick } from './api';

export interface Sym {
	symbol: string;
	digits: number;
	mid: number;
	/** Per-poll volatility as a fraction of price. */
	vol: number;
	spread: number;
	point: number;
	/** P/L per 1.0 price unit per 1.0 lot (USD). USDJPY is converted at the live rate. */
	contract: number;
}

export const SYMS: Sym[] = [
	{ symbol: 'XAUUSD', digits: 2, mid: 2391.26, vol: 0.00018, spread: 28, point: 0.01, contract: 100 },
	{ symbol: 'EURUSD', digits: 5, mid: 1.08432, vol: 0.00006, spread: 9, point: 0.00001, contract: 100000 },
	{ symbol: 'GBPUSD', digits: 5, mid: 1.27118, vol: 0.00007, spread: 12, point: 0.00001, contract: 100000 },
	{ symbol: 'USDJPY', digits: 3, mid: 156.234, vol: 0.00007, spread: 14, point: 0.001, contract: 100000 },
	{ symbol: 'US100', digits: 2, mid: 19842.5, vol: 0.00016, spread: 180, point: 0.01, contract: 1 },
	{ symbol: 'BTCUSD', digits: 2, mid: 67210.4, vol: 0.00035, spread: 1500, point: 0.01, contract: 1 }
];
export const BAD_SYMBOL = 'US30';

const state = new Map(SYMS.map((s) => [s.symbol, s.mid]));

export const symInfo = (symbol: string) => SYMS.find((s) => s.symbol === symbol);
export const mid = (symbol: string) => state.get(symbol) ?? 0;

export function round(v: number, d: number): number {
	const m = 10 ** d;
	return Math.round(v * m) / m;
}

function gauss(): number {
	return (Math.random() + Math.random() + Math.random() - 1.5) * 1.6;
}

export function tick(): PriceTick[] {
	return SYMS.map((s) => {
		const m = (state.get(s.symbol) ?? s.mid) * (1 + gauss() * s.vol);
		state.set(s.symbol, m);
		const half = (s.spread * s.point) / 2;
		const bid = round(m - half, s.digits);
		const ask = round(m + half, s.digits);
		return {
			symbol: s.symbol,
			bid,
			ask,
			last: 0,
			spread_points: Math.round((ask - bid) / s.point),
			digits: s.digits,
			time: Math.floor(Date.now() / 1000)
		};
	});
}

/** Aliases the mock chat understands. */
export function symbolFromText(text: string): string | null {
	if (/gold|xau/.test(text)) return 'XAUUSD';
	if (/eur/.test(text)) return 'EURUSD';
	if (/gbp|cable|pound/.test(text)) return 'GBPUSD';
	if (/jpy|yen/.test(text)) return 'USDJPY';
	if (/btc|bitcoin/.test(text)) return 'BTCUSD';
	if (/nas/.test(text)) return 'US100';
	return null;
}

// ---- Book ----------------------------------------------------------------

export interface BookPosition {
	ticket: number;
	symbol: string;
	side: 'buy' | 'sell';
	volume: number;
	price_open: number;
	sl: number | null;
	tp: number | null;
	swap: number;
	time: number;
}

const ago = (s: number) => Math.floor(Date.now() / 1000) - s;

export const book: BookPosition[] = [];
export const pending: PendingOrder[] = [];
let nextTicket = 884_400_001;

export function seedBook(): void {
	book.length = 0;
	pending.length = 0;
	book.push(
		{ ticket: 884201337, symbol: 'XAUUSD', side: 'buy', volume: 0.1, price_open: 2390.1, sl: null, tp: 2420, swap: 0, time: ago(5400) },
		{ ticket: 884201401, symbol: 'XAUUSD', side: 'buy', volume: 0.2, price_open: 2388.4, sl: 2370, tp: null, swap: 0, time: ago(7200) },
		{ ticket: 884201552, symbol: 'EURUSD', side: 'sell', volume: 0.5, price_open: 1.0861, sl: 1.09, tp: 1.08, swap: -1.2, time: ago(18000) },
		{ ticket: 884202019, symbol: 'USDJPY', side: 'buy', volume: 0.2, price_open: 156.41, sl: 155.8, tp: null, swap: 0.4, time: ago(900) },
		// Closing this one is rejected by the mock market ("Market closed"), to show a partial failure.
		{ ticket: 884202230, symbol: 'US100', side: 'sell', volume: 0.05, price_open: 19860, sl: null, tp: null, swap: 0, time: ago(2400) }
	);
	pending.push(
		{ ticket: 884300101, symbol: 'EURUSD', type: 'buy_limit', volume_initial: 0.1, volume_current: 0.1, price_open: 1.08, sl: null, tp: 1.09, time_setup: ago(3000), comment: '' },
		{ ticket: 884300102, symbol: 'XAUUSD', type: 'sell_stop', volume_initial: 0.1, volume_current: 0.1, price_open: 2380, sl: 2395, tp: null, time_setup: ago(1500), comment: '' }
	);
}
seedBook();

export const newTicket = () => nextTicket++;

export function floatingProfit(p: BookPosition): number {
	const cur = mid(p.symbol);
	const s = symInfo(p.symbol);
	if (!s) return 0;
	const dir = p.side === 'buy' ? 1 : -1;
	const raw = (cur - p.price_open) * dir * p.volume * s.contract;
	return round(p.symbol === 'USDJPY' ? raw / cur : raw, 2);
}

export function positions(): Position[] {
	return book.map((p) => {
		const s = symInfo(p.symbol);
		return {
			ticket: p.ticket,
			symbol: p.symbol,
			side: p.side,
			volume: p.volume,
			price_open: p.price_open,
			price_current: round(mid(p.symbol), s?.digits ?? 5),
			sl: p.sl,
			tp: p.tp,
			profit: floatingProfit(p),
			swap: p.swap,
			magic: 0,
			comment: '',
			time: p.time
		};
	});
}

/** Fill price for a market order or close. */
export function fillPrice(symbol: string, side: 'buy' | 'sell'): number {
	const s = symInfo(symbol);
	if (!s) return 0;
	const half = (s.spread * s.point) / 2;
	return round(mid(symbol) + (side === 'buy' ? half : -half), s.digits);
}
