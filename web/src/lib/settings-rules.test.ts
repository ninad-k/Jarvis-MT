import { describe, expect, it } from 'vitest';
import type { Limits } from './api';
import {
	MAX_ALIASES,
	MAX_WATCHLIST,
	addSymbol,
	aliasesToRows,
	describeHours,
	formToOverrides,
	hasOverrides,
	limitsBody,
	limitErrorField,
	limitsToForm,
	moveItem,
	normalizeSpoken,
	parseHHMM,
	rowsToAliases,
	symbolError,
	validateAliasRows,
	validateLimitsForm,
	watchlistError
} from './settings-rules';

const CONFIG: Limits = {
	max_lot: 1,
	max_positions: 5,
	max_daily_loss: 500,
	allowed_symbols: ['XAUUSD', 'EURUSD', 'GBPUSD'],
	trading_hours: { days: ['mon', 'tue', 'wed', 'thu', 'fri'], start: '00:05', end: '23:55' }
};

describe('symbols and the watchlist', () => {
	it('accepts broker symbols and refuses the rest', () => {
		expect(symbolError('XAUUSD')).toBeNull();
		expect(symbolError('US30.cash')).toBeNull();
		expect(symbolError('EURUSD#')).toBeNull();
		expect(symbolError('')).toMatch(/Enter/);
		expect(symbolError('has space')).toMatch(/no spaces/);
		expect(symbolError('a'.repeat(33))).toMatch(/32/);
		expect(symbolError('<script>')).not.toBeNull();
	});

	it('adds a symbol, trimmed, and refuses duplicates in any case', () => {
		const r = addSymbol(['XAUUSD'], '  eurusd ');
		expect(r).toEqual({ list: ['XAUUSD', 'eurusd'] });
		expect(addSymbol(['XAUUSD'], 'xauusd')).toEqual({ error: 'xauusd is already in the list' });
	});

	it('allows a spoken alias as a watchlist entry', () => {
		expect(addSymbol([], 'yellow  metal')).toEqual({ list: ['yellow metal'] });
		expect(addSymbol([], 'bad;entry')).toHaveProperty('error');
	});

	it('stops at 30 entries', () => {
		const full = Array.from({ length: MAX_WATCHLIST }, (_, i) => `S${i}`);
		expect(addSymbol(full, 'ONEMORE')).toEqual({ error: `The watchlist holds at most ${MAX_WATCHLIST} symbols` });
		expect(watchlistError(full)).toBeNull();
		expect(watchlistError([...full, 'X'])).toMatch(/At most 30/);
	});

	it('checks a whole list before saving', () => {
		expect(watchlistError(['XAUUSD', 'EURUSD'])).toBeNull();
		expect(watchlistError([])).toBeNull();
		expect(watchlistError(['XAUUSD', 'xauusd'])).toMatch(/appears twice/);
		expect(watchlistError(['XAUUSD', ''])).toMatch(/\(empty\)/);
	});

	it('moves items without mutating and clamps at the ends', () => {
		const a = ['A', 'B', 'C'];
		expect(moveItem(a, 2, -1)).toEqual(['A', 'C', 'B']);
		expect(moveItem(a, 0, -1)).toEqual(['A', 'B', 'C']);
		expect(moveItem(a, 2, 1)).toEqual(['A', 'B', 'C']);
		expect(moveItem(a, 0, 5)).toEqual(['B', 'C', 'A']);
		expect(moveItem(a, 9, 1)).toEqual(['A', 'B', 'C']);
		expect(a).toEqual(['A', 'B', 'C']);
	});
});

