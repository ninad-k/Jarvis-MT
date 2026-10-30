import {
	api,
	type Account,
	type Position,
	type PriceTick,
	type Status,
	type WatchlistResponse
} from '#lib/api.ts';
import { Poller } from './poller.svelte';

const POLL_MS = 2000;
const STATUS_MS = 3000;
const MAX_BACKOFF_MS = 30_000;

export interface Move {
	dir: 'up' | 'down';
	/** Increments on every change so the flash animation restarts. */
	n: number;
}

/** Per-symbol last move, derived from successive watchlist payloads. */
class Moves {
	map = $state<Record<string, Move>>({});

	update(next: WatchlistResponse, prev: WatchlistResponse | null): void {
		if (!prev) return;
		const before = new Map<string, PriceTick>(prev.prices.map((p) => [p.symbol, p]));
		let changed: Record<string, Move> | null = null;
		for (const p of next.prices) {
			const o = before.get(p.symbol);
			if (!o) continue;
			const delta = p.bid !== o.bid ? p.bid - o.bid : p.ask - o.ask;
			if (delta === 0) continue;
			changed ??= { ...this.map };
			changed[p.symbol] = { dir: delta > 0 ? 'up' : 'down', n: (this.map[p.symbol]?.n ?? 0) + 1 };
		}
		if (changed) this.map = changed;
	}
}

export const moves = new Moves();

export const status = new Poller<Status>((s) => api.status(s), {
	interval: STATUS_MS,
	maxInterval: MAX_BACKOFF_MS
});
export const account = new Poller<Account>((s) => api.account(s), {
	interval: POLL_MS,
	maxInterval: MAX_BACKOFF_MS
});
export const positions = new Poller<Position[]>(async (s) => (await api.positions(s)).positions, {
	interval: POLL_MS,
	maxInterval: MAX_BACKOFF_MS
});
export const watchlist = new Poller<WatchlistResponse>((s) => api.watchlist(s), {
	interval: POLL_MS,
	maxInterval: MAX_BACKOFF_MS,
	onData: (next, prev) => moves.update(next, prev)
});

/** Digits per symbol from the watchlist; positions don't carry digits in the API. */
export function digitsFor(symbol: string): number | undefined {
	return watchlist.data?.prices.find((p) => p.symbol === symbol)?.digits;
}

class Backend {
	/** The Jarvis server itself is unreachable. */
	get offline(): boolean {
		return status.error?.isOffline === true;
	}
	/** Server is up but the MT5 bridge is not connected. */
	get bridgeDown(): boolean {
		return !this.offline && status.data !== null && !status.data.bridge.connected;
	}
	/** Anything that isn't explicitly "demo" is treated as live money. */
	get isLive(): boolean {
		return status.data !== null && status.data.mode !== 'demo';
	}
}

export const backend = new Backend();

export function startPolling(): () => void {
	const all = [status, account, positions, watchlist];
	for (const p of all) p.start();
	return () => {
		for (const p of all) p.stop();
	};
}
