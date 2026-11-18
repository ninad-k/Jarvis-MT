/**
 * Mock of the market view endpoints (tasks/spec-market-view.md section 4). Mock mode only.
 *
 * XAUUSD is bullish with a setup and configured news, EURUSD stands aside for a high-impact USD
 * event 20 minutes away, US100 is neutral without a setup. Any other known symbol gets a quiet
 * neutral view. Prices follow the simulated market, so a view lines up with the watchlist.
 * Every view made is journaled as open, next to a seeded history with mixed outcomes.
 *
 * Switch: `?mock=viewttl=5` makes views valid for 5 seconds (to see the expiry state).
 */
import {
	ApiError,
	type Bias,
	type MarketCalendarResponse,
	type MarketEvent,
	type MarketFactor,
	type MarketHeadline,
	type MarketJournalResponse,
	type MarketJournalStats,
	type MarketJournalView,
	type MarketLevels,
	type MarketNews,
	type MarketNewsResponse,
	type MarketOutcome,
	type MarketSetup,
	type MarketTimeframe,
	type MarketViewDto,
	type PreviewDto,
	type Side
} from './api';
import { DISCLAIMER } from './market';
import { mid, round, symInfo, SYMS } from './mock-market';
import { resolveSymbol } from './mock-settings';
import { toBroker, toCanonical } from './mock-symbols';
import { assertWritable, limitPreview, openPreview } from './mock-trading';

const MIN = 60_000;
const HORIZON_MIN = 240;

// ---- Profiles ---------------------------------------------------------------------------------------

interface EventSeed {
	minutes: number;
	currency: string;
	title: string;
	importance: MarketEvent['importance'];
}

interface Profile {
	bias: Bias;
	currencies: string[];
	/** ATR(14) on H1 and M15 as a share of price. */
	atrH1: number;
	atrM15: number;
	/** Factor points, in the order of the scoring table (the news factor is added when news is on). */
	factors: Array<[name: string, points: number, detail: string]>;
	/** Trend, RSI and MACD histogram sign per frame: M5, M15, H1, H4, D1. */
	frames: Array<[tf: string, trend: string, rsi: number, hist: number]>;
	events: EventSeed[];
	standAside: string | null;
	setup: boolean;
	news: boolean;
	/** The session VWAP sits below the price (true) or above it. */
	vwapBelow: boolean;
}

const GOLD: Profile = {
	bias: 'bullish',
	currencies: ['USD'],
	atrH1: 0.0052,
	atrM15: 0.0021,
	factors: [
		['H4 trend', 25, 'EMA20 above EMA50, close above EMA50'],
		['H1 trend', 20, 'EMA20 above EMA50, close above EMA50'],
		['M15 momentum', 10, 'MACD histogram positive and growing over 3 bars'],
		['M15 RSI', 5, 'RSI 58.2, in the 55 to 70 zone'],
		['VWAP', 10, 'Price above the session VWAP'],
		['Daily pivot', -10, 'Price below the pivot of the previous day']
	],
	frames: [
		['M5', 'flat', 51.0, -0.04],
		['M15', 'up', 58.2, 0.21],
		['H1', 'up', 58.4, 0.84],
		['H4', 'up', 61.2, 2.31],
		['D1', 'up', 56.7, 5.4]
	],
	events: [
		{ minutes: -12, currency: 'USD', title: 'ISM Services PMI', importance: 'moderate' },
		{ minutes: 95, currency: 'USD', title: 'Initial Jobless Claims', importance: 'moderate' },
		{ minutes: 140, currency: 'USD', title: 'Core PCE Price Index m/m', importance: 'high' }
	],
	standAside: null,
	setup: true,
	news: true,
	vwapBelow: true
};

