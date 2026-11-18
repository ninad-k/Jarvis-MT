/**
 * Pure helpers for the Market view panel: wording, tone classes, number and time formatting,
 * the track-record percentages and the volume check for "Prepare trade". No DOM, no Svelte.
 */
import type {
	EventImportance,
	MarketHeadline,
	MarketJournalStats,
	MarketNews,
	MarketOutcome,
	MarketSetup,
	MarketViewDto,
	PreviewSizing
} from './api';
import { decimalsOf, fmtPrice, fmtVolume } from './format';

export const DISCLAIMER =
	'Computed from price data, indicators, the economic calendar and headlines. It is not financial advice and no outcome is guaranteed.';

// ---- Bias and confidence ------------------------------------------------------------------

export type ViewTone = 'up' | 'down' | 'flat';

export interface BiasInfo {
	label: string;
	tone: ViewTone;
	/** Classes for the pill (full strings so Tailwind keeps them). */
	pill: string;
	/** Text colour for the arrow or number next to it. */
	text: string;
	/** Fill colour of the confidence meter. */
	bar: string;
}

export function biasInfo(bias: string): BiasInfo {
	if (bias === 'bullish')
		return { label: 'Bullish', tone: 'up', pill: 'border-up/70 bg-up/10 text-up', text: 'text-up', bar: 'bg-up' };
	if (bias === 'bearish')
		return { label: 'Bearish', tone: 'down', pill: 'border-down/70 bg-down/10 text-down', text: 'text-down', bar: 'bg-down' };
	return { label: 'Neutral', tone: 'flat', pill: 'border-line-hi bg-raise/50 text-mute', text: 'text-mute', bar: 'bg-mute' };
}

/** Arrow for a bias or a trend; always paired with text for screen readers. */
export function trendArrow(trend: string): string {
	if (trend === 'up' || trend === 'bullish') return '↑';
	if (trend === 'down' || trend === 'bearish') return '↓';
	return '→';
}

export function trendWord(trend: string): string {
	if (trend === 'up' || trend === 'bullish') return 'up';
	if (trend === 'down' || trend === 'bearish') return 'down';
	return 'flat';
}

export function toneClass(tone: ViewTone): string {
	return tone === 'up' ? 'text-up' : tone === 'down' ? 'text-down' : 'text-mute';
}

/** Confidence kept to the 0..100 the meter can draw. */
export function clampConfidence(v: number): number {
	return Number.isFinite(v) ? Math.min(100, Math.max(0, Math.round(v))) : 0;
}

export function confidenceWord(v: number): string {
	const c = clampConfidence(v);
	if (c >= 70) return 'high';
	if (c >= 50) return 'moderate';
	if (c >= 30) return 'low';
	return 'very low';
}

/** "Bullish, moderate confidence (62 of 100)". */
export function biasSentence(bias: string, confidence: number): string {
	const c = clampConfidence(confidence);
	return `${biasInfo(bias).label}, ${confidenceWord(c)} confidence (${c} of 100)`;
}

/** Indicator reading for the timeframe table. */
export function rsiTone(rsi: number): ViewTone | 'warn' {
	if (rsi >= 70 || rsi <= 30) return 'warn';
	return 'flat';
}

export function macdSign(hist: number): { text: string; tone: ViewTone; word: string } {
	if (hist > 0) return { text: '+', tone: 'up', word: 'positive' };
	if (hist < 0) return { text: '−', tone: 'down', word: 'negative' };
	return { text: '0', tone: 'flat', word: 'flat' };
}

/** "+25" / "−10" / "0"; halves are kept ("+12.5"). */
export function fmtPoints(v: number): string {
	if (!Number.isFinite(v) || v === 0) return '0';
	const abs = Math.abs(v);
	const s = Number.isInteger(abs) ? String(abs) : abs.toFixed(1);
	return `${v > 0 ? '+' : '−'}${s}`;
}

// ---- Prices ---------------------------------------------------------------------------------

