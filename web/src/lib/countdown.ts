/** Pure countdown maths for the confirmation window. The UI feeds it the client clock. */

/** Length of a normal confirmation window (SPEC rule 2). */
export const DEFAULT_WINDOW_MS = 15_000;

export interface Countdown {
	remainingMs: number;
	/** Whole seconds left, rounded up: "1" until the very last instant. */
	seconds: number;
	/** 1 at the start of the window, 0 when expired. */
	fraction: number;
	expired: boolean;
	/** 5 seconds or less. */
	urgent: boolean;
}

export function remainingMs(expiresAtMs: number, now: number): number {
	return Math.max(0, expiresAtMs - now);
}

/** Total length to draw the ring against: the fresh window, or longer if the server gave more. */
export function windowFor(remaining: number): number {
	return remaining > DEFAULT_WINDOW_MS ? remaining : DEFAULT_WINDOW_MS;
}

export function countdown(expiresAtMs: number, totalMs: number, now: number): Countdown {
	const remaining = remainingMs(expiresAtMs, now);
	const total = Math.max(1, totalMs);
	return {
		remainingMs: remaining,
		seconds: Math.ceil(remaining / 1000),
		fraction: Math.min(1, remaining / total),
		expired: remaining <= 0,
		urgent: remaining > 0 && remaining <= 5000
	};
}

/**
 * Text for the screen-reader live region, or null. Fires once when the countdown crosses
 * 10 s, once at 5 s, and once on expiry. `prevSeconds` is null on the first tick.
 */
export function announcement(prevSeconds: number | null, c: Countdown): string | null {
	if (c.expired) return prevSeconds === 0 ? null : 'Expired. Confirm is disabled.';
	const prev = prevSeconds ?? Infinity;
	if (prev > 5 && c.seconds <= 5) return '5 seconds left to confirm';
	if (prev > 10 && c.seconds <= 10) return '10 seconds left to confirm';
	return null;
}
