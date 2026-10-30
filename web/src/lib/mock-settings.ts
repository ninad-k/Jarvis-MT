/**
 * Mock of the persona, settings and limits endpoints (docs/api.md "Personas and user settings").
 * Mock mode only (api.ts imports ./mock behind a build-time constant).
 * State lives in sessionStorage so a reload keeps it, like a real profile would.
 */
import {
	ApiError,
	type Limits,
	type LimitsOverrides,
	type LimitsResponse,
	type Persona,
	type PersonasResponse,
	type Settings,
	type TradingHours,
	type WakeResult,
	type Weekday
} from './api';
import { SYMS } from './mock-market';
import { MAX_ALIASES, MAX_WATCHLIST, SYMBOL_RE, WATCH_ENTRY_RE, WEEKDAYS, parseHHMM } from './settings-rules';

export const PERSONAS: Persona[] = [
	{
		id: 'jarvis',
		name: 'Jarvis',
		description: 'Calm, formal and concise.',
		wake_word: 'hey jarvis',
		voice_available: true,
		theme: { accent: '#38bdf8', accent_soft: '#0c4a6e', orb_core: '#7dd3fc', orb_glow: '#0ea5e9' }
	},
	{
		id: 'friday',
		name: 'Friday',
		description: 'Warm, upbeat and casual.',
		wake_word: 'hey friday',
		voice_available: true,
		theme: { accent: '#f5b942', accent_soft: '#5c3d0a', orb_core: '#fcd77f', orb_glow: '#f59e0b' }
	}
];

export const CONFIG_ALIASES: Record<string, string> = { gold: 'XAUUSD', nasdaq: 'US100', dow: 'US30' };

export const CONFIG_LIMITS: Limits = {
	max_lot: 1,
	max_positions: 5,
	max_daily_loss: 500,
	allowed_symbols: ['XAUUSD', 'EURUSD', 'GBPUSD', 'USDJPY', 'US100', 'BTCUSD'],
	trading_hours: { days: ['mon', 'tue', 'wed', 'thu', 'fri'], start: '00:05', end: '23:55' }
};

const SETTINGS_KEY = 'arcmt.mock.settings';
const LIMITS_KEY = 'arcmt.mock.limits';

const DEFAULT_SETTINGS: Settings = {
	persona: 'jarvis',
	llm_provider: null,
	voice: { read_aloud: false, input_mode: 'hold', language: 'en' },
	watchlist: ['XAUUSD', 'EURUSD', 'GBPUSD', 'USDJPY', 'US100', 'BTCUSD', 'US30'],
	aliases: { 'yellow metal': 'XAUUSD' }
};

function load<T>(key: string, fallback: T): T {
	try {
		const raw = sessionStorage.getItem(key);
		return raw ? (JSON.parse(raw) as T) : structuredClone(fallback);
	} catch {
		return structuredClone(fallback);
	}
}

function save(key: string, value: unknown): void {
	try {
		sessionStorage.setItem(key, JSON.stringify(value));
	} catch {
		/* storage unavailable: state lasts until reload */
	}
}

let settings: Settings | null = null;
let overrides: LimitsOverrides | null | undefined;

const cur = (): Settings => (settings ??= load(SETTINGS_KEY, DEFAULT_SETTINGS));
const curOverrides = (): LimitsOverrides | null => {
	if (overrides === undefined) overrides = load<LimitsOverrides | null>(LIMITS_KEY, null);
	return overrides;
};

const bad = (message: string) => new ApiError('bad_request', message, 400);

// ---- Personas ------------------------------------------------------------------

export function getPersonas(): PersonasResponse {
	return { default: 'jarvis', active: cur().persona, personas: PERSONAS };
}

export const activePersona = (): Persona => PERSONAS.find((p) => p.id === cur().persona) ?? PERSONAS[0];

// ---- Settings --------------------------------------------------------------------