function priceValues(v: MarketViewDto): number[] {
	const out: number[] = [v.price.bid, v.price.ask];
	const s = v.setup;
	if (s) out.push(s.entry_low, s.entry_high, s.stop_loss, s.take_profit);
	if (s?.take_profit_2 != null) out.push(s.take_profit_2);
	const l = v.levels;
	for (const x of [l.pivot, l.r1, l.r2, l.s1, l.s2, l.prev_day_high, l.prev_day_low, l.vwap]) {
		if (x != null) out.push(x);
	}
	out.push(...l.support, ...l.resistance);
	return out.filter((n) => Number.isFinite(n));
}

/**
 * Decimals to show prices with. The view carries `digits` when the server sends it; otherwise they
 * come from the numbers themselves (the server rounds every price to the symbol's digits, so the
 * longest one tells), never fewer than a price of that size usually has. `hint` is the watchlist's
 * digits for the same instrument.
 */
export function viewDigits(v: MarketViewDto, hint?: number | null): number {
	if (typeof v.digits === 'number' && v.digits >= 0) return v.digits;
	if (typeof hint === 'number' && hint >= 0) return hint;
	const longest = Math.max(0, ...priceValues(v).map(decimalsOf));
	const floor = v.price.bid >= 100 ? 2 : 4;
	return Math.max(longest, floor);
}

export function fmtLevel(v: number | null | undefined, digits: number): string {
	return v == null || !Number.isFinite(v) ? '—' : fmtPrice(v, digits);
}

/** "Buy at the ask, 3,925.22" style pieces for the Prepare dialog. */
export interface PrepareChoice {
	type: 'market' | 'limit';
	label: string;
	price: number;
	/** Where the price comes from, in words. */
	basis: string;
}

export function prepareChoices(setup: MarketSetup, price: { bid: number; ask: number }): PrepareChoice[] {
	const buy = setup.side === 'buy';
	return [
		{
			type: 'market',
			label: 'Market',
			price: buy ? price.ask : price.bid,
			basis: buy ? 'at the current ask' : 'at the current bid'
		},
		{
			type: 'limit',
			label: 'Limit',
			price: buy ? setup.entry_low : setup.entry_high,
			basis: buy ? 'at the low end of the entry zone' : 'at the high end of the entry zone'
		}
	];
}

/** "Buy · stop 3,911.00 · TP1 3,944.10". */
export function setupLine(setup: MarketSetup, digits: number): string {
	const side = setup.side === 'buy' ? 'Buy' : 'Sell';
	return `${side} · stop ${fmtPrice(setup.stop_loss, digits)} · TP1 ${fmtPrice(setup.take_profit, digits)}`;
}

// ---- Time -------------------------------------------------------------------------------------

function parseMs(iso: string | null | undefined): number | null {
	if (!iso) return null;
	const t = Date.parse(iso);
	return Number.isNaN(t) ? null : t;
}

export function isExpired(validUntil: string, now: number): boolean {
	const t = parseMs(validUntil);
	return t !== null && t <= now;
}

/** "3 h 12 min", "45 min", "1 h" (whole minutes, at least 1). */
function span(minutes: number): string {
	const m = Math.max(1, Math.round(minutes));
	if (m < 60) return `${m} min`;
	const h = Math.floor(m / 60);
	const r = m % 60;
	return r === 0 ? `${h} h` : `${h} h ${r} min`;
}

/** "expires in 3 h 12 min" or "expired 5 min ago". */
export function expiryText(validUntil: string, now: number): string {
	const t = parseMs(validUntil);
	if (t === null) return 'validity unknown';
	const diff = t - now;
	if (diff <= 0) return diff > -60_000 ? 'expired just now' : `expired ${span(-diff / 60_000)} ago`;
	return `expires in ${span(diff / 60_000)}`;
}

/** "in 40 min", "in 2 h 5 min", "now", "12 min ago". */
export function minutesAwayText(minutes: number): string {
	if (!Number.isFinite(minutes)) return '—';
	const m = Math.round(minutes);
	if (m === 0) return 'now';
	return m > 0 ? `in ${span(m)}` : `${span(-m)} ago`;
}

