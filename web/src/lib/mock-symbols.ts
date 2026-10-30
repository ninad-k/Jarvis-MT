/**
 * Mock of the broker symbol mapping endpoints (docs/api.md "Broker symbol mapping").
 * Mock mode only (api.ts imports ./mock behind a build-time constant).
 *
 * Two brokers: a CFI-like one (suffix "_", "_Spot" indices; the default) and an IC-Markets-like one
 * (no suffix, USTEC / DE40 / XTIUSD). `?mock=switchbroker` switches from the first to the second a
 * few seconds after the first request and pushes `symbol_map_changed`. User overrides are kept per
 * broker in sessionStorage, like a real profile would.
 */
import {
	ApiError,
	type BrokerInfo,
	type CatalogResponse,
	type CatalogSymbol,
	type StatusBroker,
	type SymbolCandidate,
	type SymbolMapping,
	type SymbolSource,
	type SymbolsResponse
} from './api';
import { emitMockEvent } from './mock-trading';

// ---- Instruments --------------------------------------------------------------------

interface Instrument {
	canonical: string;
	class: string;
	aliases: string[];
}

const fx = (canonical: string, aliases: string[] = []): Instrument => ({ canonical, class: 'fx', aliases });

/** Canonical instruments, as config/instruments.toml would list them. */
export const INSTRUMENTS: Instrument[] = [
	{ canonical: 'XAUUSD', class: 'metal', aliases: ['Gold', 'XAU'] },
	{ canonical: 'XAGUSD', class: 'metal', aliases: ['Silver', 'XAG'] },
	{ canonical: 'US100', class: 'index', aliases: ['Nasdaq', 'NAS100', 'USTEC'] },
	{ canonical: 'US30', class: 'index', aliases: ['Dow Jones', 'DJ30', 'Wall Street'] },
	{ canonical: 'US500', class: 'index', aliases: ['S&P 500', 'SPX500'] },
	{ canonical: 'GER40', class: 'index', aliases: ['DAX', 'DE40', 'GER30'] },
	{ canonical: 'UK100', class: 'index', aliases: ['FTSE 100', 'FTSE'] },
	{ canonical: 'JP225', class: 'index', aliases: ['Nikkei', 'JPN225'] },
	{ canonical: 'HK50', class: 'index', aliases: ['Hang Seng', 'HSI'] },
	{ canonical: 'USOIL', class: 'energy', aliases: ['WTI', 'Crude Oil'] },
	{ canonical: 'UKOIL', class: 'energy', aliases: ['Brent'] },
	{ canonical: 'NATGAS', class: 'energy', aliases: ['Natural Gas'] },
	{ canonical: 'BTCUSD', class: 'crypto', aliases: ['Bitcoin', 'XBT'] },
	{ canonical: 'ETHUSD', class: 'crypto', aliases: ['Ethereum'] },
	{ canonical: 'LTCUSD', class: 'crypto', aliases: ['Litecoin'] },
	fx('EURUSD', ['Euro', 'Fiber']),
	fx('GBPUSD', ['Cable', 'Pound']),
	fx('USDJPY', ['Yen']),
	fx('AUDUSD', ['Aussie']),
	fx('USDCAD', ['Loonie']),
	fx('USDCHF', ['Swissie']),
	fx('NZDUSD', ['Kiwi']),
	fx('EURGBP'),
	fx('EURJPY'),
	fx('GBPJPY')
];

// ---- Brokers and their catalogs -------------------------------------------------------

type BrokerId = 'cfi' | 'ic';

interface BrokerDef {
	id: BrokerId;
	company: string;
	server: string;
	suffix: string;
	prefix: string;
	catalog: CatalogSymbol[];
	/** What detection and the curated list resolve, before the user's overrides. */
	base: Record<string, { broker_symbol: string; source: SymbolSource }>;
}

const CURRENCY: Record<string, string> = {
	EUR: 'Euro',
	GBP: 'British Pound',
	AUD: 'Australian Dollar',
	NZD: 'New Zealand Dollar',
	USD: 'US Dollar',
	CAD: 'Canadian Dollar',
	CHF: 'Swiss Franc',
	JPY: 'Japanese Yen',
	MXN: 'Mexican Peso',
	ZAR: 'South African Rand',
	SEK: 'Swedish Krona',
	NOK: 'Norwegian Krone',
	TRY: 'Turkish Lira',
	SGD: 'Singapore Dollar'
};

