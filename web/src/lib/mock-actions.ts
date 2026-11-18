/**
 * Mock of the assistant-action features (tasks/spec-assistant-actions.md): price alerts that fire
 * when the simulated price crosses them, the daily briefing, and the chat tools that change
 * preferences. Mock mode only.
 *
 * Every tool applies its effect the way the server does and pushes the same event
 * (`settings_changed`, `kill_switch`, `alert_triggered`), so the HUD's live refresh is exercised.
 * `window.__jarvisMock.setPrice("XAUUSD", 2410)` moves the simulated price to cross an alert.
 */
import {
	ApiError,
	type Account,
	type AlertCreateBody,
	type AlertDirection,
	type AlertsResponse,
	type BriefingDto,
	type BriefingView,
	type MarketEvent,
	type Position,
	type PriceAlert,
	type PriceTick,
	type ToolCall
} from './api';
import { quote, symbolFromText, symInfo, SYMS } from './mock-market';
import { getMarketCalendar, getMarketView } from './mock-market-view';
import {
	activePersona,
	effectiveLimits,
	getPersonas,
	getSettings,
	putLimits,
	putSettings,
	resolveSymbol
} from './mock-settings';
import { toBroker, toCanonical } from './mock-symbols';
import { emitMockEvent, killState, setKill } from './mock-trading';

// ---- Alerts ---------------------------------------------------------------------------------------

const ALERTS_KEY = 'arcmt.mock.alerts';
const MAX_ACTIVE = 50;
const RECENT = 20;

let store: PriceAlert[] | null = null;

function all(): PriceAlert[] {
	if (store) return store;
	try {
		const raw = sessionStorage.getItem(ALERTS_KEY);
		store = raw ? (JSON.parse(raw) as PriceAlert[]) : [];
	} catch {
		store = [];
	}
	return store;
}

function persist(): void {
	try {
		sessionStorage.setItem(ALERTS_KEY, JSON.stringify(all()));
	} catch {
		/* storage unavailable: alerts last until reload */
	}
}

const iso = (ms = Date.now()) => new Date(ms).toISOString();

/** The canonical symbol a typed name means, or null. */
export function canonicalFor(raw: string): string | null {
	const text = raw.trim();
	if (!text) return null;
	const found = resolveSymbol(text) ?? resolveSymbol(toCanonical(text));
	return found && symInfo(found) ? found : null;
}

export function listAlerts(): AlertsResponse {
	const list = all();
	const newest = (a: PriceAlert, b: PriceAlert) => b.created_at.localeCompare(a.created_at);
	return {
		active: list.filter((a) => a.status === 'active').sort(newest),
		recent: list
			.filter((a) => a.status === 'triggered')
			.sort((a, b) => (b.triggered_at ?? '').localeCompare(a.triggered_at ?? ''))
			.slice(0, RECENT)
	};
}

export function createAlert(body: unknown): PriceAlert {
	const b = (body ?? {}) as Partial<AlertCreateBody>;
	if (typeof b.symbol !== 'string' || b.symbol.trim() === '') throw new ApiError('bad_request', 'symbol is required', 400);
	if (typeof b.price !== 'number' || !Number.isFinite(b.price) || b.price <= 0)
		throw new ApiError('bad_request', 'price must be greater than 0', 400);
	if (b.direction !== undefined && b.direction !== 'above' && b.direction !== 'below')
		throw new ApiError('bad_request', 'direction must be above or below', 400);
	if (b.note !== undefined && (typeof b.note !== 'string' || b.note.length > 200))
		throw new ApiError('bad_request', 'note must be text of at most 200 characters', 400);
	const symbol = canonicalFor(b.symbol);
	if (!symbol) throw new ApiError('bad_request', `unknown symbol: ${b.symbol}`, 400);
	if (all().filter((a) => a.status === 'active').length >= MAX_ACTIVE)
		throw new ApiError('gate_rejected', `at most ${MAX_ACTIVE} active alerts`, 422);
	const q = quote(symbol)!;
	const direction: AlertDirection = b.direction ?? (b.price > q.bid ? 'above' : 'below');
	const alert: PriceAlert = {
		id: crypto.randomUUID(),
		symbol,
		broker_symbol: toBroker(symbol),
		direction,
		price: b.price,
		note: b.note?.trim() ? b.note.trim() : null,
		status: 'active',
		created_at: iso(),
		triggered_at: null,
		trigger_price: null
	};
	all().push(alert);
	persist();
	return alert;
}