/** Minutes until the event, from its own time when it parses (the view was made a while ago). */
export function eventMinutes(ev: { time: string; minutes_away: number }, now: number): number {
	const t = parseMs(ev.time);
	return t === null ? ev.minutes_away : Math.round((t - now) / 60_000);
}

/** "just now", "35 min ago", "2 h ago", "3 d ago". */
export function timeAgo(iso: string, now: number): string {
	const t = parseMs(iso);
	if (t === null) return '';
	const s = Math.max(0, Math.round((now - t) / 1000));
	if (s < 60) return 'just now';
	const m = Math.floor(s / 60);
	if (m < 60) return `${m} min ago`;
	const h = Math.floor(m / 60);
	return h < 24 ? `${h} h ago` : `${Math.floor(h / 24)} d ago`;
}

/** "13:50" in the browser's time zone. */
export function clockTime(iso: string): string {
	const t = parseMs(iso);
	return t === null ? '' : new Date(t).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
}

export function dayTime(iso: string): string {
	const t = parseMs(iso);
	if (t === null) return '';
	return new Date(t).toLocaleString('en-GB', { day: '2-digit', month: 'short', hour: '2-digit', minute: '2-digit' });
}

export const IMPORTANCE: Record<EventImportance, { label: string; cls: string }> = {
	high: { label: 'High', cls: 'border-down/70 text-down' },
	moderate: { label: 'Medium', cls: 'border-warn/70 text-warn' },
	low: { label: 'Low', cls: 'border-line-hi text-mute' }
};

export function importanceInfo(i: string): { label: string; cls: string } {
	return (IMPORTANCE as Record<string, { label: string; cls: string } | undefined>)[i] ?? { label: i, cls: 'border-line-hi text-mute' };
}

// ---- News -------------------------------------------------------------------------------------

export function newsTone(score: number | null | undefined): ViewTone {
	if (score == null || !Number.isFinite(score)) return 'flat';
	return score >= 0.1 ? 'up' : score <= -0.1 ? 'down' : 'flat';
}

/** "Slightly positive (7 headlines)", "No relevant headlines" or "Not configured". */
export function newsLabel(news: Pick<MarketNews, 'configured' | 'label' | 'count'>): string {
	if (!news.configured) return 'Not configured';
	if (news.count <= 0) return 'No relevant headlines';
	const label = (news.label ?? 'neutral').trim() || 'neutral';
	const cap = label.charAt(0).toUpperCase() + label.slice(1);
	return `${cap} (${news.count} headline${news.count === 1 ? '' : 's'})`;
}

/** Only http(s) links are rendered as links: a feed must not be able to inject another scheme. */
export function safeHttpUrl(url: string): string | null {
	try {
		const u = new URL(url);
		return u.protocol === 'https:' || u.protocol === 'http:' ? u.href : null;
	} catch {
		return null;
	}
}

/** Up to `max` headlines, newest first. */
export function topHeadlines(list: readonly MarketHeadline[], max = 5): MarketHeadline[] {
	return [...list]
		.sort((a, b) => (parseMs(b.published_at) ?? 0) - (parseMs(a.published_at) ?? 0))
		.slice(0, max);
}

// ---- Journal -------------------------------------------------------------------------------------

export interface OutcomeInfo {
	label: string;
	cls: string;
}

const OUTCOMES: Record<MarketOutcome, OutcomeInfo> = {
	open: { label: 'Open', cls: 'border-accent/60 text-accent' },
	tp_hit: { label: 'TP hit', cls: 'border-up/70 text-up' },
	sl_hit: { label: 'SL hit', cls: 'border-down/70 text-down' },
	expired: { label: 'Expired', cls: 'border-warn/60 text-warn' },
	no_setup: { label: 'No setup', cls: 'border-line-hi text-mute' }
};

export function outcomeInfo(outcome: string): OutcomeInfo {
	return (OUTCOMES as Record<string, OutcomeInfo | undefined>)[outcome] ?? { label: outcome, cls: 'border-line-hi text-mute' };
}

/** What the bias check says, in words ("Right", "Wrong", "Not scored yet"). */
export function biasVerdict(correct: boolean | null | undefined): string {
	return correct === true ? 'Bias right' : correct === false ? 'Bias wrong' : 'Not scored yet';
}