const FX_PAIRS = [
	'EURUSD', 'GBPUSD', 'USDJPY', 'AUDUSD', 'USDCAD', 'USDCHF', 'NZDUSD', 'EURGBP', 'EURJPY', 'GBPJPY',
	'EURCHF', 'EURAUD', 'EURCAD', 'EURNZD', 'GBPCHF', 'GBPAUD', 'GBPCAD', 'GBPNZD', 'AUDJPY', 'AUDCAD',
	'AUDCHF', 'AUDNZD', 'NZDJPY', 'NZDCAD', 'NZDCHF', 'CADJPY', 'CADCHF', 'CHFJPY'
];
const FX_EXOTIC = ['USDMXN', 'USDZAR', 'USDSEK', 'USDNOK', 'USDTRY', 'USDSGD'];

const US_STOCKS: [string, string][] = [
	['AAPL', 'Apple Inc'], ['MSFT', 'Microsoft Corp'], ['NVDA', 'NVIDIA Corp'], ['AMZN', 'Amazon.com Inc'],
	['GOOGL', 'Alphabet Inc Class A'], ['META', 'Meta Platforms Inc'], ['TSLA', 'Tesla Inc'], ['NFLX', 'Netflix Inc'],
	['AMD', 'Advanced Micro Devices'], ['INTC', 'Intel Corp'], ['ORCL', 'Oracle Corp'], ['CRM', 'Salesforce Inc'],
	['ADBE', 'Adobe Inc'], ['CSCO', 'Cisco Systems'], ['QCOM', 'Qualcomm Inc'], ['AVGO', 'Broadcom Inc'],
	['JPM', 'JPMorgan Chase & Co'], ['BAC', 'Bank of America'], ['GS', 'Goldman Sachs Group'], ['MS', 'Morgan Stanley'],
	['V', 'Visa Inc'], ['MA', 'Mastercard Inc'], ['PYPL', 'PayPal Holdings'], ['WMT', 'Walmart Inc'],
	['KO', 'Coca-Cola Co'], ['PEP', 'PepsiCo Inc'], ['MCD', "McDonald's Corp"], ['NKE', 'Nike Inc'],
	['DIS', 'Walt Disney Co'], ['BA', 'Boeing Co'], ['XOM', 'Exxon Mobil'], ['CVX', 'Chevron Corp'],
	['PFE', 'Pfizer Inc'], ['JNJ', 'Johnson & Johnson'], ['UNH', 'UnitedHealth Group'], ['LLY', 'Eli Lilly & Co'],
	['COST', 'Costco Wholesale'], ['HD', 'Home Depot'], ['UBER', 'Uber Technologies'], ['ABNB', 'Airbnb Inc'],
	['COIN', 'Coinbase Global'], ['PLTR', 'Palantir Technologies'], ['SHOP', 'Shopify Inc'], ['SNOW', 'Snowflake Inc'],
	['T', 'AT&T Inc'], ['VZ', 'Verizon Communications'], ['IBM', 'International Business Machines'], ['GE', 'General Electric'],
	['F', 'Ford Motor Co'], ['GM', 'General Motors'],
	['AMAT', 'Applied Materials'], ['MU', 'Micron Technology'], ['TXN', 'Texas Instruments'], ['LRCX', 'Lam Research'],
	['NOW', 'ServiceNow Inc'], ['INTU', 'Intuit Inc'], ['PANW', 'Palo Alto Networks'], ['CRWD', 'CrowdStrike Holdings'],
	['SQ', 'Block Inc'], ['SPOT', 'Spotify Technology'], ['RBLX', 'Roblox Corp'], ['DASH', 'DoorDash Inc'],
	['ZM', 'Zoom Video Communications'], ['ROKU', 'Roku Inc'], ['SNAP', 'Snap Inc'], ['PINS', 'Pinterest Inc'],
	['WFC', 'Wells Fargo & Co'], ['C', 'Citigroup Inc'], ['AXP', 'American Express'], ['BLK', 'BlackRock Inc'],
	['SCHW', 'Charles Schwab'], ['USB', 'U.S. Bancorp'], ['MRK', 'Merck & Co'], ['ABBV', 'AbbVie Inc'],
	['TMO', 'Thermo Fisher Scientific'], ['AMGN', 'Amgen Inc'], ['GILD', 'Gilead Sciences'], ['MRNA', 'Moderna Inc'],
	['LOW', "Lowe's Companies"], ['TGT', 'Target Corp'], ['SBUX', 'Starbucks Corp'], ['CMG', 'Chipotle Mexican Grill'],
	['LMT', 'Lockheed Martin'], ['RTX', 'RTX Corp'], ['CAT', 'Caterpillar Inc'], ['DE', 'Deere & Co'],
	['UPS', 'United Parcel Service'], ['FDX', 'FedEx Corp'], ['DAL', 'Delta Air Lines'], ['UAL', 'United Airlines Holdings'],
	['COP', 'ConocoPhillips'], ['SLB', 'Schlumberger Ltd'], ['MARA', 'Marathon Digital Holdings'], ['MSTR', 'MicroStrategy Inc']
];

