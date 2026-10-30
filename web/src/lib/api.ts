/**
 * Typed client for the Jarvis HTTP API. Mirrors docs/api.md exactly.
 * In mock mode (VITE_MOCK=1) every call is answered by ./mock instead of the network.
 */

// ---- Types (docs/api.md) -------------------------------------------------

/** Server mode. "demo" is the safe default; anything else is treated as LIVE by the UI. */
export type Mode = 'demo' | 'live';
/** MT5 account trade mode as reported by the bridge. */
export type TradeMode = 'demo' | 'contest' | 'real';

export interface BridgeStatus {
	connected: boolean;
	latency_ms: number | null;
	ea_version: string | null;
	protocol_version: number | null;
	trade_mode: TradeMode | null;
	terminal_connected: boolean | null;
	error: string | null;
}

export interface LlmProvider {
	name: string;
	kind: string;
	model: string;
	available: boolean;
}

export interface Status {
	version: string;
	mode: Mode;
	bridge: BridgeStatus;
	llm: { default: string; providers: LlmProvider[] };
}

export interface Account {
	currency: string;
	balance: number;
	equity: number;
	profit: number;
	margin: number;
	free_margin: number;
	margin_level: number;
	leverage: number;
	trade_mode: TradeMode;
}

export type Side = 'buy' | 'sell';

export interface Position {
	ticket: number;
	symbol: string;
	side: Side;
	volume: number;
	price_open: number;
	price_current: number;
	sl: number | null;
	tp: number | null;
	profit: number;
	swap: number;
	magic: number;
	comment: string;
	/** Unix seconds. */
	time: number;
}

export interface PositionsResponse {
	positions: Position[];
}

export interface PriceTick {
	symbol: string;
	bid: number;
	ask: number;
	last: number;
	spread_points: number;
	digits: number;
	/** Unix seconds. */
	time: number;
}

export interface SymbolError {
	symbol: string;
	message: string;
}

export interface WatchlistResponse {
	prices: PriceTick[];
	errors: SymbolError[];
}

export interface ChatRequest {
	session_id?: string;
	message: string;
	provider?: string;
}

export interface ToolCall {
	name: string;
	arguments: Record<string, unknown>;
	ok: boolean;
	duration_ms: number;
}

export interface ChatResponse {
	session_id: string;
	reply: string;
	provider: string;
	tool_calls: ToolCall[];
}

export type ApiErrorCode =
	| 'bad_request'
	| 'bridge_unavailable'
	| 'bridge_error'
	| 'llm_error'
	| 'internal';

export interface ApiErrorBody {
	error: { code: ApiErrorCode; message: string };
}

/** Client-side codes for failures that never produced a server error body. */
export type ClientErrorCode = 'network' | 'timeout' | 'aborted' | 'unknown';

export class ApiError extends Error {
	readonly code: ApiErrorCode | ClientErrorCode;
	readonly status: number;

	constructor(code: ApiErrorCode | ClientErrorCode, message: string, status = 0) {
		super(message);
		this.name = 'ApiError';
		this.code = code;
		this.status = status;
	}

	/** The Jarvis server itself could not be reached (as opposed to the MT5 bridge). */
	get isOffline(): boolean {
		return this.code === 'network';
	}
}

// ---- Transport -----------------------------------------------------------

export const MOCK = import.meta.env.VITE_MOCK === '1';

const SERVER_CODES: readonly string[] = [
	'bad_request',
	'bridge_unavailable',
	'bridge_error',
	'llm_error',
	'internal'
];

interface RequestOptions {
	method?: 'GET' | 'POST';
	body?: unknown;
	signal?: AbortSignal;
	timeoutMs?: number;
}

function isErrorBody(v: unknown): v is ApiErrorBody {
	if (typeof v !== 'object' || v === null || !('error' in v)) return false;
	const e = (v as { error: unknown }).error;
	return (
		typeof e === 'object' &&
		e !== null &&
		typeof (e as { code?: unknown }).code === 'string' &&
		typeof (e as { message?: unknown }).message === 'string'
	);
}

async function networkRequest<T>(path: string, opts: RequestOptions): Promise<T> {
	const timeout = AbortSignal.timeout(opts.timeoutMs ?? 10_000);
	const signal = opts.signal ? AbortSignal.any([opts.signal, timeout]) : timeout;

	let res: Response;
	try {
		res = await fetch(path, {
			method: opts.method ?? 'GET',
			headers: opts.body === undefined ? undefined : { 'content-type': 'application/json' },
			body: opts.body === undefined ? undefined : JSON.stringify(opts.body),
			signal,
			cache: 'no-store'
		});
	} catch {
		if (opts.signal?.aborted) throw new ApiError('aborted', 'Request cancelled');
		if (timeout.aborted) throw new ApiError('timeout', 'The server took too long to respond');
		throw new ApiError('network', 'Cannot reach the Jarvis server');
	}

	let json: unknown = undefined;
	let parsed = false;
	try {
		json = await res.json();
		parsed = true;
	} catch {
		parsed = false;
	}

	if (!res.ok) {
		if (parsed && isErrorBody(json)) {
			const code = SERVER_CODES.includes(json.error.code) ? json.error.code : 'unknown';
			throw new ApiError(code as ApiErrorCode | 'unknown', json.error.message, res.status);
		}
		// A 5xx without our error shape is a dead proxy target, not an application error.
		if (res.status >= 500) throw new ApiError('network', 'Cannot reach the Jarvis server', res.status);
		throw new ApiError('unknown', `Unexpected response (HTTP ${res.status})`, res.status);
	}
	if (!parsed) {
		// e.g. the SPA fallback page answering /api/* when no backend is wired up.
		throw new ApiError('network', 'The server returned a non-JSON response', res.status);
	}
	return json as T;
}

async function request<T>(path: string, opts: RequestOptions = {}): Promise<T> {
	if (MOCK) {
		const { mockRequest } = await import('./mock');
		return mockRequest<T>(path, opts);
	}
	return networkRequest<T>(path, opts);
}

// ---- Endpoints -----------------------------------------------------------

export const api = {
	status: (signal?: AbortSignal) => request<Status>('/api/status', { signal }),
	account: (signal?: AbortSignal) => request<Account>('/api/account', { signal }),
	positions: (signal?: AbortSignal) => request<PositionsResponse>('/api/positions', { signal }),
	watchlist: (signal?: AbortSignal) => request<WatchlistResponse>('/api/watchlist', { signal }),
	/** LLM turns can be slow on local models, so allow a generous timeout. */
	chat: (body: ChatRequest, signal?: AbortSignal) =>
		request<ChatResponse>('/api/chat', { method: 'POST', body, signal, timeoutMs: 180_000 })
};

export type { RequestOptions };