const EURO: Profile = {
	bias: 'bearish',
	currencies: ['EUR', 'USD'],
	atrH1: 0.0011,
	atrM15: 0.0005,
	factors: [
		['H4 trend', -25, 'EMA20 below EMA50, close below EMA50'],
		['H1 trend', 0, 'EMA20 and the close disagree'],
		['M15 momentum', -5, 'MACD histogram negative'],
		['M15 RSI', -5, 'RSI 41.3, in the 30 to 45 zone'],
		['VWAP', -10, 'Price below the session VWAP'],
		['Daily pivot', -10, 'Price below the pivot of the previous day']
	],
	frames: [
		['M5', 'down', 38.5, -0.00002],
		['M15', 'down', 41.3, -0.00004],
		['H1', 'flat', 47.9, -0.00001],
		['H4', 'down', 40.2, -0.00011],
		['D1', 'down', 43.8, -0.0003]
	],
	events: [
		{ minutes: 20, currency: 'USD', title: 'Nonfarm Payrolls', importance: 'high' },
		{ minutes: 20, currency: 'USD', title: 'Unemployment Rate', importance: 'high' },
		{ minutes: 190, currency: 'EUR', title: 'ECB President Speech', importance: 'moderate' }
	],
	standAside: 'High-impact USD event in 20 minutes: Nonfarm Payrolls',
	setup: false,
	news: false,
	vwapBelow: false
};

const QUIET: Profile = {
	bias: 'neutral',
	currencies: ['USD'],
	atrH1: 0.0045,
	atrM15: 0.0018,
	factors: [
		['H4 trend', 25, 'EMA20 above EMA50, close above EMA50'],
		['H1 trend', -20, 'EMA20 below EMA50, close below EMA50'],
		['M15 momentum', -5, 'MACD histogram negative'],
		['M15 RSI', 5, 'RSI 52.0, in the 45 to 55 zone'],
		['VWAP', 10, 'Price above the session VWAP'],
		['Daily pivot', -10, 'Price below the pivot of the previous day']
	],
	frames: [
		['M5', 'flat', 49.6, 0.3],
		['M15', 'down', 47.2, -1.2],
		['H1', 'down', 46.5, -3.1],
		['H4', 'up', 54.8, 8.7],
		['D1', 'up', 53.1, 22.0]
	],
	events: [
		{ minutes: 95, currency: 'USD', title: 'Initial Jobless Claims', importance: 'moderate' },
		{ minutes: 310, currency: 'USD', title: 'Fed Chair Speech', importance: 'high' }
	],
	standAside: null,
	setup: false,
	news: false,
	vwapBelow: true
};

const PROFILES: Record<string, Profile> = { XAUUSD: GOLD, EURUSD: EURO, US100: QUIET };

const HEADLINES: Array<{ title: string; source: string; ago: number; tone: number }> = [
	{ title: 'Gold edges higher as traders weigh the next rate decision', source: 'Example Wire', ago: 25, tone: 0.4 },
	{ title: 'Dollar softens ahead of inflation data, supporting bullion', source: 'Market Daily', ago: 70, tone: 0.5 },
	{ title: 'Central banks keep buying gold, survey says', source: 'Example Wire', ago: 150, tone: 0.3 },
	{ title: 'Gold holds a narrow range after a choppy session', source: 'Market Daily', ago: 260, tone: 0.05 },
	{ title: 'Profit taking caps gold near a one-week high', source: 'Trade Notes', ago: 420, tone: -0.3 }
];

// ---- Builder ----------------------------------------------------------------------------------------------

interface Stored {
	view: MarketViewDto;
	validUntil: number;
}

const views = new Map<string, Stored>();
/** Views made in this session, newest first; they show in the track record as open. */
const made: MarketJournalView[] = [];

const iso = (ms: number) => new Date(ms).toISOString();

function ttlMs(): number {
	try {
		const raw = new URLSearchParams(location.search).get('mock') ?? sessionStorage.getItem('jarvis.mock.flags') ?? '';
		const f = raw.split(',').find((x) => x.startsWith('viewttl='));
		if (f) return Math.max(1, Number(f.slice(8))) * 1000;
	} catch {
		/* no location or storage: use the default */
	}
	return HORIZON_MIN * MIN;
}

