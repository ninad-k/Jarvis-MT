/**
 * Typed client for the Arc MT HTTP API. Mirrors docs/api.md exactly.
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

export type Weekday = 'mon' | 'tue' | 'wed' | 'thu' | 'fri' | 'sat' | 'sun';

/** Broker server time, "HH:MM", start inclusive and end exclusive (crates/gate `TradingHours`). */
export interface TradingHours {
	days: Weekday[];
	start: string;
	end: string;
}

export interface Limits {
	max_lot: number;
	max_positions: number;
	max_daily_loss: number;
	allowed_symbols: string[];
	trading_hours: TradingHours | null;
	/** Largest "risk %" a sized order may use (default 2). Absent on a server without risk sizing. */
	max_risk_percent?: number;
}

/** The fields `PUT /api/settings/limits` accepts (all optional); `config` stays the ceiling. */
export type LimitsOverrides = Partial<Limits>;

export interface LimitsResponse {
	config: Limits;
	overrides: LimitsOverrides | null;
	effective: Limits;
}

export interface PersonaTheme {
	accent: string;
	accent_soft: string;
	orb_core: string;
	orb_glow: string;
}

export interface Persona {
	id: string;
	name: string;
	description: string;
	wake_word: string;
	/** False when this persona's voice model file is missing. */
	voice_available: boolean;
	theme: PersonaTheme;
}

export interface PersonasResponse {
	default: string;
	active: string;
	personas: Persona[];
}

export type InputMode = 'hold' | 'toggle' | 'wake_word';
export type VoiceLanguage = 'en' | 'auto';

export interface VoiceSettings {
	read_aloud: boolean;
	input_mode: InputMode;
	language: VoiceLanguage;
}

export interface Settings {
	persona: string;
	/** Provider name from /api/status, or null for the server default. */
	llm_provider: string | null;
	voice: VoiceSettings;
	watchlist: string[];
	/** The user's own spoken-name -> broker-symbol entries (keys lower-case). */
	aliases: Record<string, string>;
	/**
	 * Aliases from the config file, shown read-only. Not in docs/api.md yet: the UI uses it when the
	 * server sends it and hides the list otherwise.
	 */
	config_aliases?: Record<string, string>;
}

/** `PUT /api/settings`: send only what changes. Nested objects are sent whole. */
export type SettingsPatch = Partial<Omit<Settings, 'config_aliases'>>;

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
	/** Effective limits (config tightened by the user's overrides). */
	limits?: Limits;
	/** The active persona. Absent on a server without personas. */
	persona?: { id: string; name: string };
	/** The broker MT5 is logged in to and how many canonical instruments map onto it. Absent on an older server. */
	broker?: StatusBroker | null;
}

export interface StatusBroker {
	company: string;
	server: string;
	mapped: number;
	total: number;
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
	/** The broker's own name for the instrument (`XAUUSD_`). */
	symbol: string;
	/** The canonical name (`XAUUSD`). Absent on a server without symbol mapping. */
	canonical?: string;
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
	canonical?: string;
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
	canonical?: string;
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

/** How a risk-sized order got its lot size (`POST /api/market/prepare` with `risk_percent`). */
export interface PreviewSizing {
	risk_percent: number;
	/** What the stop would lose, in the account currency. */
	risk_amount: number;
	currency: string;
	/** Entry to stop, in price units. */
	stop_distance: number;
	volume: number;
}

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
	/** Present when the lot size came from a risk percentage. */
	sizing?: PreviewSizing | null;
}

