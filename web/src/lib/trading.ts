import type { Position } from './api';

/** Smallest lot step assumed when offering "Close half"; the server rounds to the real step. */
const LOT_STEP = 0.01;

/**
 * Half of `volume`, rounded down to the lot step, or null when closing half is not possible
 * (the half would be below one step, or equal to the whole position).
 */
export function halfVolume(volume: number): number | null {
	const steps = 1 / LOT_STEP;
	const half = Math.round(Math.floor((volume / 2) * steps + 1e-9)) / steps;
	return half >= LOT_STEP - 1e-9 && half < volume ? half : null;
}

export interface SymbolGroup {
	symbol: string;
	count: number;
	volume: number;
}

/** Open positions grouped by symbol, in order of first appearance. */
export function groupBySymbol(positions: readonly Position[]): SymbolGroup[] {
	const map = new Map<string, SymbolGroup>();
	for (const p of positions) {
		const g = map.get(p.symbol) ?? { symbol: p.symbol, count: 0, volume: 0 };
		g.count++;
		g.volume = Math.round((g.volume + p.volume) * 1000) / 1000;
		map.set(p.symbol, g);
	}
	return [...map.values()];
}

export function countSide(positions: readonly Position[], side: 'buy' | 'sell'): number {
	return positions.filter((p) => p.side === side).length;
}

/** Why write controls are disabled right now, or null when trading actions are allowed. */
export function writeBlockReason(s: {
	offline: boolean;
	bridgeDown: boolean;
	killSwitchEngaged: boolean;
}): string | null {
	if (s.killSwitchEngaged) return 'Kill switch engaged';
	if (s.offline) return 'Backend offline';
	if (s.bridgeDown) return 'MT5 bridge disconnected';
	return null;
}