/** A share as a whole percent, or null when there is nothing to divide by. */
export function percent(part: number, whole: number): number | null {
	if (!Number.isFinite(part) || !Number.isFinite(whole) || whole <= 0) return null;
	return Math.round((part / whole) * 100);
}

export function fmtPercent(p: number | null): string {
	return p === null ? '—' : `${p}%`;
}

export interface TrackSummary {
	/** Views that had a setup (all outcomes but "no setup"). */
	setups: number;
	/** Setups that have an answer: TP hit, SL hit or expired. */
	settled: number;
	tpHit: number;
	slHit: number;
	expired: number;
	open: number;
	noSetup: number;
	tpRate: number | null;
	slRate: number | null;
	expiredRate: number | null;
	biasScored: number;
	biasCorrect: number;
	biasAccuracy: number | null;
}

const n = (v: number | undefined): number => (typeof v === 'number' && Number.isFinite(v) && v > 0 ? Math.floor(v) : 0);

/**
 * Percentages for the track record. Rates are shares of the settled setups (open ones are not
 * judged yet); bias accuracy is the share of scored views whose bias was right.
 */
export function summarizeStats(stats: MarketJournalStats | null | undefined): TrackSummary {
	const s = stats ?? {};
	const tpHit = n(s.tp_hit);
	const slHit = n(s.sl_hit);
	const expired = n(s.expired);
	const open = n(s.open);
	const settled = tpHit + slHit + expired;
	const biasCorrect = n(s.bias_correct);
	const biasScored = biasCorrect + n(s.bias_wrong);
	return {
		setups: settled + open,
		settled,
		tpHit,
		slHit,
		expired,
		open,
		noSetup: n(s.no_setup),
		tpRate: percent(tpHit, settled),
		slRate: percent(slHit, settled),
		expiredRate: percent(expired, settled),
		biasScored,
		biasCorrect,
		biasAccuracy: percent(biasCorrect, biasScored)
	};
}

// ---- Prepare trade --------------------------------------------------------------------------------

export type VolumeCheck = { ok: true; value: number } | { ok: false; error: string };

/**
 * The lot size typed in the Prepare dialog. There is no default: an empty box is an error, and
 * the gate still has the last word on how large it may be.
 */
export function validateVolume(raw: string): VolumeCheck {
	const text = raw.trim();
	if (text === '') return { ok: false, error: 'Enter a volume in lots.' };
	if (text.startsWith('-')) return { ok: false, error: 'Volume must be greater than 0.' };
	if (!/^(\d+\.?\d*|\.\d+)$/.test(text)) return { ok: false, error: 'Use digits only, for example 0.01.' };
	const decimals = text.includes('.') ? text.split('.')[1].length : 0;
	if (decimals > 2) return { ok: false, error: 'Use at most 2 decimals.' };
	const value = Number(text);
	if (!Number.isFinite(value) || value <= 0) return { ok: false, error: 'Volume must be greater than 0.' };
	return { ok: true, value };
}

// ---- Risk-based size -----------------------------------------------------------------------------------

/** The smallest risk the server accepts, and the ceiling when /api/status carries no `max_risk_percent`. */
export const MIN_RISK_PERCENT = 0.1;
export const DEFAULT_MAX_RISK_PERCENT = 2;

/** The largest risk % to offer: the server's limit when it sent one, else 2. */
export function riskCeiling(limits: { max_risk_percent?: number } | null | undefined): number {
	const v = limits?.max_risk_percent;
	return typeof v === 'number' && Number.isFinite(v) && v >= MIN_RISK_PERCENT ? v : DEFAULT_MAX_RISK_PERCENT;
}

export type RiskCheck = { ok: true; value: number } | { ok: false; error: string };

/** "1", "0.5", "1.25": a percentage without trailing zeros. */
export function fmtRisk(percent: number): string {
	return String(Number(percent.toFixed(2)));
}

/**
 * The risk % typed in the Prepare dialog. Like the lot size there is no default: an empty box is
 * an error. The server still applies its own limit.
 */
