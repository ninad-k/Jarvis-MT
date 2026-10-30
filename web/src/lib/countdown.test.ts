import { describe, expect, it } from 'vitest';
import { DEFAULT_WINDOW_MS, announcement, countdown, remainingMs, windowFor } from './countdown';

describe('countdown', () => {
	const exp = 1_000_000;

	it('counts down against the client clock', () => {
		const c = countdown(exp, 15_000, exp - 15_000);
		expect(c.remainingMs).toBe(15_000);
		expect(c.seconds).toBe(15);
		expect(c.fraction).toBe(1);
		expect(c.expired).toBe(false);
		expect(c.urgent).toBe(false);
	});

	it('rounds seconds up so "1" shows until the final instant', () => {
		expect(countdown(exp, 15_000, exp - 1).seconds).toBe(1);
		expect(countdown(exp, 15_000, exp - 1001).seconds).toBe(2);
	});

	it('is urgent at 5 s or less', () => {
		expect(countdown(exp, 15_000, exp - 5001).urgent).toBe(false);
		expect(countdown(exp, 15_000, exp - 5000).urgent).toBe(true);
	});

	it('expires at exactly 0 and never goes negative', () => {
		const at = countdown(exp, 15_000, exp);
		expect(at.expired).toBe(true);
		expect(at.fraction).toBe(0);
		const after = countdown(exp, 15_000, exp + 5000);
		expect(after.remainingMs).toBe(0);
		expect(after.seconds).toBe(0);
		expect(after.fraction).toBe(0);
	});

	it('clamps the fraction to 1 when the window is longer than its total', () => {
		expect(countdown(exp, 15_000, exp - 30_000).fraction).toBe(1);
	});

	it('remainingMs floors at zero', () => {
		expect(remainingMs(100, 250)).toBe(0);
		expect(remainingMs(300, 250)).toBe(50);
	});

	it('windowFor keeps the 15 s default unless the server granted more', () => {
		expect(windowFor(14_200)).toBe(DEFAULT_WINDOW_MS);
		expect(windowFor(30_000)).toBe(30_000);
	});
});

describe('announcement', () => {
	const at = (sec: number) => countdown(exp0, 15_000, exp0 - sec * 1000);
	const exp0 = 5_000_000;

	it('announces once when crossing 10 s and once at 5 s', () => {
		expect(announcement(11, at(10))).toBe('10 seconds left to confirm');
		expect(announcement(10, at(9))).toBeNull();
		expect(announcement(6, at(5))).toBe('5 seconds left to confirm');
		expect(announcement(5, at(4))).toBeNull();
	});

	it('does not announce while far from expiry', () => {
		expect(announcement(null, at(15))).toBeNull();
		expect(announcement(15, at(14))).toBeNull();
	});

	it('announces the nearest threshold if the first tick is already inside it', () => {
		expect(announcement(null, at(8))).toBe('10 seconds left to confirm');
		expect(announcement(null, at(3))).toBe('5 seconds left to confirm');
	});

	it('announces expiry once', () => {
		expect(announcement(1, at(0))).toBe('Expired. Confirm is disabled.');
		expect(announcement(0, at(0))).toBeNull();
	});
});