const EU_STOCKS: [string, string, string][] = [
	['SAP', 'SAP SE', 'DE'], ['SIE', 'Siemens AG', 'DE'], ['ALV', 'Allianz SE', 'DE'], ['BMW', 'BMW AG', 'DE'],
	['MBG', 'Mercedes-Benz Group', 'DE'], ['VOW3', 'Volkswagen AG Pref', 'DE'], ['DBK', 'Deutsche Bank AG', 'DE'],
	['BAS', 'BASF SE', 'DE'], ['ADS', 'Adidas AG', 'DE'], ['DTE', 'Deutsche Telekom AG', 'DE'],
	['SHEL', 'Shell plc', 'UK'], ['BP', 'BP plc', 'UK'], ['HSBA', 'HSBC Holdings', 'UK'], ['AZN', 'AstraZeneca plc', 'UK'],
	['ULVR', 'Unilever plc', 'UK'], ['GSK', 'GSK plc', 'UK'], ['RIO', 'Rio Tinto plc', 'UK'], ['BARC', 'Barclays plc', 'UK'],
	['LLOY', 'Lloyds Banking Group', 'UK'], ['VOD', 'Vodafone Group', 'UK']
];

const sym = (name: string, description: string, path: string, extra: Partial<CatalogSymbol> = {}): CatalogSymbol => ({
	name,
	description,
	path,
	trade_mode: 'full',
	visible: true,
	...extra
});

interface Naming {
	fx: (pair: string) => string;
	metal: (base: string) => string;
	energy: (name: string) => string;
	crypto: (name: string) => string;
	stock: (ticker: string, market: string) => string;
}

function buildCatalog(n: Naming, indices: [string, string][], metals: [string, string][], energy: [string, string][], crypto: [string, string][], extra: CatalogSymbol[]): CatalogSymbol[] {
	const out: CatalogSymbol[] = [];
	const pairDesc = (p: string) => `${CURRENCY[p.slice(0, 3)] ?? p.slice(0, 3)} vs ${CURRENCY[p.slice(3)] ?? p.slice(3)}`;
	for (const p of FX_PAIRS) out.push(sym(n.fx(p), pairDesc(p), 'Forex\\Majors and minors'));
	for (const p of FX_EXOTIC) out.push(sym(n.fx(p), pairDesc(p), 'Forex\\Exotics'));
	for (const [name, desc] of metals) out.push(sym(n.metal(name), desc, 'Metals'));
	for (const [name, desc] of indices) out.push(sym(name, desc, 'Indices'));
	for (const [name, desc] of energy) out.push(sym(n.energy(name), desc, 'Energies'));
	for (const [name, desc] of crypto) out.push(sym(n.crypto(name), desc, 'Crypto'));
	for (const [t, d] of US_STOCKS) out.push(sym(n.stock(t, 'US'), d, 'Stocks\\US'));
	for (const [t, d, m] of EU_STOCKS) out.push(sym(n.stock(t, m), d, m === 'DE' ? 'Stocks\\Germany' : 'Stocks\\UK'));
	out.push(...extra);
	return out;
}