function newsFor(symbol: string, p: Profile, now: number): MarketNews {
	if (!p.news) return { configured: false, score: null, label: null, count: 0, headlines: [] };
	return {
		configured: true,
		score: 0.2,
		label: 'slightly positive',
		count: 7,
		headlines: HEADLINES.map<MarketHeadline>((h, i) => ({
			title: h.title,
			source: h.source,
			url: `https://example.com/news/${symbol.toLowerCase()}-${i + 1}`,
			published_at: iso(now - h.ago * MIN),
			tone: h.tone
		}))
	};
}

function buildView(canonical: string, now = Date.now()): MarketViewDto {
	const s = symInfo(canonical);
	if (!s) throw new ApiError('bad_request', `unknown symbol: ${canonical}`, 400);
	const p = PROFILES[canonical] ?? QUIET;
	const d = s.digits;
	const r = (v: number) => round(v, d);
	const m = mid(canonical);
	const half = (s.spread * s.point) / 2;
	const bid = r(m - half);
	const ask = r(m + half);
	const aH = m * p.atrH1;
	const aM = m * p.atrM15;

	const events: MarketEvent[] = p.events.map((e) => ({
		time: iso(now + e.minutes * MIN),
		minutes_away: e.minutes,
		currency: e.currency,
		title: e.title,
		importance: e.importance
	}));

	const news = newsFor(canonical, p, now);
	const factors: MarketFactor[] = p.factors.map(([name, points, detail]) => ({
		name,
		signal: points > 0 ? 'bullish' : points < 0 ? 'bearish' : 'neutral',
		points,
		detail
	}));
	if (news.configured) {
		const points = Math.round((news.score ?? 0) * 10);
		factors.push({
			name: 'News',
			signal: 'bullish',
			points,
			detail: `Headline tone ${(news.score ?? 0).toFixed(2)} across ${news.count} headlines`
		});
	}
	const score = factors.reduce((a, f) => a + f.points, 0);

	const vwapSide = p.vwapBelow ? -1 : 1;
	const levels: MarketLevels = {
		pivot: r(m + 0.3 * aH),
		r1: r(m + 0.8 * aH),
		r2: r(m + 1.7 * aH),
		s1: r(m - 1.2 * aH),
		s2: r(m - 2.1 * aH),
		prev_day_high: r(m + 1.9 * aH),
		prev_day_low: r(m - 2.4 * aH),
		vwap: r(m + vwapSide * 0.3 * aH),
		support: [r(m - 1.2 * aH), r(m - 2.1 * aH)],
		resistance: [r(m + 1.8 * aH), r(m + 2.7 * aH)]
	};

	let setup: MarketSetup | null = null;
	if (p.setup && p.bias !== 'neutral' && !p.standAside) {
		const buy = p.bias === 'bullish';
		const entryLow = r(buy ? ask - 0.5 * aM : bid);
		const entryHigh = r(buy ? ask : bid + 0.5 * aM);
		const entry = (entryLow + entryHigh) / 2;
		const dir = buy ? 1 : -1;
		const stop = r(entry - dir * aH);
		const tp1 = r(entry + dir * 1.5 * aH);
		setup = {
			side: (buy ? 'buy' : 'sell') as Side,
			entry_low: entryLow,
			entry_high: entryHigh,
			stop_loss: stop,
			take_profit: tp1,
			take_profit_2: r(entry + dir * 2.5 * aH),
			risk_reward: round(Math.abs(tp1 - entry) / Math.abs(entry - stop), 2),
			atr_h1: round(aH, d),
			atr_m15: round(aM, d)
		};
	}

	const timeframes: MarketTimeframe[] = p.frames.map(([timeframe, trend, rsi, hist]) => {
		const scale = timeframe === 'D1' ? 4 : timeframe === 'H4' ? 2 : timeframe === 'H1' ? 1 : timeframe === 'M15' ? 0.4 : 0.2;
		const dir = trend === 'up' ? 1 : trend === 'down' ? -1 : 0;
		return {
			timeframe,
			trend,
			ema20: r(m - dir * 0.2 * aH * scale),
			ema50: r(m - dir * 0.8 * aH * scale),
			rsi,
			macd_hist: hist,
			atr: r(aH * scale)
		};
	});

	const confidence = Math.min(100, Math.abs(score));
	const biasWord = p.bias;
	const top = [...factors].sort((a, b) => Math.abs(b.points) - Math.abs(a.points)).slice(0, 2);
	const summary = p.standAside
		? `${cap(biasWord)} lean (${confidence}/100), but stand aside. ${p.standAside}.`
		: setup
			? `${cap(biasWord)} intraday bias (${confidence}/100): ${top.map((f) => f.name).join(' and ')} lead. ${setup.side === 'buy' ? 'Buy' : 'Sell'} zone ${setup.entry_low} to ${setup.entry_high}, stop ${setup.stop_loss}, first target ${setup.take_profit}.`
			: `${cap(biasWord)} intraday bias (${confidence}/100): the timeframes disagree, so there is no setup.`;

	const ttl = ttlMs();
	const view: MarketViewDto = {
		id: crypto.randomUUID(),
		symbol: canonical,
		broker_symbol: toBroker(canonical),
		generated_at: iso(now),
		valid_until: iso(now + ttl),
		price: { bid, ask },
		bias: p.bias,
		confidence,
		score,
		stand_aside: p.standAside !== null,
		stand_aside_reason: p.standAside,
		setup,
		timeframes,
		levels,
		factors,
		events,
		news,
		summary,
		disclaimer: DISCLAIMER
	};
	views.set(view.id, { view, validUntil: now + ttl });
	return view;
}

