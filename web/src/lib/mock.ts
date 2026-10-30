/**
 * Mock backend for UI development (VITE_MOCK=1, see `pnpm dev:mock`).
 * Never bundled into production builds: api.ts imports it dynamically behind a build-time constant.
 *
 * URL switches (mock mode only):
 *   ?mock=live     LIVE mode + real account (banner, red badge)
 *   ?mock=bridge   API up, MT5 bridge down
 *   ?mock=offline  API unreachable
 *   (chat messages containing "fail" return an llm_error)
 */
import {
	ApiError,
	type Account,
	type ChatRequest,
	type ChatResponse,
	type Position,
	type PositionsResponse,
	type PriceTick,
	type RequestOptions,
	type Status,
	type ToolCall,
	type WatchlistResponse
} from './api';

interface Sym {
	symbol: string;
	digits: number;
	mid: number;
	/** Per-poll volatility as a fraction of price. */
	vol: number;
	spread: number;
	point: number;
}

const SYMS: Sym[] = [
	{ symbol: 'XAUUSD', digits: 2, mid: 2391.26, vol: 0.00018, spread: 28, point: 0.01 },
	{ symbol: 'EURUSD', digits: 5, mid: 1.08432, vol: 0.00006, spread: 9, point: 0.00001 },
	{ symbol: 'GBPUSD', digits: 5, mid: 1.27118, vol: 0.00007, spread: 12, point: 0.00001 },
	{ symbol: 'USDJPY', digits: 3, mid: 156.234, vol: 0.00007, spread: 14, point: 0.001 },
	{ symbol: 'NAS100', digits: 2, mid: 19842.5, vol: 0.00016, spread: 180, point: 0.01 },
	{ symbol: 'BTCUSD', digits: 2, mid: 67210.4, vol: 0.00035, spread: 1500, point: 0.01 }
];
const BAD_SYMBOL = 'US30';

const state = new Map(SYMS.map((s) => [s.symbol, s.mid]));

function mode(): string | null {
	return new URLSearchParams(location.search).get('mock');
}

const sleep = (ms: number, signal?: AbortSignal) =>
	new Promise<void>((resolve, reject) => {
		const t = setTimeout(resolve, ms);
		signal?.addEventListener('abort', () => {
			clearTimeout(t);
			reject(new ApiError('aborted', 'Request cancelled'));
		});
	});

function gauss(): number {
	return (Math.random() + Math.random() + Math.random() - 1.5) * 1.6;
}

function tick(): PriceTick[] {
	return SYMS.map((s) => {
		const mid = (state.get(s.symbol) ?? s.mid) * (1 + gauss() * s.vol);
		state.set(s.symbol, mid);
		const half = (s.spread * s.point) / 2;
		const bid = round(mid - half, s.digits);
		const ask = round(mid + half, s.digits);
		return {
			symbol: s.symbol,
			bid,
			ask,
			last: 0,
			spread_points: Math.round((ask - bid) / s.point),
			digits: s.digits,
			time: Math.floor(Date.now() / 1000)
		};
	});
}

function round(v: number, d: number): number {
	const m = 10 ** d;
	return Math.round(v * m) / m;
}

function mid(symbol: string): number {
	return state.get(symbol) ?? 0;
}

function positions(): Position[] {
	const now = Math.floor(Date.now() / 1000);
	const xau = mid('XAUUSD');
	const eur = mid('EURUSD');
	const jpy = mid('USDJPY');
	return [
		{
			ticket: 884201337,
			symbol: 'XAUUSD',
			side: 'buy',
			volume: 0.1,
			price_open: 2390.1,
			price_current: round(xau, 2),
			sl: null,
			tp: 2420.0,
			profit: round((xau - 2390.1) * 0.1 * 100, 2),
			swap: 0,
			magic: 0,
			comment: '',
			time: now - 5400
		},
		{
			ticket: 884201552,
			symbol: 'EURUSD',
			side: 'sell',
			volume: 0.5,
			price_open: 1.0861,
			price_current: round(eur, 5),
			sl: 1.09,
			tp: 1.08,
			profit: round((1.0861 - eur) * 0.5 * 100000, 2),
			swap: -1.2,
			magic: 0,
			comment: '',
			time: now - 18000
		},
		{
			ticket: 884202019,
			symbol: 'USDJPY',
			side: 'buy',
			volume: 0.2,
			price_open: 156.41,
			price_current: round(jpy, 3),
			sl: 155.8,
			tp: null,
			profit: round(((jpy - 156.41) * 0.2 * 100000) / jpy, 2),
			swap: 0.4,
			magic: 0,
			comment: '',
			time: now - 900
		}
	];
}