describe('aliases', () => {
	it('normalises spoken names the way the server keys them', () => {
		expect(normalizeSpoken('  Yellow   METAL ')).toBe('yellow metal');
	});

	it('converts between the map and editable rows', () => {
		const rows = aliasesToRows({ silver: 'XAGUSD', 'yellow metal': 'XAUUSD' });
		expect(rows).toEqual([
			{ spoken: 'silver', symbol: 'XAGUSD' },
			{ spoken: 'yellow metal', symbol: 'XAUUSD' }
		]);
		expect(rowsToAliases([{ spoken: ' Yellow Metal ', symbol: ' XAUUSD ' }, { spoken: '', symbol: 'X' }])).toEqual({
			'yellow metal': 'XAUUSD'
		});
	});

	it('flags each bad row and stays ok for good ones', () => {
		const ok = validateAliasRows([{ spoken: 'gold', symbol: 'XAUUSD' }]);
		expect(ok.ok).toBe(true);

		const v = validateAliasRows([
			{ spoken: '', symbol: 'XAUUSD' },
			{ spoken: 'gold', symbol: 'bad symbol' },
			{ spoken: 'cable', symbol: 'GBPUSD' },
			{ spoken: 'Cable', symbol: 'GBPUSD' }
		]);
		expect(v.ok).toBe(false);
		expect(v.rows[0]).toMatch(/spoken name/i);
		expect(v.rows[1]).toMatch(/no spaces/);
		expect(v.rows[2]).toBeNull();
		expect(v.rows[3]).toMatch(/already used in row 3/);
	});

	it('refuses more than 100', () => {
		const rows = Array.from({ length: MAX_ALIASES + 1 }, (_, i) => ({ spoken: `name ${i}`, symbol: 'XAUUSD' }));
		const v = validateAliasRows(rows);
		expect(v.general).toMatch(/At most 100/);
		expect(v.ok).toBe(false);
	});
});