const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** The canonical symbol a typed name means (a broker symbol, a canonical name or an alias). */
function canonicalOf(raw: string): string {
	const text = raw.trim();
	const direct = SYMS.find((x) => x.symbol.toLowerCase() === text.toLowerCase())?.symbol;
	const viaBroker = SYMS.find((x) => x.symbol === toCanonical(text))?.symbol;
	const found = direct ?? viaBroker ?? resolveSymbol(text);
	if (!found || !symInfo(found)) throw new ApiError('bad_request', `unknown symbol: ${text}`, 400);
	return found;
}

// ---- Endpoints ---------------------------------------------------------------------------------------------

export function getMarketView(path: string): MarketViewDto {
	const symbol = new URLSearchParams(path.split('?')[1] ?? '').get('symbol');
	if (!symbol?.trim()) throw new ApiError('bad_request', 'symbol is required', 400);
	const view = buildView(canonicalOf(symbol));
	made.unshift({
		id: view.id,
		created_at: view.generated_at,
		symbol: view.symbol,
		broker_symbol: view.broker_symbol,
		bias: view.bias,
		confidence: view.confidence,
		score: view.score,
		stand_aside: view.stand_aside,
		side: view.setup?.side ?? null,
		entry: view.setup ? round((view.setup.entry_low + view.setup.entry_high) / 2, symInfo(view.symbol)!.digits) : null,
		stop: view.setup?.stop_loss ?? null,
		tp: view.setup?.take_profit ?? null,
		price: round((view.price.bid + view.price.ask) / 2, symInfo(view.symbol)!.digits),
		valid_until: view.valid_until,
		outcome: 'open',
		evaluated_at: null,
		bias_correct: null,
		move_atr: null
	});
	return view;
}

