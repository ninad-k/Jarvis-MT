import { describe, expect, it } from 'vitest';
import type { SymbolCandidate, SymbolMapping } from './api';
import {
	affixText,
	aliasLine,
	attentionCount,
	exportFileName,
	filterCandidates,
	groupMappings,
	normalizeClass,
	rankCandidates,
	sourceInfo,
	symbolLabel
} from './symbols';

const m = (canonical: string, cls: string, extra: Partial<SymbolMapping> = {}): SymbolMapping => ({
	canonical,
	class: cls,
	aliases: [],
	broker_symbol: canonical,
	source: 'curated',
	available: true,
	candidates: [],
	...extra
});

describe('normalizeClass', () => {
	it('accepts the contract names and common variants', () => {
		expect(normalizeClass('metal')).toBe('metal');
		expect(normalizeClass('Indices')).toBe('index');
		expect(normalizeClass(' forex ')).toBe('fx');
		expect(normalizeClass('FX')).toBe('fx');
	});
	it('files anything else under other', () => {
		expect(normalizeClass('stock')).toBe('other');
		expect(normalizeClass('')).toBe('other');
		expect(normalizeClass(undefined)).toBe('other');
	});
});

describe('groupMappings', () => {
	it('orders the groups Metals, Indices, Energy, Crypto, FX and skips empty ones', () => {
		const groups = groupMappings([
			m('EURUSD', 'fx'),
			m('BTCUSD', 'crypto'),
			m('US100', 'index'),
			m('XAUUSD', 'metal'),
			m('USOIL', 'energy')
		]);
		expect(groups.map((g) => g.label)).toEqual(['Metals', 'Indices', 'Energy', 'Crypto', 'FX']);
		expect(groupMappings([m('XAUUSD', 'metal'), m('EURUSD', 'fx')]).map((g) => g.id)).toEqual(['metal', 'fx']);
		expect(groupMappings([])).toEqual([]);
	});
	it('keeps the server order inside a group', () => {
		const [g] = groupMappings([m('US30', 'index'), m('US100', 'index'), m('GER40', 'index')]);
		expect(g.rows.map((r) => r.canonical)).toEqual(['US30', 'US100', 'GER40']);
	});
	it('puts unknown classes last', () => {
		const groups = groupMappings([m('AAPL', 'stock'), m('EURUSD', 'fx')]);
		expect(groups.map((g) => g.label)).toEqual(['FX', 'Other']);
	});
});

describe('aliasLine', () => {
	it('joins the canonical name and its aliases', () => {
		expect(aliasLine({ canonical: 'US100', aliases: ['Nasdaq', 'NAS100', 'USTEC'] })).toBe('US100 · Nasdaq, NAS100, USTEC');
		expect(aliasLine({ canonical: 'EURGBP', aliases: [] })).toBe('EURGBP');
	});
});

describe('sourceInfo', () => {
	it('words the five sources as user, curated, auto, pattern and unchanged', () => {
		expect(sourceInfo('user').label).toBe('user');
		expect(sourceInfo('curated').label).toBe('curated');
		expect(sourceInfo('generated').label).toBe('auto');
		expect(sourceInfo('pattern').label).toBe('pattern');
		expect(sourceInfo('passthrough').label).toBe('unchanged');
	});
	it('explains each one', () => {
		for (const s of ['user', 'curated', 'generated', 'pattern', 'passthrough']) {
			expect(sourceInfo(s).tip.length).toBeGreaterThan(20);
		}
		expect(sourceInfo('pattern').tip).toMatch(/check/i);
	});
	it('shows an unknown source as it is', () => {
		expect(sourceInfo('magic')).toMatchObject({ label: 'magic' });
	});
});

describe('candidates', () => {
	const c = (broker_symbol: string, score: number, description = ''): SymbolCandidate => ({ broker_symbol, description, score, reasons: [] });
	it('ranks by score, keeps the server order on ties and drops the current symbol', () => {
		const ranked = rankCandidates([c('B', 5), c('A', 9), c('C', 5), c('NOW', 20)], 'NOW');
		expect(ranked.map((x) => x.broker_symbol)).toEqual(['A', 'B', 'C']);
	});
	it('filters by name or description, case-insensitively', () => {
		const list = [c('US100_Spot', 8, 'US Tech 100'), c('USTEC', 6, 'Nasdaq')];
		expect(filterCandidates(list, '').length).toBe(2);
		expect(filterCandidates(list, 'ustec').map((x) => x.broker_symbol)).toEqual(['USTEC']);
		expect(filterCandidates(list, 'NASDAQ').map((x) => x.broker_symbol)).toEqual(['USTEC']);
		expect(filterCandidates(list, 'zzz')).toEqual([]);
	});
});

describe('symbolLabel', () => {
	it('shows the canonical name with the broker symbol beside it when they differ', () => {
		expect(symbolLabel({ symbol: 'XAUUSD_', canonical: 'XAUUSD' })).toEqual({ primary: 'XAUUSD', secondary: 'XAUUSD_' });
	});
	it('shows one name when they match or the server sends no canonical', () => {
		expect(symbolLabel({ symbol: 'EURUSD', canonical: 'EURUSD' })).toEqual({ primary: 'EURUSD', secondary: null });
		expect(symbolLabel({ symbol: 'EURUSD' })).toEqual({ primary: 'EURUSD', secondary: null });
		expect(symbolLabel({ symbol: 'EURUSD', canonical: '' })).toEqual({ primary: 'EURUSD', secondary: null });
	});
});

describe('small formatters', () => {
	it('counts rows that need attention', () => {
		expect(attentionCount([m('A', 'fx'), m('B', 'fx', { available: false })], ['HK50'])).toBe(2);
		expect(attentionCount([], [])).toBe(0);
		// An unavailable row that is also listed as unmapped counts once.
		expect(attentionCount([m('B', 'fx', { available: false })], ['B', 'HK50'])).toBe(2);
	});
	it('names affixes', () => {
		expect(affixText('_')).toBe('"_"');
		expect(affixText('')).toBe('none');
	});
	it('builds a safe export file name', () => {
		expect(exportFileName({ company: 'CFI Financial Markets L.L.C' })).toBe('broker-symbols-cfi-financial-markets-l-l-c.json');
		expect(exportFileName(null)).toBe('broker-symbols.json');
	});
});
