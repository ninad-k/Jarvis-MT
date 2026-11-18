/**
 * Pure helpers for price alerts: which way an alert points, how a price is worded and checked,
 * and the lines shown, announced and spoken when one fires. No DOM, no Svelte.
 */
import type { AlertDirection, AlertTriggeredEvent, PriceAlert } from './api';
import { decimalsOf, fmtPrice } from './format';

/** The server allows 50 active alerts per user. */
export const MAX_ACTIVE_ALERTS = 50;
export const MAX_NOTE = 120;

/**
 * Which way a target points from the live price: above the bid means "tell me when it rises to
 * it". A target equal to the bid counts as `below`, because an "above" alert fires at once when
 * bid >= price while a "below" one waits for the ask to come down to it.
 */
export function inferDirection(target: number, bid: number): AlertDirection {
	return target > bid ? 'above' : 'below';
}

/** The server's rule: `above` fires when bid >= price, `below` when ask <= price. True means it would fire at once. */
export function alertReached(direction: AlertDirection, target: number, bid: number, ask: number): boolean {
	return direction === 'above' ? bid >= target : ask <= target;
}

export const directionWord = (d: AlertDirection): string => (d === 'above' ? 'above' : 'below');

/** Text under the direction control: what will actually happen. */
export function directionHint(direction: AlertDirection, auto: boolean): string {
	const how =
		direction === 'above'
			? 'Fires when the bid reaches this price or higher.'
			: 'Fires when the ask reaches this price or lower.';
	return auto ? `${how} Chosen from the current price.` : how;
}

export type PriceCheck = { ok: true; value: number } | { ok: false; error: string };

/** The price typed in the alert form. Required and never pre-filled. */
export function validateAlertPrice(raw: string): PriceCheck {
	const text = raw.trim();
	if (text === '') return { ok: false, error: 'Enter the price to be alerted at.' };
	if (text.startsWith('-')) return { ok: false, error: 'The price must be greater than 0.' };
	if (!/^(\d+\.?\d*|\.\d+)$/.test(text)) return { ok: false, error: 'Use digits and a decimal point only, for example 3950.50.' };
	const decimals = text.includes('.') ? text.split('.')[1].length : 0;
	if (decimals > 8) return { ok: false, error: 'Use at most 8 decimals.' };
	const value = Number(text);
	if (!Number.isFinite(value) || value <= 0) return { ok: false, error: 'The price must be greater than 0.' };
	return { ok: true, value };
}

/** Trim the note; null when empty. Too long is an error the form shows. */
export function cleanNote(raw: string): { note: string | null; error: string | null } {
	const note = raw.trim().replace(/\s+/g, ' ');
	if (note.length > MAX_NOTE) return { note: null, error: `Keep the note under ${MAX_NOTE} characters.` };
	return { note: note === '' ? null : note, error: null };
}

/** Decimals for a price when the watchlist has none for the symbol: what the number itself needs. */
export function priceDigits(price: number, hint?: number | null): number {
	if (typeof hint === 'number' && hint >= 0) return hint;
	return Math.max(decimalsOf(price), price >= 100 ? 2 : 4);
}

export const fmtAlertPrice = (price: number, hint?: number | null): string => fmtPrice(price, priceDigits(price, hint));

// ---- Names -----------------------------------------------------------------------------------------

/** How a symbol is said aloud; any other symbol is read as written. */
const SPOKEN: Record<string, string> = {
	XAUUSD: 'Gold',
	XAGUSD: 'Silver',
	BTCUSD: 'Bitcoin',
	ETHUSD: 'Ethereum',
	US100: 'Nasdaq',
	US30: 'Dow',
	US500: 'S and P 500',
	USOIL: 'Oil',
	EURUSD: 'Euro dollar',
	GBPUSD: 'Pound dollar',
	USDJPY: 'Dollar yen'
};

export function spokenSymbol(symbol: string): string {
	const key = symbol.trim().toUpperCase();
	return SPOKEN[key] ?? symbol;
}

/** A price said aloud: no thousands separators, no trailing zeros ("3950", "1.0845"). */
export function spokenPrice(price: number): string {
	return String(Number(price.toFixed(8)));
}

// ---- Lines ---------------------------------------------------------------------------------------------

/** "Gold above 3,950.00". */
export function alertTitle(a: Pick<PriceAlert, 'symbol' | 'direction' | 'price'>, digits?: number | null): string {
	return `${spokenSymbol(a.symbol)} ${directionWord(a.direction)} ${fmtAlertPrice(a.price, digits)}`;
}

/** "Gold reached 3,950.00" for the toast. */
export function triggeredLine(e: Pick<AlertTriggeredEvent, 'symbol' | 'price'>, digits?: number | null): string {
	return `${spokenSymbol(e.symbol)} reached ${fmtAlertPrice(e.price, digits)}`;
}

/** What is read aloud: "Gold reached 3950". */
export function spokenLine(e: Pick<AlertTriggeredEvent, 'symbol' | 'price'>): string {
	return `${spokenSymbol(e.symbol)} reached ${spokenPrice(e.price)}`;
}

/** The toast: the line plus the user's own note. */
export function toastText(e: AlertTriggeredEvent, digits?: number | null): string {
	const line = triggeredLine(e, digits);
	return e.note ? `${line}. ${e.note}` : line;
}

/** "Triggered at 3,950.12" (the price the watcher saw), or empty when the server did not send one. */
export function triggeredAtText(a: Pick<PriceAlert, 'trigger_price'>, digits?: number | null): string {
	return a.trigger_price == null ? '' : `Triggered at ${fmtAlertPrice(a.trigger_price, digits)}`;
}

/** Newest first, whatever order the server used. */
export function newestFirst(list: readonly PriceAlert[]): PriceAlert[] {
	const t = (a: PriceAlert) => Date.parse(a.triggered_at ?? a.created_at) || 0;
	return [...list].sort((a, b) => t(b) - t(a));
}

/** Active alerts that belong to a symbol (the watchlist bell shows a count). */
export function activeFor(list: readonly PriceAlert[], symbol: string): PriceAlert[] {
	return list.filter((a) => a.symbol === symbol || a.broker_symbol === symbol);
}