export function cancelAlert(id: string): PriceAlert {
	const a = all().find((x) => x.id === id && x.status === 'active');
	if (!a) throw new ApiError('bad_request', 'no active alert with that id', 404);
	a.status = 'cancelled';
	persist();
	return { ...a };
}

/** Called after every price step: `above` fires at bid >= price, `below` at ask <= price. One-shot. */
export function checkAlerts(ticks: PriceTick[]): void {
	let changed = false;
	for (const a of all()) {
		if (a.status !== 'active') continue;
		const t = ticks.find((x) => x.symbol === a.symbol);
		if (!t) continue;
		const hit = a.direction === 'above' ? t.bid >= a.price : t.ask <= a.price;
		if (!hit) continue;
		a.status = 'triggered';
		a.triggered_at = iso();
		a.trigger_price = a.direction === 'above' ? t.bid : t.ask;
		changed = true;
		emitMockEvent({
			type: 'alert_triggered',
			id: a.id,
			symbol: a.symbol,
			direction: a.direction,
			price: a.price,
			trigger_price: a.trigger_price,
			note: a.note,
			triggered_at: a.triggered_at
		});
	}
	if (changed) persist();
}

// ---- Briefing -------------------------------------------------------------------------------------------

/** Figures for the events that carry them, so the card shows forecast and previous values. */
const FIGURES: Record<string, Pick<MarketEvent, 'forecast' | 'previous' | 'actual' | 'unit'>> = {
	'Nonfarm Payrolls': { actual: null, forecast: 185, previous: 175, unit: 'K' },
	'Unemployment Rate': { actual: null, forecast: 4.1, previous: 4.0, unit: '%' },
	'Core PCE Price Index m/m': { actual: null, forecast: 0.3, previous: 0.2, unit: '%' }
};

const MAX_BRIEFING_VIEWS = 8;

export function buildBriefing(account: Account, positions: Position[]): BriefingDto {
	const events: MarketEvent[] = getMarketCalendar('/api/market/calendar?hours=24').events
		.filter((e) => e.importance === 'high')
		.map((e) => ({ ...e, ...(FIGURES[e.title] ?? {}) }));

	const names = getSettings().watchlist.slice(0, MAX_BRIEFING_VIEWS);
	const seen = new Set<string>();
	const views: BriefingView[] = [];
	for (const name of names) {
		const symbol = canonicalFor(name);
		if (symbol && seen.has(symbol)) continue;
		if (symbol) seen.add(symbol);
		if (!symbol) {
			views.push({
				symbol: name,
				broker_symbol: name,
				bias: 'neutral',
				confidence: 0,
				stand_aside: false,
				stand_aside_reason: null,
				summary: '',
				unavailable: 'no prices for this symbol'
			});
			continue;
		}
		const v = getMarketView(`/api/market/view?symbol=${symbol}`);
		views.push({
			symbol: v.symbol,
			broker_symbol: v.broker_symbol,
			bias: v.bias,
			confidence: v.confidence,
			stand_aside: v.stand_aside,
			stand_aside_reason: v.stand_aside_reason,
			summary: v.summary
		});
	}

	const pl = positions.reduce((a, p) => a + p.profit, 0);
	const fmt = (n: number) => n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
	const ok = views.filter((v) => !v.unavailable);
	const stand = ok.filter((v) => v.stand_aside).map((v) => v.symbol);
	const summary =
		`Equity is ${fmt(account.equity)} ${account.currency} with ${positions.length} open position${positions.length === 1 ? '' : 's'} ` +
		`and a floating P/L of ${pl >= 0 ? '+' : '-'}${fmt(Math.abs(pl))} ${account.currency}. ` +
		`${events.length} high-impact event${events.length === 1 ? '' : 's'} today. ` +
		(ok.length > 0
			? `${ok.map((v) => `${v.symbol} ${v.bias}`).join(', ')}.${stand.length ? ` Stand aside: ${stand.join(', ')}.` : ''}`
			: 'No symbol views are available.') +
		' This is a computed analysis, not a guarantee.';
	return { generated_at: iso(), account, positions, events, views, summary };
}

