/**
 * Validation and shaping helpers for the settings page. Pure functions, unit tested.
 * The server is the authority (docs/api.md); these only catch mistakes early and turn form
 * state into request bodies. The mock backend reuses the symbol rules so both agree.
 */
import type { Limits, LimitsOverrides, TradingHours, Weekday } from './api';

// ---- Watchlist ---------------------------------------------------------------

export const MAX_WATCHLIST = 30;
export const MAX_ALIASES = 100;
/** Broker symbols and alias targets: `[A-Za-z0-9._#]{1,32}` (docs/api.md). */
export const SYMBOL_RE = /^[A-Za-z0-9._#]{1,32}$/;

/** A watchlist entry is a broker symbol or a spoken alias ("yellow metal"), so spaces are allowed. */
export const WATCH_ENTRY_RE = /^[A-Za-z0-9._# -]{1,64}$/;

export function watchEntryError(raw: string): string | null {
	const s = raw.trim();
	if (s.length === 0) return 'Enter a symbol or alias';
	if (s.length > 64) return 'At most 64 characters';
	if (!WATCH_ENTRY_RE.test(s)) return 'Letters, digits, spaces and . _ # - only';
	return null;
}

export function symbolError(raw: string): string | null {
	const s = raw.trim();
	if (s.length === 0) return 'Enter a symbol';
	if (s.length > 32) return 'At most 32 characters';
	if (!SYMBOL_RE.test(s)) return 'Letters, digits and . _ # only, no spaces';
	return null;
}

/** Add `raw` to the list. Returns the new list, or the reason it was refused. */
export function addSymbol(list: readonly string[], raw: string): { list: string[] } | { error: string } {
	const s = raw.trim().replace(/\s+/g, ' ');
	const bad = watchEntryError(s);
	if (bad) return { error: bad };
	if (list.length >= MAX_WATCHLIST) return { error: `The watchlist holds at most ${MAX_WATCHLIST} symbols` };
	if (list.some((x) => x.toLowerCase() === s.toLowerCase())) return { error: `${s} is already in the list` };
	return { list: [...list, s] };
}

/** Whole-list check before saving. */
export function watchlistError(list: readonly string[]): string | null {
	if (list.length > MAX_WATCHLIST) return `At most ${MAX_WATCHLIST} symbols (you have ${list.length})`;
	const seen = new Set<string>();
	for (const s of list) {
		const bad = watchEntryError(s);
		if (bad) return `${s || '(empty)'}: ${bad}`;
		const k = s.toLowerCase();
		if (seen.has(k)) return `${s} appears twice`;
		seen.add(k);
	}
	return null;
}

/** Move the item at `from` by `delta` places (clamped). Returns a new array. */
export function moveItem<T>(list: readonly T[], from: number, delta: number): T[] {
	const to = Math.max(0, Math.min(list.length - 1, from + delta));
	const out = list.slice();
	if (from < 0 || from >= list.length || to === from) return out;
	const [item] = out.splice(from, 1);
	out.splice(to, 0, item);
	return out;
}

// ---- Aliases -------------------------------------------------------------------

export interface AliasRow {
	spoken: string;
	symbol: string;
}

/** Keys are lower-cased by the server; spaces are collapsed so "Yellow  Metal" matches speech. */
export function normalizeSpoken(raw: string): string {
	return raw.trim().replace(/\s+/g, ' ').toLowerCase();
}

export function aliasesToRows(map: Readonly<Record<string, string>>): AliasRow[] {
	return Object.entries(map)
		.map(([spoken, symbol]) => ({ spoken, symbol }))
		.sort((a, b) => a.spoken.localeCompare(b.spoken));
}

export function rowsToAliases(rows: readonly AliasRow[]): Record<string, string> {
	const out: Record<string, string> = {};
	for (const r of rows) {
		const k = normalizeSpoken(r.spoken);
		if (k) out[k] = r.symbol.trim();
	}
	return out;
}

export interface AliasValidation {
	/** One message (or null) per row, in order. */
	rows: (string | null)[];
	general: string | null;
	ok: boolean;
}

export function validateAliasRows(rows: readonly AliasRow[]): AliasValidation {
	const seen = new Map<string, number>();
	const out = rows.map((r, i) => {
		const k = normalizeSpoken(r.spoken);
		if (!k) return 'Enter the spoken name';
		if (k.length > 64) return 'Spoken name: at most 64 characters';
		const bad = symbolError(r.symbol);
		if (bad) return bad;
		if (seen.has(k)) return `"${k}" is already used in row ${(seen.get(k) ?? 0) + 1}`;
		seen.set(k, i);
		return null;
	});
	const general = rows.length > MAX_ALIASES ? `At most ${MAX_ALIASES} aliases (you have ${rows.length})` : null;
	return { rows: out, general, ok: general === null && out.every((m) => m === null) };
}

// ---- Trading limits (tighten-only) ----------------------------------------------

export const WEEKDAYS: readonly Weekday[] = ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'];
export const WEEKDAY_LABEL: Record<Weekday, string> = {
	mon: 'Mon',
	tue: 'Tue',
	wed: 'Wed',
	thu: 'Thu',
	fri: 'Fri',
	sat: 'Sat',
	sun: 'Sun'
};

export type LimitField = 'max_lot' | 'max_positions' | 'max_daily_loss' | 'allowed_symbols' | 'trading_hours';
export const LIMIT_FIELDS: readonly LimitField[] = [
	'max_lot',
	'max_positions',
	'max_daily_loss',
	'allowed_symbols',
	'trading_hours'
];

export type LimitErrors = Partial<Record<LimitField, string>>;

export interface LimitsForm {
	max_lot: string;
	max_positions: string;
	max_daily_loss: string;
	allowed_symbols: string[];
	/** "Narrow the trading hours" is on: the days and window below are sent. */
	hours_enabled: boolean;
	days: Weekday[];
	start: string;
	end: string;
}

const DEFAULT_START = '09:00';
const DEFAULT_END = '17:00';

/** "HH:MM" to minutes since midnight, or null. */
export function parseHHMM(v: string): number | null {
	const m = /^([01]\d|2[0-3]):([0-5]\d)$/.exec(v);
	return m ? Number(m[1]) * 60 + Number(m[2]) : null;
}

const num = (v: number) => String(Number(v.toFixed(6)));

/** Form state from the limits the gate enforces now. */
export function limitsToForm(effective: Limits, config: Limits): LimitsForm {
	const h = effective.trading_hours;
	const cfg = config.trading_hours;
	const narrowed = h !== null && (cfg === null || !sameHours(h, cfg));
	const shown = h ?? cfg;
	return {
		max_lot: num(effective.max_lot),
		max_positions: String(effective.max_positions),
		max_daily_loss: num(effective.max_daily_loss),
		allowed_symbols: [...effective.allowed_symbols],
		hours_enabled: narrowed,
		days: shown ? [...shown.days] : ['mon', 'tue', 'wed', 'thu', 'fri'],
		start: shown?.start ?? DEFAULT_START,
		end: shown?.end ?? DEFAULT_END
	};
}

export function sameHours(a: TradingHours, b: TradingHours): boolean {
	return (
		a.start === b.start &&
		a.end === b.end &&
		a.days.length === b.days.length &&
		a.days.every((d) => b.days.includes(d))
	);
}

/** Per-field problems. Empty object = the form can be sent. */
export function validateLimitsForm(f: LimitsForm, config: Limits): LimitErrors {
	const e: LimitErrors = {};

	const lot = Number(f.max_lot);
	if (f.max_lot.trim() === '' || !Number.isFinite(lot) || lot <= 0) e.max_lot = 'Enter a lot size above 0';
	else if (lot > config.max_lot) e.max_lot = `Can't be above the config value (${num(config.max_lot)})`;

	const pos = Number(f.max_positions);
	if (f.max_positions.trim() === '' || !Number.isInteger(pos) || pos < 1) e.max_positions = 'Enter a whole number, 1 or more';
	else if (pos > config.max_positions) e.max_positions = `Can't be above the config value (${config.max_positions})`;

	const loss = Number(f.max_daily_loss);
	if (f.max_daily_loss.trim() === '' || !Number.isFinite(loss) || loss <= 0) e.max_daily_loss = 'Enter an amount above 0';
	else if (loss > config.max_daily_loss) e.max_daily_loss = `Can't be above the config value (${num(config.max_daily_loss)})`;

	if (f.allowed_symbols.length === 0) {
		e.allowed_symbols = 'Keep at least one symbol (to stop all trading, use the kill switch)';
	} else {
		const extra = f.allowed_symbols.filter((s) => !config.allowed_symbols.includes(s));
		if (extra.length) e.allowed_symbols = `Not allowed by the config: ${extra.join(', ')}`;
	}

	if (f.hours_enabled) {
		const start = parseHHMM(f.start);
		const end = parseHHMM(f.end);
		const cfg = config.trading_hours;
		if (f.days.length === 0) e.trading_hours = 'Pick at least one day';
		else if (start === null || end === null) e.trading_hours = 'Enter the start and end as HH:MM';
		else if (start >= end) e.trading_hours = 'The window must start before it ends, on the same day';
		else if (cfg) {
			const outside = f.days.filter((d) => !cfg.days.includes(d));
			const cs = parseHHMM(cfg.start);
			const ce = parseHHMM(cfg.end);
			if (outside.length) e.trading_hours = `Days outside the config: ${outside.map((d) => WEEKDAY_LABEL[d]).join(', ')}`;
			else if (cs !== null && ce !== null && cs < ce && (start < cs || end > ce))
				e.trading_hours = `Must stay inside the config window (${cfg.start}–${cfg.end})`;
		}
	}
	return e;
}

/**
 * The fields of the form that are stricter than the config (what would be "overridden").
 * A field equal to the config is left out.
 */
export function formToOverrides(f: LimitsForm, config: Limits): LimitsOverrides {
	const out: LimitsOverrides = {};
	const lot = Number(f.max_lot);
	if (lot < config.max_lot) out.max_lot = lot;
	const pos = Number(f.max_positions);
	if (pos < config.max_positions) out.max_positions = pos;
	const loss = Number(f.max_daily_loss);
	if (loss < config.max_daily_loss) out.max_daily_loss = loss;
	if (f.allowed_symbols.length < config.allowed_symbols.length) {
		// Config order, so the request does not depend on click order.
		out.allowed_symbols = config.allowed_symbols.filter((s) => f.allowed_symbols.includes(s));
	}
	if (f.hours_enabled) {
		const days = WEEKDAYS.filter((d) => f.days.includes(d));
		const next: TradingHours = { days: [...days], start: f.start, end: f.end };
		if (!config.trading_hours || !sameHours(next, config.trading_hours)) out.trading_hours = next;
	}
	return out;
}

/**
 * The body for `PUT /api/settings/limits`. The server merges what it is sent into the stored
 * overrides, so a field the user moved back to the config value must be sent explicitly (a value
 * equal to the config is valid) or the old stricter value would stay. `current` is what is stored now.
 * `stuckHours` is true when narrowed hours cannot be undone by a merge (the config has none):
 * the caller tells the user to use "Reset to config".
 */
export function limitsBody(
	f: LimitsForm,
	config: Limits,
	current: LimitsOverrides | null
): { body: LimitsOverrides; stuckHours: boolean } {
	const body: LimitsOverrides = {
		max_lot: Number(f.max_lot),
		max_positions: Number(f.max_positions),
		max_daily_loss: Number(f.max_daily_loss),
		allowed_symbols: config.allowed_symbols.filter((s) => f.allowed_symbols.includes(s))
	};
	let stuckHours = false;
	if (f.hours_enabled) {
		body.trading_hours = {
			days: WEEKDAYS.filter((d) => f.days.includes(d)),
			start: f.start,
			end: f.end
		};
	} else if (current?.trading_hours) {
		if (config.trading_hours) body.trading_hours = config.trading_hours;
		else stuckHours = true;
	}
	return { body, stuckHours };
}

export const hasOverrides = (o: LimitsOverrides): boolean => Object.keys(o).length > 0;

/** Which field a server `limit_not_stricter` message is about, from the field name it mentions. */
export function limitErrorField(message: string): LimitField | null {
	const m = message.toLowerCase();
	for (const f of LIMIT_FIELDS) if (m.includes(f)) return f;
	if (m.includes('trading hours') || m.includes('window')) return 'trading_hours';
	return null;
}

/** "Mon–Fri 00:05–23:55" for a window; consecutive days are joined. */
export function describeHours(h: TradingHours | null): string {
	if (!h) return 'Any time';
	const idx = WEEKDAYS.map((d, i) => (h.days.includes(d) ? i : -1)).filter((i) => i >= 0);
	const runs: string[] = [];
	for (let i = 0; i < idx.length; ) {
		let j = i;
		while (j + 1 < idx.length && idx[j + 1] === idx[j] + 1) j++;
		const a = WEEKDAY_LABEL[WEEKDAYS[idx[i]]];
		const b = WEEKDAY_LABEL[WEEKDAYS[idx[j]]];
		runs.push(j === i ? a : j === i + 1 ? `${a}, ${b}` : `${a}–${b}`);
		i = j + 1;
	}
	return `${runs.join(', ')} ${h.start}–${h.end}`;
}

export function describeLimit(l: Limits, field: LimitField): string {
	switch (field) {
		case 'max_lot':
			return num(l.max_lot);
		case 'max_positions':
			return String(l.max_positions);
		case 'max_daily_loss':
			return num(l.max_daily_loss);
		case 'allowed_symbols':
			return l.allowed_symbols.join(', ') || 'none';
		case 'trading_hours':
			return describeHours(l.trading_hours);
	}
}
