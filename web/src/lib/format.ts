/** Number and text formatting helpers. All numbers render in en-US with fixed decimals. */

const cache = new Map<string, Intl.NumberFormat>();

function nf(min: number, max: number, signed: boolean): Intl.NumberFormat {
	const key = `${min}|${max}|${signed}`;
	let f = cache.get(key);
	if (!f) {
		f = new Intl.NumberFormat('en-US', {
			minimumFractionDigits: min,
			maximumFractionDigits: max,
			signDisplay: signed ? 'exceptZero' : 'auto'
		});
		cache.set(key, f);
	}
	return f;
}

/** Price with the symbol's digits, no grouping noise beyond thousands separators. */
export function fmtPrice(v: number, digits: number): string {
	return nf(digits, digits, false).format(v);
}

export function fmtMoney(v: number, digits = 2): string {
	return nf(digits, digits, false).format(v);
}

/** Signed amount ("+12.50", "-3.20", "0.00"). */
export function fmtSigned(v: number, digits = 2): string {
	return nf(digits, digits, true).format(v);
}

/** Lot size: at least 2 decimals, up to 3. */
export function fmtVolume(v: number): string {
	return nf(2, 3, false).format(v);
}

export function fmtPct(v: number): string {
	return `${nf(2, 2, false).format(v)}%`;
}

/** Number of decimals in the shortest decimal representation of `n`. */
export function decimalsOf(n: number): number {
	const s = String(n);
	if (s.includes('e-')) return Number(s.split('e-')[1]);
	const i = s.indexOf('.');
	return i < 0 ? 0 : s.length - i - 1;
}

export type Tone = 'up' | 'down' | 'flat';

export function tone(v: number): Tone {
	return v > 0 ? 'up' : v < 0 ? 'down' : 'flat';
}

export function ago(ms: number, now = Date.now()): string {
	const s = Math.max(0, Math.round((now - ms) / 1000));
	if (s < 60) return `${s}s ago`;
	const m = Math.floor(s / 60);
	return m < 60 ? `${m}m ago` : `${Math.floor(m / 60)}h ago`;
}

export function duration(seconds: number): string {
	const s = Math.max(0, seconds);
	if (s < 3600) return `${Math.max(1, Math.round(s / 60))}m`;
	if (s < 86400) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
	return `${Math.floor(s / 86400)}d`;
}

function short(v: unknown): string {
	const s = typeof v === 'string' ? v : JSON.stringify(v);
	return s.length > 24 ? `${s.slice(0, 23)}…` : s;
}

/** Compact one-line summary of tool arguments for chips: `XAUUSD` or `symbol=XAUUSD, limit=50`. */
export function summarizeArgs(args: Record<string, unknown> | null | undefined): string {
	const entries = Object.entries(args ?? {}).filter(([, v]) => v !== null && v !== undefined);
	if (entries.length === 0) return '';
	if (entries.length === 1) return short(entries[0][1]);
	return entries.map(([k, v]) => `${k}=${short(v)}`).join(', ');
}

/** Audit timestamps may be Unix seconds, Unix milliseconds or an ISO string. Returns epoch ms. */
export function auditMs(ts: number | string): number {
	if (typeof ts === 'string') {
		const t = Date.parse(ts);
		return Number.isNaN(t) ? 0 : t;
	}
	return ts < 1e11 ? ts * 1000 : ts;
}

/** One-line summary of an audit payload: `key=value` pairs, long values clipped. */
export function payloadSummary(payload: unknown, max = 110): string {
	if (payload === null || payload === undefined) return '';
	if (typeof payload !== 'object') return String(payload).slice(0, max);
	const parts: string[] = [];
	for (const [k, v] of Object.entries(payload as Record<string, unknown>)) {
		if (v === null || v === undefined) continue;
		const val = typeof v === 'object' ? JSON.stringify(v) : String(v);
		parts.push(`${k}=${val.length > 32 ? `${val.slice(0, 31)}…` : val}`);
	}
	const line = parts.join(' ');
	return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}