const CFI: BrokerDef = {
	id: 'cfi',
	company: 'CFI Financial Markets L.L.C',
	server: 'CFI3-Demo',
	suffix: '_',
	prefix: '',
	catalog: buildCatalog(
		{
			fx: (p) => `${p}_`,
			metal: (b) => `${b}_`,
			energy: (b) => `${b}_`,
			crypto: (b) => `${b}_`,
			stock: (t, m) => `${t}.${m}`
		},
		[
			['US100_Spot', 'US Tech 100 Index, spot CFD'],
			['US100_Dec', 'US Tech 100 Index, December futures'],
			['NAS100.cash', 'Nasdaq 100 cash CFD'],
			['US30_Spot', 'Wall Street 30 Index, spot CFD'],
			['US500_Spot', 'US 500 Index, spot CFD'],
			['GER40_Spot', 'Germany 40 Index, spot CFD'],
			['UK100_Spot', 'UK 100 Index, spot CFD'],
			['JPN225_Spot', 'Japan 225 Index, spot CFD'],
			['FRA40_Spot', 'France 40 Index, spot CFD'],
			['AUS200_Spot', 'Australia 200 Index, spot CFD'],
			['ESP35_Spot', 'Spain 35 Index, spot CFD'],
			['EU50_Spot', 'Euro Stoxx 50 Index, spot CFD']
		],
		[
			['XAUUSD', 'Gold vs US Dollar'],
			['XAGUSD', 'Silver vs US Dollar'],
			['XAUEUR', 'Gold vs Euro'],
			['XPTUSD', 'Platinum vs US Dollar'],
			['XPDUSD', 'Palladium vs US Dollar']
		],
		[
			['USOIL', 'WTI crude oil, spot CFD'],
			['UKOIL', 'Brent crude oil, spot CFD'],
			['NATGAS', 'Natural gas, spot CFD']
		],
		[
			['BTCUSD', 'Bitcoin vs US Dollar'],
			['ETHUSD', 'Ethereum vs US Dollar'],
			['XRPUSD', 'Ripple vs US Dollar'],
			['SOLUSD', 'Solana vs US Dollar'],
			['ADAUSD', 'Cardano vs US Dollar'],
			['DOGEUSD', 'Dogecoin vs US Dollar']
		],
		[sym('GOLD_CASH', 'Gold cash, legacy contract', 'Metals', { trade_mode: 'closeonly', visible: false })]
	),
	base: {
		XAUUSD: { broker_symbol: 'XAUUSD_', source: 'curated' },
		XAGUSD: { broker_symbol: 'XAGUSD_', source: 'curated' },
		US100: { broker_symbol: 'US100_Spot', source: 'generated' },
		US30: { broker_symbol: 'US30_Spot', source: 'generated' },
		US500: { broker_symbol: 'US500_Spot', source: 'generated' },
		GER40: { broker_symbol: 'GER40_Spot', source: 'pattern' },
		UK100: { broker_symbol: 'UK100_Spot', source: 'pattern' },
		// Nothing with this name: the canonical name goes to the broker as it is, which fails.
		JP225: { broker_symbol: 'JP225', source: 'passthrough' },
		USOIL: { broker_symbol: 'USOIL_', source: 'curated' },
		UKOIL: { broker_symbol: 'UKOIL_', source: 'generated' },
		NATGAS: { broker_symbol: 'NATGAS_', source: 'pattern' },
		BTCUSD: { broker_symbol: 'BTCUSD_', source: 'curated' },
		ETHUSD: { broker_symbol: 'ETHUSD_', source: 'generated' },
		EURUSD: { broker_symbol: 'EURUSD_', source: 'curated' },
		GBPUSD: { broker_symbol: 'GBPUSD_', source: 'curated' },
		USDJPY: { broker_symbol: 'USDJPY_', source: 'pattern' },
		AUDUSD: { broker_symbol: 'AUDUSD_', source: 'pattern' },
		USDCAD: { broker_symbol: 'USDCAD_', source: 'pattern' },
		USDCHF: { broker_symbol: 'USDCHF_', source: 'pattern' },
		NZDUSD: { broker_symbol: 'NZDUSD_', source: 'pattern' },
		EURGBP: { broker_symbol: 'EURGBP_', source: 'pattern' },
		EURJPY: { broker_symbol: 'EURJPY_', source: 'pattern' },
		GBPJPY: { broker_symbol: 'GBPJPY_', source: 'pattern' }
		// HK50 and LTCUSD have no match on this broker.
	}
};

