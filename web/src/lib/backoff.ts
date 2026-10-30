/** Reconnect delay with exponential back-off and jitter: 1 s, 2 s, 4 s ... capped at `maxMs`. */
export function reconnectDelay(
	attempt: number,
	rand: () => number = Math.random,
	baseMs = 1000,
	maxMs = 30_000
): number {
	const exp = Math.min(maxMs, baseMs * 2 ** Math.max(0, attempt));
	return Math.round(exp * (0.8 + 0.4 * rand()));
}