// ---- Chat tools -------------------------------------------------------------------------------------------

export interface ToolReply {
	reply: string;
	tool_calls: ToolCall[];
}

interface Ctx {
	account: () => Account;
	positions: () => Position[];
	providers: { name: string; available: boolean }[];
}

const call = (name: string, args: Record<string, unknown>, ok = true, ms = 14): ToolCall => ({
	name,
	arguments: args,
	ok,
	duration_ms: ms
});

const money = (v: number, d = 2) => v.toLocaleString('en-US', { minimumFractionDigits: d, maximumFractionDigits: d });
const refuse = (name: string, args: Record<string, unknown>, reply: string): ToolReply => ({
	reply,
	tool_calls: [call(name, args, false, 6)]
});

const num = (s: string | undefined) => (s ? Number(s.replace(/,/g, '')) : NaN);

/** A watchlist entry the user named, as the server would store it (canonical when it knows it). */
function entryFor(raw: string): string {
	const word = raw.trim();
	return symbolFromText(word.toLowerCase()) ?? word.toUpperCase();
}

/**
 * The preference and read tools the mock chat understands. Returns null when the message is
 * about something else, so the caller carries on with trades, prices and the rest.
 */
export function handleToolChat(text: string, ctx: Ctx): ToolReply | null {
	const t = text.toLowerCase();

	// ---- Kill switch: engage only; releasing is never a chat action.
	if (/\b(release|disengage|disable|turn off|clear|lift|reset)\b.*\bkill\s*switch\b/.test(t)) {
		return {
			reply:
				"I can't release the kill switch from chat. Use the Release button in the top bar; it needs your authenticator code when login is on.",
			tool_calls: []
		};
	}
	if (/\b(engage|activate|trigger|flip|turn on|enable|hit|pull)\b.*\bkill\s*switch\b/.test(t)) {
		const reason = text.match(/\b(?:because|reason|since)\b[:\s]+(.{3,80})/i)?.[1]?.trim() ?? 'engaged from chat';
		const state = killState();
		if (!state.engaged) setKill(true, reason);
		return {
			reply: state.engaged
				? 'The kill switch was already engaged. Trading stays disabled.'
				: `The kill switch is engaged (${reason}). Trading is disabled until you release it in the top bar.`,
			tool_calls: [call('engage_kill_switch', { reason })]
		};
	}

	// ---- Limits: tighten only.
	const limit = t.match(/\b(max[ _-]?lots?|max[ _-]?positions?|(?:max[ _-]?)?daily[ _-]?loss)\b[^\d]{0,30}(\d+(?:\.\d+)?)/);
	if (limit && /\b(tighten|lower|reduce|cut|cap|raise|increase|loosen|relax|set|change|to)\b/.test(t)) {
		const rule = /lot/.test(limit[1]) ? 'max_lot' : /position/.test(limit[1]) ? 'max_positions' : 'max_daily_loss';
		const value = Number(limit[2]);
		const before = effectiveLimits()[rule];
		try {
			putLimits({ [rule]: value }, () => {});
		} catch (e) {
			if (e instanceof ApiError && (e.code === 'limit_not_stricter' || e.code === 'bad_request')) {
				return refuse(
					'tighten_limit',
					{ rule, value },
					e.code === 'limit_not_stricter'
						? `I can't loosen a limit from chat: ${e.message}. Loosening is only possible in Settings.`
						: `I can't set that: ${e.message}.`
				);
			}
			throw e;
		}
		emitMockEvent({ type: 'settings_changed' });
		return {
			reply: `Done. ${rule.replace(/_/g, ' ')} is now ${value} (it was ${before}).`,
			tool_calls: [call('tighten_limit', { rule, value })]
		};
	}

	// ---- Alerts
	if (/\balerts?\b/.test(t) && /\b(cancel|remove|delete|clear|drop)\b/.test(t)) {
		const symbol = symbolFromText(t);
		const mine = listAlerts().active.filter((a) => !symbol || a.symbol === symbol);
		if (mine.length === 0)
			return refuse('cancel_price_alert', { symbol }, `You have no active alerts${symbol ? ` on ${symbol}` : ''} to cancel.`);
		for (const a of mine) cancelAlert(a.id);
		emitMockEvent({ type: 'settings_changed' });
		return {
			reply: `Cancelled ${mine.length} alert${mine.length === 1 ? '' : 's'}${symbol ? ` on ${symbol}` : ''}.`,
			tool_calls: [call('cancel_price_alert', symbol ? { symbol } : {})]
		};
	}
	if (/\balerts?\b/.test(t) && /\b(list|show|what|which|any|my)\b/.test(t) && !/\b(when|reach|hits?)\b/.test(t)) {
		const { active } = listAlerts();
		return {
			reply: active.length
				? `You have ${active.length} active alert${active.length === 1 ? '' : 's'}: ${active.map((a) => `${a.symbol} ${a.direction} ${a.price}`).join(', ')}.`
				: 'You have no active alerts.',
			tool_calls: [call('list_price_alerts', {})]
		};
	}
	if (/\balerts?\b/.test(t) || (/\b(tell me|let me know|notify me|ping me)\b/.test(t) && /\b(when|once|if)\b/.test(t))) {
		const symbol = symbolFromText(t);
		const price = num(t.match(/\b(?:reach(?:es)?|hits?|touch(?:es)?|cross(?:es)?|above|below|under|over|at|to)\s+\$?(\d[\d,]*(?:\.\d+)?)/)?.[1]);
		if (!symbol || !Number.isFinite(price))
			return {
				reply: 'Which symbol and price should the alert be on? For example: "tell me when gold reaches 2400".',
				tool_calls: []
			};
		const direction: AlertDirection | undefined = /\b(below|under|drops?|falls?)\b/.test(t)
			? 'below'
			: /\b(above|over|rises?|breaks?)\b/.test(t)
				? 'above'
				: undefined;
		try {
			const made = createAlert({ symbol, price, ...(direction ? { direction } : {}) });
			emitMockEvent({ type: 'settings_changed' });
			return {
				reply: `Done. I'll tell you when ${made.symbol} goes ${made.direction} ${money(made.price, symInfo(made.symbol)?.digits ?? 2)}. It fires once and never trades.`,
				tool_calls: [call('create_price_alert', { symbol, price, ...(direction ? { direction } : {}) })]
			};
		} catch (e) {
			if (e instanceof ApiError) return refuse('create_price_alert', { symbol, price }, `I couldn't set that alert: ${e.message}.`);
			throw e;
		}
	}

	// ---- Briefing and symbol info (read tools)
	if (/\b(brief me|briefing|morning brief|daily brief|brief)\b/.test(t)) {
		const b = buildBriefing(ctx.account(), ctx.positions());
		return { reply: b.summary, tool_calls: [call('get_briefing', {}, true, 900)] };
	}
	const infoAsk =
		/\b(spread|minimum lot|min lot|lot size|contract size|symbol info|swaps?|stops? level|tick size|tick value|digits)\b/.test(t) ||
		/\bmarket (?:is )?open\b|\b(?:is|are)\b.*\b(?:open|closed)\b.*\bnow\b/.test(t);
	if (infoAsk && symbolFromText(t)) {
		const symbol = symbolFromText(t)!;
		const s = symInfo(symbol)!;
		const q = quote(symbol)!;
		const open = symbol !== 'US100';
		const spread = Math.round((q.ask - q.bid) / s.point);
		return {
			reply:
				`${symbol}: spread ${spread} points (${money(q.ask - q.bid, s.digits)}), contract size ${money(s.contract, 0)}, ` +
				`minimum lot 0.01, step 0.01, maximum 100. The market is ${open ? 'open' : 'closed'} now.`,
			tool_calls: [call('get_symbol_info', { symbol })]
		};
	}

	// ---- Watchlist
	const wl = t.match(/\b(add|remove|drop|delete)\b\s+(.+?)\s+(?:to|from|on|off)\s+(?:my\s+|the\s+)?watch\s*list\b/);
	if (wl) {
		const add = wl[1] === 'add';
		const entry = entryFor(wl[2]);
		const list = getSettings().watchlist;
		const key = (x: string) => (resolveSymbol(x) ?? x).toLowerCase();
		const present = list.some((x) => key(x) === key(entry));
		if (add && present) return refuse('update_watchlist', { add: [entry] }, `${entry} is already on your watchlist.`);
		if (!add && !present) return refuse('update_watchlist', { remove: [entry] }, `${entry} is not on your watchlist.`);
		const next = add ? [...list, entry] : list.filter((x) => key(x) !== key(entry));
		try {
			putSettings({ watchlist: next }, ctx.providers);
		} catch (e) {
			if (e instanceof ApiError) return refuse('update_watchlist', add ? { add: [entry] } : { remove: [entry] }, `I couldn't change the watchlist: ${e.message}.`);
			throw e;
		}
		emitMockEvent({ type: 'settings_changed' });
		return {
			reply: `${add ? 'Added' : 'Removed'} ${entry} ${add ? 'to' : 'from'} your watchlist. It now has ${next.length} symbols.`,
			tool_calls: [call('update_watchlist', add ? { add: [entry] } : { remove: [entry] })]
		};
	}

	// ---- Persona
	const who = t.match(/\b(?:switch|change|swap|use|be|talk to|go)\b.*\b(jarvis|friday)\b/)?.[1];
	if (who) {
		if (activePersona().id === who) return { reply: `${activePersona().name} is already your assistant.`, tool_calls: [call('set_persona', { persona: who })] };
		putSettings({ persona: who }, ctx.providers);
		emitMockEvent({ type: 'settings_changed' });
		const name = getPersonas().personas.find((p) => p.id === who)!.name;
		return { reply: `Switched to ${name}. The next reply uses ${name}.`, tool_calls: [call('set_persona', { persona: who })] };
	}

	// ---- Model
	const model = t.match(/\b(?:model|provider)\b.*\b(local|cloud|default)\b|\b(?:switch|change|use)\b.*\b(local|cloud)\b.*\b(?:model|provider)\b/);
	if (model) {
		const name = model[1] ?? model[2];
		const target = name === 'default' ? null : name;
		const p = ctx.providers.find((x) => x.name === name);
		if (target !== null && !p) return refuse('set_model', { provider: name }, `There is no model provider called ${name}.`);
		if (p && !p.available) return refuse('set_model', { provider: name }, `The ${name} provider is not available (its API key is not set).`);
		putSettings({ llm_provider: target }, ctx.providers);
		emitMockEvent({ type: 'settings_changed' });
		return {
			reply: `Switched to the ${name} model. It takes effect from the next message.`,
			tool_calls: [call('set_model', { provider: name })]
		};
	}

	// ---- Spoken names
	const removeAlias = t.match(/\bremove\s+(?:the\s+)?(?:spoken name|alias)\s+"?([a-z][a-z0-9 ]*?)"?\s*$/);
	if (removeAlias) {
		const spoken = removeAlias[1].trim();
		const aliases = { ...getSettings().aliases };
		if (!(spoken in aliases)) return refuse('remove_alias', { spoken }, `You have no spoken name "${spoken}".`);
		delete aliases[spoken];
		putSettings({ aliases }, ctx.providers);
		emitMockEvent({ type: 'settings_changed' });
		return { reply: `Removed the spoken name "${spoken}".`, tool_calls: [call('remove_alias', { spoken })] };
	}
	const setAlias = t.match(/\b(?:alias|spoken name)\s+"?([a-z][a-z0-9 ]*?)"?\s+(?:for|to|as)\s+([a-z0-9._#]+)/);
	if (setAlias) {
		const spoken = setAlias[1].trim();
		const symbol = symbolFromText(setAlias[2]) ?? setAlias[2].toUpperCase();
		if (!SYMS.some((s) => s.symbol === symbol))
			return refuse('set_alias', { spoken, symbol }, `I don't know the symbol ${symbol}, so I can't point "${spoken}" at it.`);
		putSettings({ aliases: { ...getSettings().aliases, [spoken]: symbol } }, ctx.providers);
		emitMockEvent({ type: 'settings_changed' });
		return { reply: `Done. "${spoken}" now means ${symbol}.`, tool_calls: [call('set_alias', { spoken, symbol })] };
	}

	return null;
}
