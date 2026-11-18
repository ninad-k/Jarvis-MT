/**
 * Pure helpers for the daily briefing card: the account line, the events of the day (in UTC, as
 * the server picks them), one line per symbol view, and the text for screen readers.
 */
import type { Account, BriefingDto, BriefingView, MarketEvent, Position } from './api';
import { fmtMoney, fmtSigned } from './format';
import { biasSentence, minutesAwayText } from './market';

/** "Equity 10,230.40 USD · balance 10,000.00 · free margin 10,000.00". */
export function accountLine(a: Pick<Account, 'currency' | 'equity' | 'balance' | 'free_margin'>): string {
	return `Equity ${fmtMoney(a.equity)} ${a.currency} · balance ${fmtMoney(a.balance)} · free margin ${fmtMoney(a.free_margin)}`;
}

/** Sum of the open positions' floating P/L. */
export function floatingTotal(positions: ReadonlyArray<Pick<Position, 'profit'>>): number {
	return positions.reduce((sum, p) => sum + (Number.isFinite(p.profit) ? p.profit : 0), 0);
}

/** "5 open positions" / "No open positions". */
export function positionsHeading(count: number): string {
	return count === 0 ? 'No open positions' : `${count} open position${count === 1 ? '' : 's'}`;
}

/** "13:30 UTC": the server picks "today" in UTC, so the card shows UTC too. */
export function utcTime(iso: string): string {
	const t = Date.parse(iso);
	if (Number.isNaN(t)) return '';
	const d = new Date(t);
	const hh = String(d.getUTCHours()).padStart(2, '0');
	const mm = String(d.getUTCMinutes()).padStart(2, '0');
	return `${hh}:${mm} UTC`;
}

/** Events in time order. */
export function sortedEvents(events: readonly MarketEvent[]): MarketEvent[] {
	const t = (e: MarketEvent) => Date.parse(e.time) || 0;
	return [...events].sort((a, b) => t(a) - t(b));
}

/** "in 2 h 5 min" from the event's own time when it parses, else from the server's figure. */
export function eventWhen(e: Pick<MarketEvent, 'time' | 'minutes_away'>, now: number): string {
	const t = Date.parse(e.time);
	const minutes = Number.isNaN(t) ? e.minutes_away : Math.round((t - now) / 60_000);
	return minutesAwayText(minutes);
}

/** "actual 3.1% · forecast 2.9% · previous 2.7%", or empty when the figures are not out yet. */
export function eventFigures(e: Pick<MarketEvent, 'actual' | 'forecast' | 'previous' | 'unit'>): string {
	const unit = e.unit ? (e.unit.length <= 3 ? e.unit : ` ${e.unit}`) : '';
	const part = (label: string, v: number | null | undefined) =>
		v == null || !Number.isFinite(v) ? null : `${label} ${v}${unit}`;
	return [part('actual', e.actual), part('forecast', e.forecast), part('previous', e.previous)]
		.filter((x): x is string => x !== null)
		.join(' · ');
}

export const highImpactCount = (events: readonly Pick<MarketEvent, 'importance'>[]): number =>
	events.filter((e) => e.importance === 'high').length;

/** The view could not be made (the 20 s budget ran out, the bridge failed). */
export const isUnavailable = (v: Pick<BriefingView, 'unavailable'>): boolean =>
	typeof v.unavailable === 'string' && v.unavailable.trim() !== '';

/** "Bullish, moderate confidence (62 of 100)", "Stand aside: ..." or "Unavailable: ...". */
export function viewLine(v: BriefingView): string {
	if (isUnavailable(v)) return `Unavailable: ${v.unavailable}`;
	if (v.stand_aside) return `Stand aside: ${v.stand_aside_reason ?? 'conditions are not suitable right now'}`;
	return biasSentence(v.bias, v.confidence);
}

/** One sentence for the live region once the briefing has loaded. */
export function briefingAnnouncement(b: BriefingDto): string {
	const ok = b.views.filter((v) => !isUnavailable(v)).length;
	return (
		`Briefing ready. ${positionsHeading(b.positions.length)}, ` +
		`${b.events.length} event${b.events.length === 1 ? '' : 's'} today, ` +
		`${ok} of ${b.views.length} symbol view${b.views.length === 1 ? '' : 's'}.`
	);
}

/** Floating P/L with its currency, for the positions header: "+230.40 USD". */
export function plText(total: number, currency: string): string {
	return `${fmtSigned(total)} ${currency}`.trim();
}