export interface ChatResponse {
	session_id: string;
	reply: string;
	provider: string;
	tool_calls: ToolCall[];
	preview?: PreviewDto | null;
	/** Persona that answered (id). Absent on a server without personas. */
	persona?: string;
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

/** `POST /api/voice/transcribe?wake=1` adds this. */
export interface WakeResult {
	detected: boolean;
	persona: string | null;
	/** What was said after the wake phrase; may be empty. */
	command: string;
}

export interface TranscribeResponse {
	text: string;
	raw: string;
	audio_ms: number;
	elapsed_ms: number;
	intent: VoiceIntent;
	wake?: WakeResult;
}

// ---- Broker symbol mapping (docs/api.md "Broker symbol mapping") ------------

/** Where a mapping came from. The UI words these as user / curated / auto / pattern / unchanged. */
export type SymbolSource = 'user' | 'curated' | 'generated' | 'pattern' | 'passthrough';

export interface BrokerInfo {
	company: string;
	server: string;
	/** ISO time of the last detection. */
	detected_at: string;
	catalog_size: number;
	detected_suffix: string;
	detected_prefix: string;
}

export interface SymbolCandidate {
	broker_symbol: string;
	description: string;
	score: number;
	reasons: string[];
}

export interface SymbolMapping {
	canonical: string;
	/** metal | index | energy | crypto | fx (anything else is shown under "Other"). */
	class: string;
	aliases: string[];
	broker_symbol: string;
	source: SymbolSource;
	/** False when the resolved name is not in this broker's catalog: the entry needs attention. */
	available: boolean;
	candidates: SymbolCandidate[];
}

export interface SymbolsResponse {
	/** Null while the bridge has never been reachable. */
	broker: BrokerInfo | null;
	mappings: SymbolMapping[];
	/** Canonical names with no match on this broker. */
	unmapped: string[];
}

export interface CatalogSymbol {
	name: string;
	description: string;
	path: string;
	trade_mode: string | number;
	visible: boolean;
}

export interface CatalogResponse {
	symbols: CatalogSymbol[];
	total: number;
}

/** `PUT /api/symbols/map`; `broker_symbol: null` removes the override. */
export interface SymbolMapBody {
	canonical: string;
	broker_symbol: string | null;
	totp?: string;
}

// ---- Market view (docs/api.md "Market view"; tasks/spec-market-view.md section 4) -------------

export type Bias = 'bullish' | 'bearish' | 'neutral';
export type EventImportance = 'low' | 'moderate' | 'high';

export interface MarketSetup {
	side: Side;
	entry_low: number;
	entry_high: number;
	stop_loss: number;
	/** TP1. */
	take_profit: number;
	take_profit_2: number | null;
	risk_reward: number;
	atr_h1: number;
	atr_m15: number;
}

export interface MarketTimeframe {
	/** M5, M15, H1, H4 or D1. */
	timeframe: string;
	/** `up`, `down` or anything else for flat. */
	trend: string;
	ema20: number;
	ema50: number;
	rsi: number;
	macd_hist: number;
	atr: number;
}

export interface MarketLevels {
	pivot: number | null;
	r1: number | null;
	r2: number | null;
	s1: number | null;
	s2: number | null;
	prev_day_high: number | null;
	prev_day_low: number | null;
	vwap: number | null;
	support: number[];
	resistance: number[];
}

export interface MarketFactor {
	name: string;
	signal: string;
	/** Signed points this factor added to the score. */
	points: number;
	detail: string;
}

export interface MarketEvent {
	/** ISO UTC. */
	time: string;
	/** Whole minutes from the moment the view was made; negative once it has passed. */
	minutes_away: number;
	currency: string;
	title: string;
	importance: EventImportance;
	/** The calendar endpoint may add the figures; the view leaves them out. */
	actual?: number | null;
	forecast?: number | null;
	previous?: number | null;
	unit?: string | null;
}

export interface MarketHeadline {
	title: string;
	source: string;
	url: string;
	/** ISO UTC. */
	published_at: string;
	/** -1 to 1, already in the instrument's direction. */
	tone: number;
}

export interface MarketNews {
	configured: boolean;
	score: number | null;
	label: string | null;
	count: number;
	headlines: MarketHeadline[];
}

export interface MarketViewDto {
	id: string;
	/** The canonical name (`XAUUSD`). */
	symbol: string;
	/** The broker's own name for it (`XAUUSD_`). */
	broker_symbol: string;
	/** ISO UTC. */
	generated_at: string;
	valid_until: string;
	price: { bid: number; ask: number };
	bias: Bias;
	/** 0 to 100. */
	confidence: number;
	score: number;
	stand_aside: boolean;
	stand_aside_reason: string | null;
	setup: MarketSetup | null;
	timeframes: MarketTimeframe[];
	levels: MarketLevels;
	factors: MarketFactor[];
	events: MarketEvent[];
	news: MarketNews;
	summary: string;
	disclaimer: string;
	/** Price decimals. Not in the spec; the UI uses it when the server sends it and infers it otherwise. */
	digits?: number;
}

export interface MarketCalendarResponse {
	events: MarketEvent[];
}

export interface MarketNewsSummary {
	score: number;
	label: string;
	count: number;
	headlines: MarketHeadline[];
}

export interface MarketNewsResponse {
	configured: boolean;
	provider: string | null;
	summary: MarketNewsSummary | null;
}

export type MarketOutcome = 'open' | 'tp_hit' | 'sl_hit' | 'expired' | 'no_setup';

/**
 * Counts over every stored view. The spec lists only "totals per outcome, TP rate among setups,
 * bias accuracy", so every field is optional here and the UI works from the counts.
 */
export interface MarketJournalStats {
	total?: number;
	open?: number;
	tp_hit?: number;
	sl_hit?: number;
	expired?: number;
	no_setup?: number;
	/** Views whose bias was scored, split by the verdict. */
	bias_correct?: number;
	bias_wrong?: number;
}

export interface MarketJournalView {
	id: string;
	/** ISO UTC. */
	created_at: string;
	symbol: string;
	broker_symbol?: string;
	bias: Bias;
	confidence: number;
	score?: number;
	stand_aside: boolean;
	side: Side | null;
	entry: number | null;
	stop: number | null;
	tp: number | null;
	price: number;
	valid_until: string;
	outcome: MarketOutcome;
	evaluated_at: string | null;
	bias_correct: boolean | null;
	/** The move at `valid_until` in ATR(H1) units, signed in the bias direction. */
	move_atr: number | null;
}

export interface MarketJournalResponse {
	stats: MarketJournalStats;
	views: MarketJournalView[];
}

export type MarketOrderType = 'market' | 'limit';

/** Either a lot size or a risk percentage (never both): the server works the lots out from the stop. */
export type MarketPrepareBody = { view_id: string; order_type: MarketOrderType } & (
	| { volume: number }
	| { risk_percent: number }
);

// ---- Price alerts (tasks/spec-assistant-actions.md section 2) --------------------------------------

export type AlertDirection = 'above' | 'below';
export type AlertStatus = 'active' | 'triggered' | 'cancelled';

export interface PriceAlert {
	id: string;
	/** The canonical name (`XAUUSD`). */
	symbol: string;
	broker_symbol: string;
	direction: AlertDirection;
	price: number;
	note: string | null;
	status: AlertStatus;
	/** ISO UTC. */
	created_at: string;
	triggered_at: string | null;
	trigger_price: number | null;
}

export interface AlertsResponse {
	active: PriceAlert[];
	/** The most recent triggered alerts. */
	recent: PriceAlert[];
}

export interface AlertCreateBody {
	symbol: string;
	price: number;
	direction?: AlertDirection;
	note?: string;
}

export interface AlertTriggeredEvent {
	type: 'alert_triggered';
	id: string;
	symbol: string;
	direction: AlertDirection;
	price: number;
	trigger_price: number;
	note: string | null;
	/** ISO UTC. */
	triggered_at: string;
}

// ---- Daily briefing (tasks/spec-assistant-actions.md section 4) ------------------------------------

export interface BriefingView {
	symbol: string;
	broker_symbol: string;
	bias: Bias;
	confidence: number;
	stand_aside: boolean;
	stand_aside_reason: string | null;
	summary: string;
	/** Why this symbol has no view (the budget ran out, the bridge failed); the other fields are then placeholders. */
	unavailable?: string;
}

export interface BriefingDto {
	/** ISO UTC. */
	generated_at: string;
	account: Account;
	positions: Position[];
	/** Today's events for the watchlist currencies. The figures are filled in once released. */
	events: MarketEvent[];
	views: BriefingView[];
	summary: string;
}

/** Events pushed over `GET /api/ws`. */
export type WsEvent =
	| { type: 'kill_switch'; engaged: boolean; reason: string | null }
	| { type: 'preview_expired'; token: string }
	| { type: 'trade_executed'; preview_id: string; results: TradeResult[] }
	| { type: 'positions_changed' }
	| { type: 'settings_changed' }
	| { type: 'symbol_map_changed'; broker?: string }
	| AlertTriggeredEvent;

// ---- Transport -----------------------------------------------------------

export const MOCK = import.meta.env.VITE_MOCK === '1';

export interface RequestOptions {
	method?: 'GET' | 'POST' | 'PUT' | 'DELETE';
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
		throw new ApiError('network', 'Cannot reach the Arc MT server');
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
const put = <T>(path: string, body?: unknown, extra: RequestOptions = {}) =>
	request<T>(path, { method: 'PUT', body, ...extra });

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

