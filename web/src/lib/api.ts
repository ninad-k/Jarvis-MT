/**
 * Typed client for the Jarvis HTTP API. Mirrors docs/api.md exactly.
 * In mock mode (VITE_MOCK=1) every call is answered by ./mock instead of the network.
 */
import {
	ApiError,
	errorFromResponse,
	type ApiErrorBody,
	type ApiErrorCode,
	type ClientErrorCode
} from './errors';

export { ApiError };
export type { ApiErrorBody, ApiErrorCode, ClientErrorCode };

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

export interface KillSwitchState {
	engaged: boolean;
	reason: string | null;
	changed_at_ms: number | null;
}

export interface AuthStatus {
	enabled: boolean;
	username: string | null;
}

export interface VoiceStatus {
	stt: boolean;
	tts: boolean;
}

export interface Limits {
	max_lot: number;
	max_positions: number;
	max_daily_loss: number;
	allowed_symbols: string[];
	trading_hours: unknown;
}

export interface Status {
	version: string;
	mode: Mode;
	mode_mismatch?: boolean;
	bridge: BridgeStatus;
	llm: { default: string; providers: LlmProvider[] };
	/** Sprint 2-5 fields. Absent on an older server, which the UI treats as "feature off". */
	kill_switch?: KillSwitchState;
	auth?: AuthStatus;
	voice?: VoiceStatus;
	limits?: Limits;
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

export type OrderType =
	| 'buy_limit'
	| 'sell_limit'
	| 'buy_stop'
	| 'sell_stop'
	| 'buy_stop_limit'
	| 'sell_stop_limit';

/** A pending order (bridge `get_orders`). */
export interface PendingOrder {
	ticket: number;
	symbol: string;
	type: OrderType;
	volume_initial: number;
	volume_current: number;
	price_open: number;
	sl: number | null;
	tp: number | null;
	/** Unix seconds. */
	time_setup: number;
	comment: string;
}

export interface OrdersResponse {
	orders: PendingOrder[];
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

// ---- Chat, previews, confirmation ----------------------------------------

export interface ChatRequest {
	session_id?: string;
	message: string;
	provider?: string;
	/** The turn came from the microphone: previews use `after_readback` timing. */
	voice?: boolean;
}

export interface ToolCall {
	name: string;
	arguments: Record<string, unknown>;
	ok: boolean;
	duration_ms: number;
}

export interface EstPnl {
	current: number | null;
	at_sl: number | null;
	at_tp: number | null;
}

export type PreviewStage = 'first' | 'second';
export type PreviewTiming = 'immediate' | 'after_readback';

export interface PreviewDto {
	id: string;
	description: string;
	symbols: string[];
	tickets: number[];
	total_volume: number;
	est_pnl: EstPnl;
	is_batch: boolean;
	count: number;
	warnings: string[];
	token: string;
	stage: PreviewStage;
	expires_at_ms: number;
	timing: PreviewTiming;
}

export interface ChatResponse {
	session_id: string;
	reply: string;
	provider: string;
	tool_calls: ToolCall[];
	preview?: PreviewDto | null;
}

export interface TradeResult {
	ticket: number;
	ok: boolean;
	retcode: number;
	retcode_description: string;
	order?: number | null;
	deal?: number | null;
	price?: number | null;
	volume?: number | null;
	error?: string | null;
}

export interface ConfirmExecuted {
	status: 'executed';
	preview_id: string;
	results: TradeResult[];
}

export interface ConfirmNeedsSecond {
	status: 'needs_second_confirmation';
	preview_id: string;
	token: string;
	stage: 'second';
	expires_at_ms: number;
	count: number;
	message: string;
}

export type ConfirmResponse = ConfirmExecuted | ConfirmNeedsSecond;

export interface ClosePositionsRequest {
	symbol: string | null;
	side: Side | null;
	all: boolean;
}

// ---- Auth, audit, voice ---------------------------------------------------

export interface LoginRequest {
	username: string;
	password: string;
	totp: string;
}

export interface Session {
	username: string;
	/** Unix seconds or ms; only displayed loosely, never used for security decisions. */
	expires_at: number | string;
}

export interface AuditEntry {
	id: number;
	/** Unix ms or an ISO string; use `auditTime()` in format.ts. */
	ts: number | string;
	session_id: string | null;
	kind: string;
	payload: unknown;
}

export interface AuditResponse {
	entries: AuditEntry[];
}

export type VoiceIntent = 'confirm' | 'cancel' | 'other';

export interface TranscribeResponse {
	text: string;
	raw: string;
	audio_ms: number;
	elapsed_ms: number;
	intent: VoiceIntent;
}

/** Events pushed over `GET /api/ws`. */
export type WsEvent =
	| { type: 'kill_switch'; engaged: boolean; reason: string | null }
	| { type: 'preview_expired'; token: string }
	| { type: 'trade_executed'; preview_id: string; results: TradeResult[] }
	| { type: 'positions_changed' };

// ---- Transport -----------------------------------------------------------

export const MOCK = import.meta.env.VITE_MOCK === '1';

export interface RequestOptions {
	method?: 'GET' | 'POST';
	/** JSON body. */
	body?: unknown;
	/** Binary body (voice upload); sent with its own content type. */
	blob?: Blob;
	/** Expect a binary response (voice playback). */
	asBlob?: boolean;
	signal?: AbortSignal;
	timeoutMs?: number;
	/** A 401 is an answer here (wrong login code), not a reason to leave for /login. */
	keepOn401?: boolean;
}

let onUnauthorized: (() => void) | null = null;

/** The auth store registers here: any 401 outside `keepOn401` routes the user to /login. */
export function setUnauthorizedHandler(fn: (() => void) | null): void {
	onUnauthorized = fn;
}

async function networkRequest<T>(path: string, opts: RequestOptions): Promise<T> {
	const timeout = AbortSignal.timeout(opts.timeoutMs ?? 10_000);
	const signal = opts.signal ? AbortSignal.any([opts.signal, timeout]) : timeout;

	const headers: Record<string, string> = {};
	let body: BodyInit | undefined;
	if (opts.blob) {
		headers['content-type'] = opts.blob.type || 'application/octet-stream';
		body = opts.blob;
	} else if (opts.body !== undefined) {
		headers['content-type'] = 'application/json';
		body = JSON.stringify(opts.body);
	}

	let res: Response;
	try {
		res = await fetch(path, {
			method: opts.method ?? 'GET',
			headers,
			body,
			signal,
			cache: 'no-store',
			credentials: 'same-origin'
		});
	} catch {
		if (opts.signal?.aborted) throw new ApiError('aborted', 'Request cancelled');
		if (timeout.aborted) throw new ApiError('timeout', 'The server took too long to respond');
		throw new ApiError('network', 'Cannot reach the Jarvis server');
	}

	if (!res.ok) {
		let json: unknown;
		try {
			json = await res.json();
		} catch {
			json = undefined;
		}
		throw errorFromResponse(res.status, json, res.headers.get('retry-after'));
	}
	if (res.status === 204) return undefined as T;
	if (opts.asBlob) return (await res.blob()) as T;

	try {
		return (await res.json()) as T;
	} catch {
		// e.g. the SPA fallback page answering /api/* when no backend is wired up.
		throw new ApiError('network', 'The server returned a non-JSON response', res.status);
	}
}

async function request<T>(path: string, opts: RequestOptions = {}): Promise<T> {
	try {
		// The test is written out here (not via MOCK) so Vite folds it to `false` at build time and
		// leaves the mock module out of production builds entirely (audit WEB-1).
		if (import.meta.env.VITE_MOCK === '1') {
			const { mockRequest } = await import('./mock');
			return await mockRequest<T>(path, opts);
		}
		return await networkRequest<T>(path, opts);
	} catch (e) {
		if (e instanceof ApiError && e.isUnauthorized && !opts.keepOn401) onUnauthorized?.();
		throw e;
	}
}

const post = <T>(path: string, body?: unknown, extra: RequestOptions = {}) =>
	request<T>(path, { method: 'POST', body, ...extra });

// ---- Endpoints -----------------------------------------------------------

export const api = {
	status: (signal?: AbortSignal) => request<Status>('/api/status', { signal }),
	account: (signal?: AbortSignal) => request<Account>('/api/account', { signal }),
	positions: (signal?: AbortSignal) => request<PositionsResponse>('/api/positions', { signal }),
	/**
	 * Pending orders. docs/api.md lists no read endpoint for them; `GET /api/orders` returning
	 * `{orders}` is assumed (bridge `get_orders` shape). A server without it answers 404.
	 */
	orders: (signal?: AbortSignal) => request<OrdersResponse>('/api/orders', { signal }),
	watchlist: (signal?: AbortSignal) => request<WatchlistResponse>('/api/watchlist', { signal }),
	/** LLM turns can be slow on local models, so allow a generous timeout. */
	chat: (body: ChatRequest, signal?: AbortSignal) =>
		post<ChatResponse>('/api/chat', body, { signal, timeoutMs: 180_000 }),

	confirm: (token: string) =>
		post<ConfirmResponse>(`/api/confirm/${encodeURIComponent(token)}`, undefined, {
			// Batch closes run sequentially on the server; allow for slow brokers.
			timeoutMs: 60_000
		}),
	cancel: (token: string) => post<void>(`/api/cancel/${encodeURIComponent(token)}`),
	readback: (token: string) =>
		post<{ expires_at_ms: number }>(`/api/readback/${encodeURIComponent(token)}`),
	closePosition: (ticket: number, volume: number | null) =>
		post<PreviewDto>(`/api/positions/${ticket}/close`, { volume }),
	closePositions: (body: ClosePositionsRequest) => post<PreviewDto>('/api/positions/close', body),
	cancelOrder: (ticket: number) => post<PreviewDto>(`/api/orders/${ticket}/cancel`),

	killSwitch: (signal?: AbortSignal) => request<KillSwitchState>('/api/killswitch', { signal }),
	setKillSwitch: (body: { engaged: boolean; reason?: string; totp?: string }) =>
		post<KillSwitchState>('/api/killswitch', body, { keepOn401: body.engaged === false }),

	login: (body: LoginRequest) => post<Session>('/api/auth/login', body, { keepOn401: true }),
	logout: () => post<void>('/api/auth/logout', undefined, { keepOn401: true }),
	me: (signal?: AbortSignal) => request<Session>('/api/auth/me', { signal, keepOn401: true }),

	audit: (opts: { limit?: number; beforeId?: number | null } = {}, signal?: AbortSignal) => {
		const q = new URLSearchParams({ limit: String(opts.limit ?? 50) });
		if (opts.beforeId != null) q.set('before_id', String(opts.beforeId));
		return request<AuditResponse>(`/api/audit?${q}`, { signal });
	},

	transcribe: (wav: Blob, signal?: AbortSignal) =>
		request<TranscribeResponse>('/api/voice/transcribe', {
			method: 'POST',
			blob: wav,
			signal,
			timeoutMs: 60_000
		}),
	speak: (text: string, signal?: AbortSignal) =>
		request<Blob>('/api/voice/speak', {
			method: 'POST',
			body: { text },
			asBlob: true,
			signal,
			timeoutMs: 60_000
		})
};