function account(): Account {
	const profit = round(
		positions().reduce((a, p) => a + p.profit, 0),
		2
	);
	const balance = 10000;
	const margin = 230;
	const equity = round(balance + profit, 2);
	return {
		currency: 'USD',
		balance,
		equity,
		profit,
		margin,
		free_margin: round(equity - margin, 2),
		margin_level: round((equity / margin) * 100, 2),
		leverage: 100,
		trade_mode: mode() === 'live' ? 'real' : 'demo'
	};
}

function status(): Status {
	const down = mode() === 'bridge';
	return {
		version: '0.1.0-mock',
		mode: mode() === 'live' ? 'live' : 'demo',
		bridge: down
			? {
					connected: false,
					latency_ms: null,
					ea_version: null,
					protocol_version: null,
					trade_mode: null,
					terminal_connected: null,
					error: 'bridge did not reply within 3000 ms'
				}
			: {
					connected: true,
					latency_ms: 3 + Math.floor(Math.random() * 7),
					ea_version: '0.1.0',
					protocol_version: 1,
					trade_mode: mode() === 'live' ? 'real' : 'demo',
					terminal_connected: true,
					error: null
				},
		llm: {
			default: 'local',
			providers: [
				{ name: 'local', kind: 'ollama', model: 'qwen3.5:9b', available: true },
				{ name: 'claude', kind: 'anthropic', model: 'claude-sonnet-5-5', available: false }
			]
		}
	};
}

const fmt = (v: number, d: number) =>
	v.toLocaleString('en-US', { minimumFractionDigits: d, maximumFractionDigits: d });

function chat(req: ChatRequest): ChatResponse {
	const text = req.message.toLowerCase();
	const tool_calls: ToolCall[] = [];
	let reply: string;

	if (/fail/.test(text)) {
		throw new ApiError('llm_error', 'LLM provider failed: connection refused (mock)', 502);
	} else if (/gold|xau|price|eur|jpy|btc|nas/.test(text)) {
		const symbol = /eur/.test(text)
			? 'EURUSD'
			: /jpy/.test(text)
				? 'USDJPY'
				: /btc/.test(text)
					? 'BTCUSD'
					: /nas/.test(text)
						? 'NAS100'
						: 'XAUUSD';
		const t = tick().find((p) => p.symbol === symbol)!;
		tool_calls.push({ name: 'get_price', arguments: { symbol }, ok: true, duration_ms: 6 });
		reply = `${symbol} is ${fmt(t.bid, t.digits)} bid / ${fmt(t.ask, t.digits)} ask, a spread of ${t.spread_points} points.`;
	} else if (/position|open|trade/.test(text)) {
		const ps = positions();
		const total = ps.reduce((a, p) => a + p.profit, 0);
		tool_calls.push({ name: 'get_positions', arguments: {}, ok: true, duration_ms: 9 });
		reply = `You have ${ps.length} open positions with a floating P/L of ${fmt(total, 2)} USD: ${ps
			.map((p) => `${p.side} ${p.volume} ${p.symbol}`)
			.join(', ')}.`;
	} else if (/equity|balance|account|margin/.test(text)) {
		const a = account();
		tool_calls.push({ name: 'get_account', arguments: {}, ok: true, duration_ms: 5 });
		reply = `Equity is ${fmt(a.equity, 2)} ${a.currency} on a balance of ${fmt(a.balance, 2)}. Free margin is ${fmt(a.free_margin, 2)}.`;
	} else {
		reply =
			"I can read prices, your account and open positions right now. Try asking what gold is at, or to list your positions. Trading arrives in a later sprint.";
	}
	return {
		session_id: req.session_id ?? crypto.randomUUID(),
		reply,
		provider: req.provider ?? 'local',
		tool_calls
	};
}

export async function mockRequest<T>(path: string, opts: RequestOptions): Promise<T> {
	await sleep(40 + Math.random() * 120, opts.signal);
	const m = mode();
	if (m === 'offline') throw new ApiError('network', 'Cannot reach the Jarvis server');

	if (path === '/api/status') return status() as T;
	if (m === 'bridge')
		throw new ApiError('bridge_unavailable', 'bridge did not reply within 3000 ms', 503);

	switch (path) {
		case '/api/account':
			return account() as T;
		case '/api/positions':
			return { positions: positions() } satisfies PositionsResponse as T;
		case '/api/watchlist':
			return {
				prices: tick(),
				errors: [{ symbol: BAD_SYMBOL, message: `unknown symbol: ${BAD_SYMBOL}` }]
			} satisfies WatchlistResponse as T;
		case '/api/chat': {
			const req = opts.body as ChatRequest;
			if (!req?.message || req.message.length > 2000)
				throw new ApiError('bad_request', 'message must be 1-2000 characters', 400);
			await sleep(1200, opts.signal);
			return chat(req) as T;
		}
	}
	throw new ApiError('unknown', `No mock for ${path}`, 404);
}