const IC: BrokerDef = {
	id: 'ic',
	company: 'Raw Trading Ltd',
	server: 'ICMarketsSC-Demo',
	suffix: '',
	prefix: '',
	catalog: buildCatalog(
		{
			fx: (p) => p,
			metal: (b) => b,
			energy: (b) => b,
			crypto: (b) => b,
			stock: (t, m) => `${t}.${m === 'US' ? 'NAS' : m}`
		},
		[
			['USTEC', 'US Tech 100 Index'],
			['US30', 'Wall Street 30 Index'],
			['US500', 'US SP 500 Index'],
			['DE40', 'Germany 40 Index'],
			['UK100', 'UK 100 Index'],
			['JP225', 'Japan 225 Index'],
			['HK50', 'Hong Kong 50 Index'],
			['F40', 'France 40 Index'],
			['AUS200', 'Australia 200 Index'],
			['ES35', 'Spain 35 Index'],
			['STOXX50', 'Euro Stoxx 50 Index']
		],
		[
			['XAUUSD', 'Gold vs US Dollar'],
			['XAGUSD', 'Silver vs US Dollar'],
			['XAUEUR', 'Gold vs Euro'],
			['XPTUSD', 'Platinum vs US Dollar'],
			['XPDUSD', 'Palladium vs US Dollar']
		],
		[
			['XTIUSD', 'West Texas Intermediate crude oil'],
			['XBRUSD', 'Brent crude oil'],
			['XNGUSD', 'Natural gas']
		],
		[
			['BTCUSD', 'Bitcoin vs US Dollar'],
			['ETHUSD', 'Ethereum vs US Dollar'],
			['LTCUSD', 'Litecoin vs US Dollar'],
			['XRPUSD', 'Ripple vs US Dollar'],
			['SOLUSD', 'Solana vs US Dollar'],
			['ADAUSD', 'Cardano vs US Dollar']
		],
		[]
	),
	base: {
		XAUUSD: { broker_symbol: 'XAUUSD', source: 'curated' },
		XAGUSD: { broker_symbol: 'XAGUSD', source: 'curated' },
		US100: { broker_symbol: 'USTEC', source: 'curated' },
		US30: { broker_symbol: 'US30', source: 'curated' },
		US500: { broker_symbol: 'US500', source: 'generated' },
		GER40: { broker_symbol: 'DE40', source: 'generated' },
		UK100: { broker_symbol: 'UK100', source: 'passthrough' },
		JP225: { broker_symbol: 'JP225', source: 'passthrough' },
		HK50: { broker_symbol: 'HK50', source: 'passthrough' },
		USOIL: { broker_symbol: 'XTIUSD', source: 'generated' },
		UKOIL: { broker_symbol: 'XBRUSD', source: 'generated' },
		NATGAS: { broker_symbol: 'XNGUSD', source: 'generated' },
		BTCUSD: { broker_symbol: 'BTCUSD', source: 'passthrough' },
		ETHUSD: { broker_symbol: 'ETHUSD', source: 'passthrough' },
		LTCUSD: { broker_symbol: 'LTCUSD', source: 'passthrough' },
		...Object.fromEntries(
			INSTRUMENTS.filter((i) => i.class === 'fx').map((i) => [i.canonical, { broker_symbol: i.canonical, source: 'passthrough' as SymbolSource }])
		)
	}
};

const BROKERS: Record<BrokerId, BrokerDef> = { cfi: CFI, ic: IC };

// ---- State (sessionStorage) -------------------------------------------------------------

const KEY = 'arcmt.mock.symbols';

interface Stored {
	broker: BrokerId;
	/** Per broker company: canonical -> broker symbol the user chose. */
	overrides: Record<string, Record<string, string>>;
	/** Per broker company: ISO time of the last detection. */
	detected: Record<string, string>;
}

let state: Stored | null = null;