export function validateRiskPercent(raw: string, max: number): RiskCheck {
	const text = raw.trim();
	if (text === '') return { ok: false, error: 'Enter how much of your equity to risk, in percent.' };
	if (text.startsWith('-')) return { ok: false, error: `Risk must be at least ${fmtRisk(MIN_RISK_PERCENT)}%.` };
	if (!/^(\d+\.?\d*|\.\d+)$/.test(text)) return { ok: false, error: 'Use digits only, for example 1 or 0.5.' };
	const decimals = text.includes('.') ? text.split('.')[1].length : 0;
	if (decimals > 2) return { ok: false, error: 'Use at most 2 decimals.' };
	const value = Number(text);
	if (!Number.isFinite(value) || value < MIN_RISK_PERCENT)
		return { ok: false, error: `Risk must be at least ${fmtRisk(MIN_RISK_PERCENT)}%.` };
	if (value > max) return { ok: false, error: `Risk is limited to ${fmtRisk(max)}% of equity.` };
	return { ok: true, value };
}

/** "1% risk = 39.20 USD → 0.05 lots". */
export function sizingLine(s: PreviewSizing): string {
	const amount = fmtPrice(s.risk_amount, 2);
	const ccy = s.currency ? ` ${s.currency}` : '';
	return `${fmtRisk(s.risk_percent)}% risk = ${amount}${ccy} → ${fmtVolume(s.volume)} lots`;
}

/** Second line under the sizing: the distance the stop sits from the entry. */
export function sizingDetail(s: PreviewSizing, digits?: number | null): string {
	const d = typeof digits === 'number' && digits >= 0 ? digits : Math.max(decimalsOf(s.stop_distance), s.stop_distance >= 10 ? 2 : 4);
	return `Stop distance ${fmtPrice(s.stop_distance, d)}`;
}

/** A setup can be prepared while the view is current. */
export function canPrepare(view: Pick<MarketViewDto, 'setup' | 'valid_until' | 'stand_aside'>, now: number): boolean {
	return view.setup !== null && !view.stand_aside && !isExpired(view.valid_until, now);
}

// ---- Symbol input -----------------------------------------------------------------------------------

/** The symbol box: trimmed, spaces collapsed, 1 to 32 characters; null when empty. */
export function cleanSymbolInput(raw: string): string | null {
	const s = raw.trim().replace(/\s+/g, ' ');
	return s === '' ? null : s.slice(0, 32);
}

// ---- Chat tool labels -----------------------------------------------------------------------------------

const TOOL_LABELS: Record<string, string> = {
	get_market_view: 'Market view',
	get_economic_calendar: 'Economic calendar',
	get_news: 'News',
	get_briefing: 'Briefing',
	get_symbol_info: 'Symbol info',
	update_watchlist: 'Update watchlist',
	set_persona: 'Switch persona',
	set_model: 'Switch model',
	set_alias: 'Add spoken name',
	remove_alias: 'Remove spoken name',
	engage_kill_switch: 'Engage kill switch',
	tighten_limit: 'Tighten limit',
	create_price_alert: 'Create price alert',
	list_price_alerts: 'List price alerts',
	cancel_price_alert: 'Cancel price alert'
};

/**
 * Tools that change the user's own preferences or make trading stricter. After one of these the
 * HUD re-reads settings, status, the watchlist and the alerts, even if the push event was missed.
 */
export const PREFERENCE_TOOLS: ReadonlySet<string> = new Set([
	'update_watchlist',
	'set_persona',
	'set_model',
	'set_alias',
	'remove_alias',
	'engage_kill_switch',
	'tighten_limit',
	'create_price_alert',
	'cancel_price_alert'
]);

/** True when at least one successful call in a reply was a preference tool. */
export function changedPreferences(calls: ReadonlyArray<{ name: string; ok: boolean }>): boolean {
	return calls.some((c) => c.ok && PREFERENCE_TOOLS.has(c.name));
}

/** Friendly chip text for the market and assistant tools; any other tool keeps its own name. */
export function toolLabel(name: string): string {
	return TOOL_LABELS[name] ?? name;
}