export function getMarketCalendar(path: string): MarketCalendarResponse {
	const q = new URLSearchParams(path.split('?')[1] ?? '');
	const hours = Math.min(168, Math.max(1, Number(q.get('hours') ?? 24)));
	const wanted = (q.get('currencies') ?? '')
		.split(',')
		.map((c) => c.trim().toUpperCase())
		.filter(Boolean);
	const now = Date.now();
	const seen = new Set<string>();
	const events: MarketEvent[] = [];
	for (const p of Object.values(PROFILES)) {
		for (const e of p.events) {
			const key = `${e.currency}|${e.title}|${e.minutes}`;
			if (seen.has(key)) continue;
			seen.add(key);
			if (e.minutes > hours * 60 || (wanted.length > 0 && !wanted.includes(e.currency))) continue;
			events.push({
				time: iso(now + e.minutes * MIN),
				minutes_away: e.minutes,
				currency: e.currency,
				title: e.title,
				importance: e.importance
			});
		}
	}
	events.sort((a, b) => a.time.localeCompare(b.time));
	return { events };
}

export function getMarketNews(path: string): MarketNewsResponse {
	const symbol = new URLSearchParams(path.split('?')[1] ?? '').get('symbol');
	if (!symbol?.trim()) throw new ApiError('bad_request', 'symbol is required', 400);
	const canonical = canonicalOf(symbol);
	const p = PROFILES[canonical] ?? QUIET;
	if (!p.news) return { configured: false, provider: null, summary: null };
	const n = newsFor(canonical, p, Date.now());
	return {
		configured: true,
		provider: 'finnhub',
		summary: { score: n.score ?? 0, label: n.label ?? 'neutral', count: n.count, headlines: n.headlines }
	};
}

// ---- Journal -------------------------------------------------------------------------------------------------

type Seed = [outcome: MarketOutcome, biasRight: boolean, symbol: string, bias: Bias];

/** Twenty past views, newest first: 8 TP, 4 SL, 2 expired, 6 without a setup; bias right 12 of 20. */
const HISTORY: Seed[] = [
	['tp_hit', true, 'XAUUSD', 'bullish'],
	['no_setup', true, 'US100', 'neutral'],
	['sl_hit', false, 'EURUSD', 'bearish'],
	['tp_hit', true, 'XAUUSD', 'bullish'],
	['tp_hit', true, 'XAUUSD', 'bullish'],
	['no_setup', false, 'EURUSD', 'bearish'],
	['expired', false, 'US100', 'bullish'],
	['sl_hit', false, 'XAUUSD', 'bullish'],
	['tp_hit', true, 'EURUSD', 'bearish'],
	['no_setup', true, 'US100', 'neutral'],
	['tp_hit', true, 'XAUUSD', 'bullish'],
	['sl_hit', true, 'US100', 'bearish'],
	['no_setup', false, 'XAUUSD', 'neutral'],
	['tp_hit', true, 'EURUSD', 'bearish'],
	['expired', false, 'XAUUSD', 'bullish'],
	['no_setup', true, 'US100', 'neutral'],
	['tp_hit', true, 'XAUUSD', 'bullish'],
	['sl_hit', false, 'EURUSD', 'bearish'],
	['no_setup', false, 'US100', 'neutral'],
	['tp_hit', true, 'XAUUSD', 'bullish']
];

function history(): MarketJournalView[] {
	const now = Date.now();
	return HISTORY.map(([outcome, right, symbol, bias], i) => {
		const s = symInfo(symbol)!;
		const d = s.digits;
		const base = s.mid * (1 + ((i % 5) - 2) * 0.002);
		const aH = base * (PROFILES[symbol]?.atrH1 ?? 0.004);
		const created = now - (5 + i * 7) * 60 * MIN;
		const hasSetup = outcome !== 'no_setup';
		const buy = bias === 'bullish';
		const dir = buy ? 1 : -1;
		return {
			id: `00000000-0000-4000-8000-${String(i + 1).padStart(12, '0')}`,
			created_at: iso(created),
			symbol,
			broker_symbol: toBroker(symbol),
			bias,
			confidence: bias === 'neutral' ? 4 + (i % 9) : 40 + ((i * 7) % 35),
			stand_aside: !hasSetup && bias !== 'neutral',
			side: hasSetup ? (buy ? 'buy' : 'sell') : null,
			entry: hasSetup ? round(base, d) : null,
			stop: hasSetup ? round(base - dir * aH, d) : null,
			tp: hasSetup ? round(base + dir * 1.5 * aH, d) : null,
			price: round(base, d),
			valid_until: iso(created + HORIZON_MIN * MIN),
			outcome,
			evaluated_at: iso(created + HORIZON_MIN * MIN + 2 * MIN),
			bias_correct: right,
			move_atr: round((right ? 1 : -1) * (0.3 + (i % 4) * 0.35), 2)
		};
	});
}