describe('trading limits (tighten-only)', () => {
	const form = () => limitsToForm(CONFIG, CONFIG);

	it('starts from the effective values; hours are off until narrowed', () => {
		const f = form();
		expect(f).toMatchObject({ max_lot: '1', max_positions: '5', max_daily_loss: '500', hours_enabled: false });
		expect(f.allowed_symbols).toEqual(CONFIG.allowed_symbols);
		expect(f.days).toEqual(['mon', 'tue', 'wed', 'thu', 'fri']);
	});

	it('shows already-narrowed hours as enabled', () => {
		const eff: Limits = { ...CONFIG, trading_hours: { days: ['mon', 'tue'], start: '08:00', end: '16:00' } };
		const f = limitsToForm(eff, CONFIG);
		expect(f).toMatchObject({ hours_enabled: true, days: ['mon', 'tue'], start: '08:00', end: '16:00' });
	});

	it('accepts the config values and stricter ones', () => {
		expect(validateLimitsForm(form(), CONFIG)).toEqual({});
		const f = { ...form(), max_lot: '0.5', max_positions: '3', max_daily_loss: '200', allowed_symbols: ['XAUUSD'] };
		expect(validateLimitsForm(f, CONFIG)).toEqual({});
	});

	it('rejects a looser value per field, with the config value in the message', () => {
		const e = validateLimitsForm(
			{ ...form(), max_lot: '2', max_positions: '9', max_daily_loss: '900' },
			CONFIG
		);
		expect(e.max_lot).toMatch(/above the config value \(1\)/);
		expect(e.max_positions).toMatch(/\(5\)/);
		expect(e.max_daily_loss).toMatch(/\(500\)/);
	});

	it('rejects nonsense numbers', () => {
		const e = validateLimitsForm({ ...form(), max_lot: '', max_positions: '2.5', max_daily_loss: '-1' }, CONFIG);
		expect(e.max_lot).toBeDefined();
		expect(e.max_positions).toMatch(/whole number/);
		expect(e.max_daily_loss).toMatch(/above 0/);
		expect(validateLimitsForm({ ...form(), max_lot: 'abc' }, CONFIG).max_lot).toBeDefined();
	});

	it('rejects symbols outside the config and an empty selection', () => {
		expect(validateLimitsForm({ ...form(), allowed_symbols: ['XAUUSD', 'US30'] }, CONFIG).allowed_symbols).toMatch(/US30/);
		expect(validateLimitsForm({ ...form(), allowed_symbols: [] }, CONFIG).allowed_symbols).toMatch(/at least one/);
	});

	it('checks narrowed hours against the config window and days', () => {
		const narrow = { ...form(), hours_enabled: true };
		expect(validateLimitsForm({ ...narrow, start: '08:00', end: '16:00' }, CONFIG)).toEqual({});
		expect(validateLimitsForm({ ...narrow, start: '00:00', end: '16:00' }, CONFIG).trading_hours).toMatch(/inside the config window/);
		expect(validateLimitsForm({ ...narrow, start: '08:00', end: '23:59' }, CONFIG).trading_hours).toMatch(/inside the config window/);
		expect(validateLimitsForm({ ...narrow, days: ['mon', 'sat'] }, CONFIG).trading_hours).toMatch(/Sat/);
		expect(validateLimitsForm({ ...narrow, days: [] }, CONFIG).trading_hours).toMatch(/at least one day/);
		expect(validateLimitsForm({ ...narrow, start: '16:00', end: '08:00' }, CONFIG).trading_hours).toMatch(/start before it ends/);
		expect(validateLimitsForm({ ...narrow, start: '8am', end: '16:00' }, CONFIG).trading_hours).toMatch(/HH:MM/);
	});

	it('does not judge hours when they are not being narrowed', () => {
		const e = validateLimitsForm({ ...form(), hours_enabled: false, start: 'junk', days: [] }, CONFIG);
		expect(e.trading_hours).toBeUndefined();
	});

	it('any window is stricter than no hours at all', () => {
		const none: Limits = { ...CONFIG, trading_hours: null };
		const f = { ...limitsToForm(none, none), hours_enabled: true, start: '09:00', end: '17:00' };
		expect(validateLimitsForm(f, none)).toEqual({});
	});

	it('builds a body from only the fields that are stricter than the config', () => {
		expect(formToOverrides(form(), CONFIG)).toEqual({});
		expect(hasOverrides(formToOverrides(form(), CONFIG))).toBe(false);

		const f = {
			...form(),
			max_lot: '0.5',
			allowed_symbols: ['GBPUSD', 'XAUUSD'],
			hours_enabled: true,
			days: ['fri' as const, 'mon' as const],
			start: '08:00',
			end: '16:00'
		};
		expect(formToOverrides(f, CONFIG)).toEqual({
			max_lot: 0.5,
			// config order, not click order
			allowed_symbols: ['XAUUSD', 'GBPUSD'],
			trading_hours: { days: ['mon', 'fri'], start: '08:00', end: '16:00' }
		});
	});

	it('does not pin narrowed hours that equal the config', () => {
		const f = { ...form(), hours_enabled: true };
		expect(formToOverrides(f, CONFIG).trading_hours).toBeUndefined();
	});

	it('sends every field on save, because the server merges into the stored overrides', () => {
		// 0.5 is stored; the user moves it back to the config value: it must be sent, not left out.
		const f = { ...form(), max_lot: '1', max_positions: '3' };
		const { body } = limitsBody(f, CONFIG, { max_lot: 0.5 });
		expect(body).toEqual({
			max_lot: 1,
			max_positions: 3,
			max_daily_loss: 500,
			allowed_symbols: ['XAUUSD', 'EURUSD', 'GBPUSD']
		});
	});

	it('puts the config hours back when narrowed hours are switched off', () => {
		const f = { ...form(), hours_enabled: false };
		const stored = { trading_hours: { days: ['mon' as const], start: '09:00', end: '10:00' } };
		expect(limitsBody(f, CONFIG, stored).body.trading_hours).toEqual(CONFIG.trading_hours);
		expect(limitsBody(f, CONFIG, null).body.trading_hours).toBeUndefined();
	});

	it('cannot undo narrowed hours by merge when the config has none', () => {
		const none: Limits = { ...CONFIG, trading_hours: null };
		const f = { ...limitsToForm(none, none), hours_enabled: false };
		const stored = { trading_hours: { days: ['mon' as const], start: '09:00', end: '10:00' } };
		const r = limitsBody(f, none, stored);
		expect(r.stuckHours).toBe(true);
		expect(r.body.trading_hours).toBeUndefined();
	});

	it('reads the field a server refusal is about', () => {
		expect(limitErrorField('max_lot 2 is looser than the config value 1')).toBe('max_lot');
		expect(limitErrorField('allowed_symbols: US30 not in the config list')).toBe('allowed_symbols');
		expect(limitErrorField('trading_hours: window 00:00-16:00 is looser')).toBe('trading_hours');
		expect(limitErrorField('the trading hours window is wider')).toBe('trading_hours');
		expect(limitErrorField('something else')).toBeNull();
	});

	it('parses HH:MM strictly and describes windows', () => {
		expect(parseHHMM('00:05')).toBe(5);
		expect(parseHHMM('23:59')).toBe(1439);
		expect(parseHHMM('24:00')).toBeNull();
		expect(parseHHMM('9:00')).toBeNull();
		expect(parseHHMM('12:60')).toBeNull();
		expect(describeHours(CONFIG.trading_hours)).toBe('Mon–Fri 00:05–23:55');
		expect(describeHours({ days: ['mon', 'wed', 'thu', 'sun'], start: '08:00', end: '12:00' })).toBe('Mon, Wed, Thu, Sun 08:00–12:00');
		expect(describeHours(null)).toBe('Any time');
	});
});