	/** `wake: true` is hands-free mode: the reply carries a `wake` result. */
	transcribe: (wav: Blob, signal?: AbortSignal, opts: { wake?: boolean } = {}) =>
		request<TranscribeResponse>(`/api/voice/transcribe${opts.wake ? '?wake=1' : ''}`, {
			method: 'POST',
			blob: wav,
			signal,
			timeoutMs: 60_000
		}),
	/** `persona` previews another persona's voice (settings page). */
	speak: (text: string, signal?: AbortSignal, persona?: string) =>
		request<Blob>('/api/voice/speak', {
			method: 'POST',
			body: persona ? { text, persona } : { text },
			asBlob: true,
			signal,
			timeoutMs: 60_000
		}),

	personas: (signal?: AbortSignal) => request<PersonasResponse>('/api/personas', { signal }),
	settings: (signal?: AbortSignal) => request<Settings>('/api/settings', { signal }),
	updateSettings: (patch: SettingsPatch) => put<Settings>('/api/settings', patch),

	limits: (signal?: AbortSignal) => request<LimitsResponse>('/api/settings/limits', { signal }),
	/** A wrong TOTP code is a 401 that is an answer, not a lost session. */
	setLimits: (body: LimitsOverrides & { totp?: string }) =>
		put<LimitsResponse | Limits>('/api/settings/limits', body, { keepOn401: true }),
	resetLimits: (totp?: string) =>
		request<LimitsResponse | void>('/api/settings/limits', {
			method: 'DELETE',
			body: totp ? { totp } : {},
			keepOn401: true
		}),