function cur(): Stored {
	if (state) return state;
	let loaded: Stored | null = null;
	try {
		const raw = sessionStorage.getItem(KEY);
		if (raw) loaded = JSON.parse(raw) as Stored;
	} catch {
		/* storage unavailable: state lasts until reload */
	}
	state = loaded ?? { broker: 'cfi', overrides: {}, detected: {} };
	return state;
}

function persist(): void {
	try {
		sessionStorage.setItem(KEY, JSON.stringify(cur()));
	} catch {
		/* storage unavailable */
	}
}

const broker = (): BrokerDef => BROKERS[cur().broker];
const userMap = (): Record<string, string> => (cur().overrides[broker().company] ??= {});
const catalogHas = (b: BrokerDef, name: string) => b.catalog.some((s) => s.name === name);

// ---- Candidates ------------------------------------------------------------------------------

const alnum = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, '');

/** A small stand-in for the auto-mapper's ranking: name and alias matches, plus a bonus for tradable, visible, suffix-matching names. */
function candidatesFor(b: BrokerDef, ins: Instrument): SymbolCandidate[] {
	const canon = alnum(ins.canonical);
	const out: SymbolCandidate[] = [];
	for (const s of b.catalog) {
		const n = alnum(s.name);
		const d = s.description.toLowerCase();
		let score = 0;
		const reasons: string[] = [];
		if (n.startsWith(canon)) {
			score = 6;
			reasons.push('name matches the canonical name');
		} else {
			const hit = ins.aliases.find((a) => alnum(a).length >= 3 && n.startsWith(alnum(a)));
			const hitDesc = ins.aliases.find((a) => a.length >= 4 && d.includes(a.toLowerCase()));
			if (hit) {
				score = 4;
				reasons.push(`name matches the alias ${hit}`);
			} else if (hitDesc) {
				score = 3;
				reasons.push(`description matches the alias ${hitDesc}`);
			}
		}
		if (score === 0) continue;
		if (s.visible) {
			score += 1;
			reasons.push('visible');
		}
		if (s.trade_mode === 'full') {
			score += 1;
			reasons.push('tradable');
		}
		if (b.suffix && s.name.endsWith(b.suffix)) {
			score += 1;
			reasons.push('matches detected suffix');
		}
		out.push({ broker_symbol: s.name, description: s.description, score, reasons });
	}
	return out.sort((a, b2) => b2.score - a.score || a.broker_symbol.localeCompare(b2.broker_symbol)).slice(0, 5);
}

// ---- Public API -----------------------------------------------------------------------------------

function brokerInfo(b: BrokerDef): BrokerInfo {
	const s = cur();
	s.detected[b.company] ??= new Date(Date.now() - 3600_000).toISOString();
	return {
		company: b.company,
		server: b.server,
		detected_at: s.detected[b.company],
		catalog_size: b.catalog.length,
		detected_suffix: b.suffix,
		detected_prefix: b.prefix
	};
}

function resolve(b: BrokerDef, canonical: string): { broker_symbol: string; source: SymbolSource } | null {
	const mine = userMap()[canonical];
	if (mine) return { broker_symbol: mine, source: 'user' };
	return b.base[canonical] ?? null;
}

export function getSymbols(): SymbolsResponse {
	const b = broker();
	const mappings: SymbolMapping[] = [];
	const unmapped: string[] = [];
	for (const ins of INSTRUMENTS) {
		const r = resolve(b, ins.canonical);
		if (!r) {
			unmapped.push(ins.canonical);
			continue;
		}
		mappings.push({
			canonical: ins.canonical,
			class: ins.class,
			aliases: ins.aliases,
			broker_symbol: r.broker_symbol,
			source: r.source,
			available: catalogHas(b, r.broker_symbol),
			candidates: candidatesFor(b, ins)
		});
	}
	return { broker: brokerInfo(b), mappings, unmapped };
}

export function statusBroker(): StatusBroker {
	const b = broker();
	const res = getSymbols();
	return {
		company: b.company,
		server: b.server,
		mapped: res.mappings.filter((m) => m.available).length,
		total: INSTRUMENTS.length
	};
}

