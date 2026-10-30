import {
	api,
	type Account,
	type PendingOrder,
	type Position,
	type PriceTick,
	type Status,
	type WatchlistResponse
} from '#lib/api.ts';
import { writeBlockReason } from '#lib/trading.ts';
import { killSwitch } from './killswitch.svelte';
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
	maxInterval: MAX_BACKOFF_MS,
	onData: (next) => {
		if (next.kill_switch) killSwitch.apply(next.kill_switch);
	}
});
export const account = new Poller<Account>((s) => api.account(s), {
	interval: POLL_MS,
	maxInterval: MAX_BACKOFF_MS
});
export const positions = new Poller<Position[]>(async (s) => (await api.positions(s)).positions, {
	interval: POLL_MS,
	maxInterval: MAX_BACKOFF_MS
});
/** Pending orders. `GET /api/orders` is assumed (see api.ts); the section hides itself on a 404. */
export const orders = new Poller<PendingOrder[]>(async (s) => (await api.orders(s)).orders, {
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
	/** The Arc MT server itself is unreachable. */
	get offline(): boolean {
		return status.error?.isOffline === true;
	}
	/** Server is up but the MT5 bridge is not connected. */
	get bridgeDown(): boolean {
		return !this.offline && status.data !== null && !status.data.bridge.connected;
	}
	/** The kill switch is engaged on the server (status poll, toggle response or WebSocket). */
	get killSwitchEngaged(): boolean {
		return killSwitch.engaged;
	}
	/** Why every write control is disabled right now, or null when trading actions are allowed. */
	get writeBlockedReason(): string | null {
		return writeBlockReason({
			offline: this.offline,
			bridgeDown: this.bridgeDown,
			killSwitchEngaged: killSwitch.engaged
		});
	}
	/** Whether the server reports microphone transcription as available. */
	get sttAvailable(): boolean {
		return status.data?.voice?.stt === true;
	}
	get ttsAvailable(): boolean {
		return status.data?.voice?.tts === true;
	}
	/** Anything that isn't explicitly "demo" is treated as live money. */
	get isLive(): boolean {
		return status.data !== null && status.data.mode !== 'demo';
	}
}

export const backend = new Backend();

const all = [status, account, positions, orders, watchlist];

/** Re-read everything a trade can change. Called after an execute and on server events. */
export function refreshTrading(): void {
	positions.refresh();
	orders.refresh();
	account.refresh();
}

/** Drop cached data (sign-out), so the next user never sees the previous one's positions. */
export function resetPollers(): void {
	for (const p of all) p.reset();
}

export function startPolling(): () => void {
	for (const p of all) p.start();
	return () => {
		for (const p of all) p.stop();
	};
}