function statsOf(list: MarketJournalView[]): MarketJournalStats {
	const count = (o: MarketOutcome) => list.filter((v) => v.outcome === o).length;
	return {
		total: list.length,
		open: count('open'),
		tp_hit: count('tp_hit'),
		sl_hit: count('sl_hit'),
		expired: count('expired'),
		no_setup: count('no_setup'),
		bias_correct: list.filter((v) => v.bias_correct === true).length,
		bias_wrong: list.filter((v) => v.bias_correct === false).length
	};
}

export function getMarketJournal(path: string): MarketJournalResponse {
	const q = new URLSearchParams(path.split('?')[1] ?? '');
	const limit = Math.min(200, Math.max(1, Number(q.get('limit') ?? 20)));
	const sym = q.get('symbol')?.trim();
	let all = [...made, ...history()];
	if (sym) {
		const canonical = canonicalOf(sym);
		all = all.filter((v) => v.symbol === canonical);
	}
	return { stats: statsOf(all), views: all.slice(0, limit) };
}

// ---- Prepare -----------------------------------------------------------------------------------------------------

/**
 * A view becomes the normal preview and nothing more: the limits apply, the kill switch applies,
 * and the confirmation modal decides.
 */
export function prepareFromView(body: unknown): PreviewDto {
	const b = (body ?? {}) as { view_id?: unknown; volume?: unknown; risk_percent?: unknown; order_type?: unknown };
	const stored = typeof b.view_id === 'string' ? views.get(b.view_id) : undefined;
	if (!stored) throw new ApiError('bad_request', 'that view is gone; analyze again', 404);
	const hasVolume = b.volume !== undefined;
	const hasRisk = b.risk_percent !== undefined;
	if (hasVolume && hasRisk) throw new ApiError('bad_request', 'give either volume or risk_percent, not both', 400);
	if (hasRisk) {
		if (typeof b.risk_percent !== 'number' || !Number.isFinite(b.risk_percent) || b.risk_percent <= 0)
			throw new ApiError('bad_request', 'risk_percent must be greater than 0', 400);
	} else if (typeof b.volume !== 'number' || !Number.isFinite(b.volume) || b.volume <= 0)
		throw new ApiError('bad_request', 'volume (or risk_percent) is required and must be greater than 0', 400);
	if (b.order_type !== 'market' && b.order_type !== 'limit')
		throw new ApiError('bad_request', 'order_type must be market or limit', 400);
	const { view, validUntil } = stored;
	if (!view.setup) throw new ApiError('gate_rejected', 'this view has no setup to trade', 422);
	if (validUntil <= Date.now()) throw new ApiError('gate_rejected', 'this view has expired; analyze again', 422);
	assertWritable();

	const s = view.setup;
	const size = hasRisk ? { risk_percent: b.risk_percent as number } : { volume: b.volume as number };
	const legs = { symbol: view.symbol, side: s.side, ...size, sl: s.stop_loss, tp: s.take_profit };
	if (b.order_type === 'market') return openPreview(legs, false);
	return limitPreview({ ...legs, price: s.side === 'buy' ? s.entry_low : s.entry_high });
}