export function searchCatalog(path: string): CatalogResponse {
	const q = new URLSearchParams(path.split('?')[1] ?? '');
	const needle = (q.get('q') ?? '').trim().toLowerCase();
	const limit = Math.max(1, Math.min(200, Number(q.get('limit') ?? 50) || 50));
	const rank = (s: CatalogSymbol) => {
		const n = s.name.toLowerCase();
		return n === needle ? 0 : n.startsWith(needle) ? 1 : n.includes(needle) ? 2 : 3;
	};
	const hits = broker()
		.catalog.filter((s) => !needle || s.name.toLowerCase().includes(needle) || s.description.toLowerCase().includes(needle))
		.sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));
	return { symbols: hits.slice(0, limit).map((s) => ({ ...s })), total: hits.length };
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

/**
 * `PUT /api/symbols/map`. Like the server: the body is validated first (400, then 422 for a name
 * the broker does not list), then `checkCode` runs, then the override is stored or removed.
 */
export function putSymbolMap(body: unknown, checkCode: () => void): void {
	if (!isObj(body)) throw new ApiError('bad_request', 'body must be a JSON object', 400);
	for (const k of Object.keys(body)) {
		if (!['canonical', 'broker_symbol', 'totp'].includes(k)) throw new ApiError('bad_request', `unknown field: ${k}`, 400);
	}
	const canonical = body.canonical;
	if (typeof canonical !== 'string' || !INSTRUMENTS.some((i) => i.canonical === canonical))
		throw new ApiError('bad_request', `unknown instrument: ${String(canonical)}`, 400);
	if (!('broker_symbol' in body)) throw new ApiError('bad_request', 'broker_symbol is required (null removes the override)', 400);
	const target = body.broker_symbol;
	if (target !== null && (typeof target !== 'string' || target.trim() === ''))
		throw new ApiError('bad_request', 'broker_symbol must be a name or null', 400);
	const b = broker();
	if (typeof target === 'string' && !catalogHas(b, target))
		throw new ApiError('unknown_broker_symbol', `${target} is not in the ${b.company} catalog`, 422);
	checkCode();
	const mine = userMap();
	if (target === null) delete mine[canonical];
	else mine[canonical] = target;
	persist();
	emitMockEvent({ type: 'symbol_map_changed', broker: b.company });
}

/** `POST /api/symbols/detect`: re-reads the catalog (here: stamps a new time; user overrides stay). */
export function detectSymbols(): SymbolsResponse {
	cur().detected[broker().company] = new Date().toISOString();
	persist();
	emitMockEvent({ type: 'symbol_map_changed', broker: broker().company });
	return getSymbols();
}

export function exportSymbols(): Blob {
	const b = broker();
	const map: Record<string, string> = {};
	for (const m of getSymbols().mappings) map[m.canonical] = m.broker_symbol;
	const doc = { broker: b.company, server: b.server, generated_at: new Date().toISOString(), map };
	return new Blob([JSON.stringify(doc, null, 2)], { type: 'application/json' });
}

// ---- Rows ------------------------------------------------------------------------------------------

/** The broker's name for a canonical symbol, or the name itself when it has no mapping. */
export function toBroker(canonical: string): string {
	const r = resolve(broker(), canonical);
	return r ? r.broker_symbol : canonical;
}

/** The canonical name for a broker symbol (or a canonical name, or an alias), or the input unchanged. */
export function toCanonical(name: string): string {
	const b = broker();
	for (const ins of INSTRUMENTS) {
		if (resolve(b, ins.canonical)?.broker_symbol === name) return ins.canonical;
	}
	return name;
}

/** A market row as the API sends it: the broker's `symbol` and the `canonical` name. */
export function withBrokerSymbol<T extends { symbol: string }>(row: T): T & { canonical: string } {
	return { ...row, canonical: row.symbol, symbol: toBroker(row.symbol) };
}

// ---- Broker switch -------------------------------------------------------------------------------------

let switchTimer: ReturnType<typeof setTimeout> | undefined;

/**
 * `?mock=switchbroker`: a few seconds after the first request the account in MT5 "changes" to the
 * other broker. Happens once per browser session (the new broker is remembered).
 */
export function scheduleBrokerSwitch(ms = 3500): void {
	if (switchTimer !== undefined || cur().broker === 'ic') return;
	switchTimer = setTimeout(() => {
		cur().broker = 'ic';
		persist();
		emitMockEvent({ type: 'symbol_map_changed', broker: IC.company });
	}, ms);
}
