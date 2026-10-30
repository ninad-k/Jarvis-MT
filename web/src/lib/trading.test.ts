import { describe, expect, it } from 'vitest';
import type { Position } from './api';
import { countSide, groupBySymbol, halfVolume, writeBlockReason } from './trading';
import { reconnectDelay } from './backoff';

const pos = (ticket: number, symbol: string, side: 'buy' | 'sell', volume: number): Position => ({
	ticket,
	symbol,
	side,
	volume,
	price_open: 1,
	price_current: 1,
	sl: null,
	tp: null,
	profit: 0,
	swap: 0,
	magic: 0,
	comment: '',
	time: 0
});

describe('halfVolume', () => {
	it('halves to the lot step', () => {
		expect(halfVolume(0.1)).toBe(0.05);
		expect(halfVolume(0.5)).toBe(0.25);
		expect(halfVolume(1)).toBe(0.5);
	});

	it('rounds down when the half is not on a step', () => {
		expect(halfVolume(0.03)).toBe(0.01);
		expect(halfVolume(0.07)).toBe(0.03);
	});

	it('is unavailable when half would be below one step', () => {
		expect(halfVolume(0.01)).toBeNull();
		expect(halfVolume(0)).toBeNull();
	});
});

describe('position groups', () => {
	const list = [
		pos(1, 'XAUUSD', 'buy', 0.1),
		pos(2, 'EURUSD', 'sell', 0.5),
		pos(3, 'XAUUSD', 'buy', 0.2)
	];

	it('groups by symbol with counts and volume', () => {
		expect(groupBySymbol(list)).toEqual([
			{ symbol: 'XAUUSD', count: 2, volume: 0.3 },
			{ symbol: 'EURUSD', count: 1, volume: 0.5 }
		]);
	});

	it('counts sides', () => {
		expect(countSide(list, 'buy')).toBe(2);
		expect(countSide(list, 'sell')).toBe(1);
	});
});

describe('writeBlockReason', () => {
	const ok = { offline: false, bridgeDown: false, killSwitchEngaged: false };
	it('is null when trading is allowed', () => expect(writeBlockReason(ok)).toBeNull());
	it('puts the kill switch first', () => {
		expect(writeBlockReason({ ...ok, killSwitchEngaged: true, offline: true })).toBe('Kill switch engaged');
	});
	it('blocks when offline or the bridge is down', () => {
		expect(writeBlockReason({ ...ok, offline: true })).toBe('Backend offline');
		expect(writeBlockReason({ ...ok, bridgeDown: true })).toBe('MT5 bridge disconnected');
	});
});

describe('reconnectDelay', () => {
	const mid = () => 0.5; // jitter factor exactly 1
	it('doubles up to the cap', () => {
		expect([0, 1, 2, 3].map((n) => reconnectDelay(n, mid))).toEqual([1000, 2000, 4000, 8000]);
		expect(reconnectDelay(10, mid)).toBe(30_000);
	});
	it('jitters within +-20%', () => {
		expect(reconnectDelay(0, () => 0)).toBe(800);
		expect(reconnectDelay(0, () => 1)).toBe(1200);
	});
});