	symbols: (signal?: AbortSignal) => request<SymbolsResponse>('/api/symbols', { signal }),
	/** Re-reads the broker's catalog and re-runs the auto-mapper; a slow broker can take a while. */
	detectSymbols: (signal?: AbortSignal) =>
		post<SymbolsResponse>('/api/symbols/detect', undefined, { signal, timeoutMs: 60_000 }),
	symbolCatalog: (q: string, limit = 30, signal?: AbortSignal) => {
		const params = new URLSearchParams({ q, limit: String(limit) });
		return request<CatalogResponse>(`/api/symbols/catalog?${params}`, { signal });
	},
	/** A wrong TOTP code is a 401 that is an answer, not a lost session. */
	setSymbolMap: (body: SymbolMapBody) => put<unknown>('/api/symbols/map', body, { keepOn401: true }),
	/** The effective map as a JSON file. */
	exportSymbols: () => request<Blob>('/api/symbols/export', { asBlob: true }),

	/** The server reads rates, the calendar and news before it answers, so allow for a slow broker. */
	marketView: (symbol: string, signal?: AbortSignal) =>
		request<MarketViewDto>(`/api/market/view?${new URLSearchParams({ symbol })}`, {
			signal,
			timeoutMs: 45_000
		}),
	marketCalendar: (opts: { hours?: number; currencies?: string[] } = {}, signal?: AbortSignal) => {
		const q = new URLSearchParams({ hours: String(opts.hours ?? 24) });
		if (opts.currencies?.length) q.set('currencies', opts.currencies.join(','));
		return request<MarketCalendarResponse>(`/api/market/calendar?${q}`, { signal });
	},
	marketNews: (symbol: string, signal?: AbortSignal) =>
		request<MarketNewsResponse>(`/api/market/news?${new URLSearchParams({ symbol })}`, {
			signal,
			timeoutMs: 20_000
		}),
	marketJournal: (opts: { limit?: number; symbol?: string } = {}, signal?: AbortSignal) => {
		const q = new URLSearchParams({ limit: String(opts.limit ?? 20) });
		if (opts.symbol) q.set('symbol', opts.symbol);
		return request<MarketJournalResponse>(`/api/market/journal?${q}`, { signal });
	},
	/** Turns a view into the usual preview. It never confirms: the confirmation modal does that. */
	marketPrepare: (body: MarketPrepareBody) => post<PreviewDto>('/api/market/prepare', body),

	alerts: (signal?: AbortSignal) => request<AlertsResponse>('/api/alerts', { signal }),
	createAlert: (body: AlertCreateBody) => post<PriceAlert>('/api/alerts', body),
	/** The server answers 204 or the cancelled alert; both mean it is gone. */
	cancelAlert: (id: string) =>
		request<PriceAlert | void>(`/api/alerts/${encodeURIComponent(id)}`, { method: 'DELETE' }),

	/** Builds a view for every watchlist symbol (up to 20 s each, concurrently), so allow for it. */
	briefing: (signal?: AbortSignal) => request<BriefingDto>('/api/briefing', { signal, timeoutMs: 60_000 })
};