export function getSettings(): Settings {
	return { ...structuredClone(cur()), config_aliases: CONFIG_ALIASES };
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

/** Apply a partial update like the server: unknown fields, personas and providers are 400s. */
export function putSettings(
	body: unknown,
	providers: { name: string; available: boolean }[]
): { settings: Settings; changed: boolean } {
	if (!isObj(body)) throw bad('body must be a JSON object');
	const next = structuredClone(cur());
	const known = ['persona', 'llm_provider', 'voice', 'watchlist', 'aliases'];
	for (const k of Object.keys(body)) if (!known.includes(k)) throw bad(`unknown field: ${k}`);

	if ('persona' in body) {
		if (typeof body.persona !== 'string' || !PERSONAS.some((p) => p.id === body.persona))
			throw bad(`unknown persona: ${String(body.persona)}`);
		next.persona = body.persona;
	}
	if ('llm_provider' in body) {
		const v = body.llm_provider;
		if (v !== null) {
			const p = providers.find((x) => x.name === v);
			if (typeof v !== 'string' || !p) throw bad(`unknown provider: ${String(v)}`);
			if (!p.available) throw bad(`provider ${v} is not available (API key not set)`);
		}
		next.llm_provider = v as string | null;
	}
	if ('voice' in body) {
		if (!isObj(body.voice)) throw bad('voice must be an object');
		const v = { ...next.voice };
		for (const [k, val] of Object.entries(body.voice)) {
			if (k === 'read_aloud' && typeof val === 'boolean') v.read_aloud = val;
			else if (k === 'input_mode' && (val === 'hold' || val === 'toggle' || val === 'wake_word')) v.input_mode = val;
			else if (k === 'language' && (val === 'en' || val === 'auto')) v.language = val;
			else throw bad(`invalid voice.${k}`);
		}
		next.voice = v;
	}
	if ('watchlist' in body) {
		const w = body.watchlist;
		if (!Array.isArray(w) || w.some((s) => typeof s !== 'string')) throw bad('watchlist must be a list of symbols');
		if (w.length > MAX_WATCHLIST) throw bad(`watchlist: at most ${MAX_WATCHLIST} symbols`);
		const seen = new Set<string>();
		for (const s of w as string[]) {
			if (!WATCH_ENTRY_RE.test(s)) throw bad(`watchlist: invalid entry "${s}" (letters, digits, spaces and . _ # - only, 1-64 characters)`);
			if (seen.has(s.toLowerCase())) throw bad(`watchlist: "${s}" appears twice`);
			seen.add(s.toLowerCase());
		}
		next.watchlist = w as string[];
	}
	if ('aliases' in body) {
		const a = body.aliases;
		if (!isObj(a)) throw bad('aliases must be an object');
		const entries = Object.entries(a);
		if (entries.length > MAX_ALIASES) throw bad(`aliases: at most ${MAX_ALIASES} entries`);
		const out: Record<string, string> = {};
		for (const [k, v] of entries) {
			if (typeof v !== 'string' || !SYMBOL_RE.test(v))
				throw bad(`aliases: invalid symbol for "${k}" (letters, digits and . _ # only, 1-32 characters)`);
			if (k.trim() === '') throw bad('aliases: spoken name must not be empty');
			out[k.trim().toLowerCase()] = v;
		}
		next.aliases = out;
	}

	const changed = JSON.stringify(next) !== JSON.stringify(cur());
	settings = next;
	save(SETTINGS_KEY, next);
	return { settings: getSettings(), changed };
}

/** Switch persona the way a wake phrase does. */
export function setPersona(id: string): void {
	const next = { ...cur(), persona: id };
	settings = next;
	save(SETTINGS_KEY, next);
}

/** The broker symbol a watchlist entry means, or null (alias lookup: the user's entry wins). */
export function resolveSymbol(name: string): string | null {
	const direct = SYMS.find((s) => s.symbol.toLowerCase() === name.toLowerCase());
	if (direct) return direct.symbol;
	const key = name.trim().toLowerCase();
	const via = cur().aliases[key] ?? CONFIG_ALIASES[key];
	return via ?? null;
}

// ---- Wake word --------------------------------------------------------------------

/** Text starts with "hey <name>" (punctuation ignored): who was called and what followed. */
export function detectWake(text: string): WakeResult {
	const t = text
		.toLowerCase()
		.replace(/[^a-z0-9' ]+/g, ' ')
		.replace(/\s+/g, ' ')
		.trim();
	for (const p of PERSONAS) {
		if (t === p.wake_word || t.startsWith(`${p.wake_word} `)) {
			return { detected: true, persona: p.id, command: t.slice(p.wake_word.length).trim() };
		}
	}
	return { detected: false, persona: null, command: '' };
}

// ---- Limits (tighten-only) ---------------------------------------------------------------

function merged(o: LimitsOverrides | null): Limits {
	return {
		max_lot: o?.max_lot ?? CONFIG_LIMITS.max_lot,
		max_positions: o?.max_positions ?? CONFIG_LIMITS.max_positions,
		max_daily_loss: o?.max_daily_loss ?? CONFIG_LIMITS.max_daily_loss,
		allowed_symbols: o?.allowed_symbols ?? CONFIG_LIMITS.allowed_symbols,
		trading_hours: o?.trading_hours ?? CONFIG_LIMITS.trading_hours
	};
}

export const effectiveLimits = (): Limits => merged(curOverrides());

export function getLimits(): LimitsResponse {
	return structuredClone({ config: CONFIG_LIMITS, overrides: curOverrides(), effective: effectiveLimits() });
}

const looser = (message: string) => new ApiError('limit_not_stricter', message, 422);

function checkHours(h: unknown): TradingHours {
	if (!isObj(h) || !Array.isArray(h.days) || typeof h.start !== 'string' || typeof h.end !== 'string')
		throw bad('trading_hours must be {days, start, end}');
	const days = h.days as unknown[];
	if (days.length === 0 || days.some((d) => !WEEKDAYS.includes(d as Weekday))) throw bad('trading_hours.days: pick days from mon..sun');
	const start = parseHHMM(h.start);
	const end = parseHHMM(h.end);
	if (start === null || end === null) throw bad('trading_hours: start and end must be HH:MM');
	if (start >= end) throw looser('trading_hours: only same-day windows (start before end) can be set here');
	const cfg = CONFIG_LIMITS.trading_hours;
	if (cfg) {
		const extra = (days as Weekday[]).filter((d) => !cfg.days.includes(d));
		if (extra.length) throw looser(`trading_hours: days ${extra.join(', ')} are not allowed by the config`);
		const cs = parseHHMM(cfg.start)!;
		const ce = parseHHMM(cfg.end)!;
		if (start < cs || end > ce) throw looser(`trading_hours: window ${h.start}-${h.end} is looser than the config window ${cfg.start}-${cfg.end}`);
	}
	return { days: days as Weekday[], start: h.start, end: h.end };
}

/**
 * `PUT /api/settings/limits`. Like the server: the body is validated first, then `checkCode` runs
 * (it throws for a bad TOTP code), then the fields are merged into the stored overrides.
 */
export function putLimits(body: unknown, checkCode: () => void): LimitsResponse {
	if (!isObj(body)) throw bad('body must be a JSON object');
	const known = ['totp', 'max_lot', 'max_positions', 'max_daily_loss', 'allowed_symbols', 'trading_hours'];
	for (const k of Object.keys(body)) if (!known.includes(k)) throw bad(`unknown field: ${k}`);
	const next: LimitsOverrides = {};

	if ('max_lot' in body) {
		const v = body.max_lot;
		if (typeof v !== 'number' || !(v > 0)) throw bad('max_lot must be a number above 0');
		if (v > CONFIG_LIMITS.max_lot) throw looser(`max_lot ${v} is looser than the config value ${CONFIG_LIMITS.max_lot}`);
		next.max_lot = v;
	}
	if ('max_positions' in body) {
		const v = body.max_positions;
		if (typeof v !== 'number' || !Number.isInteger(v) || v < 1) throw bad('max_positions must be a whole number, 1 or more');
		if (v > CONFIG_LIMITS.max_positions) throw looser(`max_positions ${v} is looser than the config value ${CONFIG_LIMITS.max_positions}`);
		next.max_positions = v;
	}
	if ('max_daily_loss' in body) {
		const v = body.max_daily_loss;
		if (typeof v !== 'number' || !(v > 0)) throw bad('max_daily_loss must be a number above 0');
		if (v > CONFIG_LIMITS.max_daily_loss) throw looser(`max_daily_loss ${v} is looser than the config value ${CONFIG_LIMITS.max_daily_loss}`);
		next.max_daily_loss = v;
	}
	if ('allowed_symbols' in body) {
		const v = body.allowed_symbols;
		if (!Array.isArray(v) || v.some((s) => typeof s !== 'string')) throw bad('allowed_symbols must be a list of symbols');
		const extra = (v as string[]).filter((s) => !CONFIG_LIMITS.allowed_symbols.includes(s));
		if (extra.length) throw looser(`allowed_symbols: ${extra.join(', ')} not in the config list`);
		next.allowed_symbols = v as string[];
	}
	if ('trading_hours' in body) next.trading_hours = checkHours(body.trading_hours);

	if (Object.keys(next).length === 0) throw bad('no limits given');
	checkCode();
	overrides = { ...curOverrides(), ...next };
	save(LIMITS_KEY, overrides);
	return getLimits();
}

export function deleteLimits(): LimitsResponse {
	overrides = null;
	save(LIMITS_KEY, null);
	return getLimits();
}
