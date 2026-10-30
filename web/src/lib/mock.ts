/**
 * Mock backend for UI development (VITE_MOCK=1, see `pnpm dev:mock`).
 * Never bundled into production builds: api.ts imports it dynamically behind a build-time constant.
 *
 * URL switches (mock mode only), comma separated and remembered for the browser session:
 *   ?mock=live          LIVE mode + real account (banner, red badge)
 *   ?mock=bridge        API up, MT5 bridge down
 *   ?mock=offline       API unreachable
 *   ?mock=auth          sign-in required (any user + password, TOTP 123456; also needed for limits changes)
 *   ?mock=novoice       STT/TTS reported as unavailable
 *   ?mock=unknown       every execute ends in outcome_unknown
 *   ?mock=settingsfail  saving settings fails (shows the optimistic rollback)
 *   ?mock=staleconfig   the limits page is told a looser config than the gate enforces (shows server 422s)
 *   ?mock=ttl=4         confirmation window of 4 s instead of 15 s
 *   ?mock=reset         forget the remembered switches
 *
 * Chat: "close gold", "close all", "close buys", "close half of gold", "buy 0.1 gold sl 2300",
 * "cancel my pending orders". Messages containing "fail" return an llm_error.
 * Voice: the mock transcriber returns a scripted phrase; `window.__jarvisMock.say("...")` sets it.
 * Hands-free (`/api/voice/transcribe?wake=1`): script "hey friday what's gold at" and the reply carries
 * `wake` (and switches the persona); without a script the segment is ignored background speech.
 * Settings, personas and limits are remembered for the browser session (tighten-only; TOTP 123456 with `auth`).
 */
import {
	ApiError,
	type Account,
	type ChatRequest,
	type ChatResponse,
	type LoginRequest,
	type OrdersResponse,
	type PositionsResponse,
	type PreviewDto,
	type RequestOptions,
	type Session,
	type Status,
	type ToolCall,
	type TranscribeResponse,
	type VoiceIntent,
	type WatchlistResponse
} from './api';
import { book, pending, positions, round, seedBook, symbolFromText, tick } from './mock-market';
import {
	activePersona,
	deleteLimits,
	detectWake,
	effectiveLimits,
	getLimits,
	getPersonas,
	getSettings,
	putLimits,
	putSettings,
	resolveSymbol,
	setPersona
} from './mock-settings';
import {
	assertWritable,
	cancelOrdersPreview,
	cancelToken,
	closePreview,
	config,
	confirm,
	hasLivePreview,
	killState,
	emitMockEvent,
	onMockEvent,
	openPreview,
	readback,
	setKill
} from './mock-trading';
import { toneWav, wavDurationMs } from './voice/wav';

export { onMockEvent };

// ---- Switches -------------------------------------------------------------

const FLAG_KEY = 'jarvis.mock.flags';

function flags(): Set<string> {
	let raw: string | null = null;
	try {
		const q = new URLSearchParams(location.search).get('mock');
		if (q === 'reset') sessionStorage.removeItem(FLAG_KEY);
		else if (q !== null) sessionStorage.setItem(FLAG_KEY, q);
		raw = q === 'reset' ? null : (q ?? sessionStorage.getItem(FLAG_KEY));
	} catch {
		raw = new URLSearchParams(location.search).get('mock');
	}
	return new Set((raw ?? '').split(',').filter(Boolean));
}

const flag = (name: string) => flags().has(name);

function applyConfig(): void {
	const ttl = [...flags()].find((f) => f.startsWith('ttl='));
	config.ttlMs = ttl ? Math.max(1, Number(ttl.slice(4))) * 1000 : 15_000;
	config.capMs = Math.max(config.ttlMs * 2, 30_000);
	config.outcomeUnknown = flag('unknown');
}

// ---- Auth -----------------------------------------------------------------

const SESSION_KEY = 'jarvis.mock.session';
const MOCK_TOTP = '123456';

function session(): Session | null {
	try {
		const raw = sessionStorage.getItem(SESSION_KEY);
		return raw ? (JSON.parse(raw) as Session) : null;
	} catch {
		return null;
	}
}

function login(body: LoginRequest): Session {
	if (!body?.username || !body.password || body.totp !== MOCK_TOTP)
		throw new ApiError('unauthorized', 'invalid credentials', 401);
	const s: Session = { username: body.username, expires_at: Date.now() + 8 * 3600_000 };
	try {
		sessionStorage.setItem(SESSION_KEY, JSON.stringify(s));
	} catch {
		/* storage unavailable: the mock session just won't survive a reload */
	}
	return s;
}

// ---- Read endpoints ---------------------------------------------------------

function account(): Account {
	const profit = round(positions().reduce((a, p) => a + p.profit, 0), 2);
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
		trade_mode: flag('live') ? 'real' : 'demo'
	};
}

function status(): Status {
	const down = flag('bridge');
	const live = flag('live');
	const s = session();
	return {
		version: '0.1.0-mock',
		mode: live ? 'live' : 'demo',
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
					trade_mode: live ? 'real' : 'demo',
					terminal_connected: true,
					error: null
				},
		llm: {
			default: 'local',
			providers: [
				{ name: 'local', kind: 'ollama', model: 'qwen3.5:9b', available: true },
				{ name: 'cloud', kind: 'openai', model: 'gpt-5', available: false }
			]
		},
		kill_switch: killState(),
		auth: { enabled: flag('auth'), username: s?.username ?? null },
		voice: { stt: !flag('novoice'), tts: !flag('novoice') },
		limits: effectiveLimits(),
		persona: { id: activePersona().id, name: activePersona().name }
	};
}

const PROVIDERS = [
	{ name: 'local', available: true },
	{ name: 'cloud', available: false }
];

/** Prices for the user's watchlist (aliases resolved); an entry the market doesn't know is reported, not fatal. */
function watchlist(): WatchlistResponse {
	const ticks = tick();
	const prices: WatchlistResponse['prices'] = [];
	const errors: WatchlistResponse['errors'] = [];
	for (const entry of getSettings().watchlist) {
		const symbol = resolveSymbol(entry);
		const t = symbol ? ticks.find((p) => p.symbol === symbol) : undefined;
		if (t) prices.push(t);
		else errors.push({ symbol: entry, message: `unknown symbol: ${entry}` });
	}
	return { prices, errors };
}

const fmt = (v: number, d: number) =>
	v.toLocaleString('en-US', { minimumFractionDigits: d, maximumFractionDigits: d });

// ---- Chat -------------------------------------------------------------------

function parseVolume(text: string): number {
	if (/point\s+zero\s+one/.test(text)) return 0.01;
	if (/point\s+one/.test(text)) return 0.1;
	const m = text.match(/(?:buy|sell)\s+(\d+(?:\.\d+)?)/) ?? text.match(/(\d+(?:\.\d+)?)\s*lots?/);
	return m ? Number(m[1]) : 0.1;
}

/** `pattern` must capture the price in group 1. */
function priceAfter(text: string, pattern: RegExp): number | null {
	const m = text.match(pattern);
	return m ? Number(m[1]) : null;
}

function proposal(req: ChatRequest): { preview: PreviewDto; call: ToolCall } | null {
	const text = req.message.toLowerCase();
	const voice = req.voice === true;
	const call = (name: string, args: Record<string, unknown>): ToolCall => ({
		name,
		arguments: args,
		ok: true,
		duration_ms: 12
	});

	if (/\b(close|exit|flatten)\b/.test(text)) {
		const ticket = text.match(/\b(\d{6,})\b/)?.[1];
		const symbol = symbolFromText(text);
		const side = /\b(buys?|longs?)\b/.test(text) ? 'buy' : /\b(sells?|shorts?)\b/.test(text) ? 'sell' : null;
		const all = /\ball\b/.test(text) && !symbol && !side;
		let targets = book.slice();
		if (ticket) targets = targets.filter((p) => p.ticket === Number(ticket));
		else if (symbol || side || all) {
			if (symbol) targets = targets.filter((p) => p.symbol === symbol);
			if (side) targets = targets.filter((p) => p.side === side);
		} else return null;
		const half = /half/.test(text) && targets.length === 1;
		return {
			preview: closePreview(targets, {
				batch: !ticket,
				voice,
				volume: half ? Math.floor((targets[0].volume / 2) * 100) / 100 : null
			}),
			call: call('propose_close', { symbol, side, all, ticket: ticket ? Number(ticket) : null })
		};
	}
	if (/\bcancel\b.*\border/.test(text) || /\bdelete\b.*\border/.test(text)) {
		const symbol = symbolFromText(text);
		const tickets = pending.filter((o) => !symbol || o.symbol === symbol).map((o) => o.ticket);
		return { preview: cancelOrdersPreview(tickets, voice), call: call('propose_cancel', { symbol }) };
	}
	const order = text.match(/\b(buy|sell)\b/);
	const symbol = symbolFromText(text);
	if (order && symbol) {
		const side = order[1] as 'buy' | 'sell';
		const volume = parseVolume(text);
		const sl = priceAfter(text, /\b(?:sl|stop(?:\s+loss)?)\b\s*(?:at|=|of)?\s*(\d+(?:\.\d+)?)/);
		const tp = priceAfter(text, /\b(?:tp|take\s+profit|target)\b\s*(?:at|=|of)?\s*(\d+(?:\.\d+)?)/);
		return {
			preview: openPreview({ symbol, side, volume, sl, tp }, voice),
			call: call('propose_order', { symbol, side, volume, sl, tp })
		};
	}
	return null;
}

function chat(req: ChatRequest): ChatResponse {
	const text = req.message.toLowerCase();
	const tool_calls: ToolCall[] = [];
	let reply: string;
	let preview: PreviewDto | null = null;

	if (/fail/.test(text)) {
		throw new ApiError('llm_error', 'LLM provider failed: connection refused (mock)', 502);
	}

	let proposed: ReturnType<typeof proposal> = null;
	try {
		proposed = proposal(req);
	} catch (e) {
		if (e instanceof ApiError && e.code === 'gate_rejected') {
			// Jarvis relays the rejection; nothing was previewed.
			return {
				session_id: req.session_id ?? crypto.randomUUID(),
				reply: `I can't do that: ${e.message}.`,
				provider: req.provider ?? 'local',
				tool_calls: [{ name: 'propose_order', arguments: {}, ok: false, duration_ms: 8 }],
				preview: null,
				persona: activePersona().id
			};
		}
		throw e;
	}

	if (proposed) {
		// Kill switch: the propose tools refuse like every write path.
		assertWritable();
		preview = proposed.preview;
		tool_calls.push(proposed.call);
		reply = `Here is what I'd do: ${preview.description}. Please review and confirm within the time shown.`;
	} else if (/gold|xau|price|eur|jpy|btc|nas/.test(text)) {
		const symbol = symbolFromText(text) ?? 'XAUUSD';
		const t = tick().find((p) => p.symbol === symbol)!;
		tool_calls.push({ name: 'get_price', arguments: { symbol }, ok: true, duration_ms: 6 });
		reply = `${symbol} is ${fmt(t.bid, t.digits)} bid / ${fmt(t.ask, t.digits)} ask, a spread of ${t.spread_points} points.`;
	} else if (/position|open|trade/.test(text)) {
		const ps = positions();
		const total = ps.reduce((a, p) => a + p.profit, 0);
		tool_calls.push({ name: 'get_positions', arguments: {}, ok: true, duration_ms: 9 });
		reply = ps.length
			? `You have ${ps.length} open positions with a floating P/L of ${fmt(total, 2)} USD: ${ps.map((p) => `${p.side} ${p.volume} ${p.symbol}`).join(', ')}.`
			: 'You have no open positions.';
	} else if (/equity|balance|account|margin/.test(text)) {
		const a = account();
		tool_calls.push({ name: 'get_account', arguments: {}, ok: true, duration_ms: 5 });
		reply = `Equity is ${fmt(a.equity, 2)} ${a.currency} on a balance of ${fmt(a.balance, 2)}. Free margin is ${fmt(a.free_margin, 2)}.`;
	} else {
		reply =
			'I can read prices, your account and positions, and prepare trades for you to confirm. Try "close gold", "buy 0.1 gold sl 2300" or "what is gold at?".';
	}
	return {
		session_id: req.session_id ?? crypto.randomUUID(),
		reply: inVoice(reply),
		provider: req.provider ?? 'local',
		tool_calls,
		preview,
		persona: activePersona().id
	};
}

/** Friday sounds friendlier; the content (numbers, previews, refusals) is identical. */
function inVoice(reply: string): string {
	if (activePersona().id !== 'friday') return reply;
	return reply
		.replace(/^Here is what I'd do:/, "Here's the plan:")
		.replace(/Please review and confirm within the time shown\./, 'Take a look and confirm when you are ready.');
}

// ---- Voice -------------------------------------------------------------------

const SCRIPT = ['What is gold at?', 'Buy zero point one gold with stop loss 2300', 'Show my open positions'];
let scriptAt = 0;
let nextSpeech: string | null = null;

function classify(text: string): VoiceIntent {
	const t = text.toLowerCase().trim();
	if (/^(yes[, ]*)?(confirm|go ahead|do it|execute)\b/.test(t)) return 'confirm';
	if (/^(cancel|abort|never ?mind|stop)\b/.test(t)) return 'cancel';
	return 'other';
}

async function transcribe(blob: Blob | undefined, wake: boolean): Promise<TranscribeResponse> {
	const audio_ms = blob ? wavDurationMs(await blob.arrayBuffer()) : 0;
	if (audio_ms > 30_500) throw new ApiError('bad_request', 'audio longer than 30 s', 400);
	if (wake) {
		// Hands-free: only scripted speech is "heard"; anything else is background noise.
		const text = nextSpeech ?? 'mm-hm';
		nextSpeech = null;
		const w = detectWake(text);
		if (w.detected && w.persona) setPersona(w.persona);
		return { text, raw: text.toLowerCase(), audio_ms, elapsed_ms: 180, intent: classify(text), wake: w };
	}
	const text = nextSpeech ?? (hasLivePreview() ? 'confirm' : SCRIPT[scriptAt++ % SCRIPT.length]);
	nextSpeech = null;
	return { text, raw: text.toLowerCase(), audio_ms, elapsed_ms: 180, intent: classify(text) };
}

if (typeof window !== 'undefined') {
	(window as unknown as { __jarvisMock: unknown }).__jarvisMock = {
		say: (t: string) => (nextSpeech = t)
	};
}

// ---- Router -------------------------------------------------------------------

const sleep = (ms: number, signal?: AbortSignal) =>
	new Promise<void>((resolve, reject) => {
		const t = setTimeout(resolve, ms);
		signal?.addEventListener('abort', () => {
			clearTimeout(t);
			reject(new ApiError('aborted', 'Request cancelled'));
		});
	});

const tokenOf = (path: string, prefix: string) => decodeURIComponent(path.slice(prefix.length));
let seeded = false;

export async function mockRequest<T>(path: string, opts: RequestOptions): Promise<T> {
	if (!seeded) {
		seeded = true;
		seedBook();
	}
	applyConfig();
	await sleep(40 + Math.random() * 120, opts.signal);
	if (flag('offline')) throw new ApiError('network', 'Cannot reach the Arc MT server');

	// Auth: with ?mock=auth every route but login needs the mock session.
	if (path === '/api/auth/login') return login(opts.body as LoginRequest) as T;
	if (path === '/api/auth/logout') {
		try {
			sessionStorage.removeItem(SESSION_KEY);
		} catch {
			/* nothing to clear */
		}
		return undefined as T;
	}
	if (flag('auth') && !session()) throw new ApiError('unauthorized', 'sign in required', 401);
	if (path === '/api/auth/me') {
		// Like the server: without auth there is no session to describe.
		if (!flag('auth')) throw new ApiError('unknown', 'not found', 404);
		return session() as T;
	}

	if (path === '/api/status') return status() as T;
	if (path === '/api/personas') return getPersonas() as T;
	if (path === '/api/settings') {
		if (opts.method !== 'PUT') return getSettings() as T;
		if (flag('settingsfail')) throw new ApiError('internal', 'settings could not be saved (mock)', 500);
		const { settings, changed } = putSettings(opts.body, PROVIDERS);
		if (changed) emitMockEvent({ type: 'settings_changed' });
		return settings as T;
	}
	if (path === '/api/settings/limits') {
		if (opts.method === 'PUT' || opts.method === 'DELETE') {
			const b = (opts.body ?? {}) as { totp?: string };
			// Like the server: a code is required only when login is on.
			const checkCode = () => {
				if (flag('auth') && b.totp !== MOCK_TOTP) throw new ApiError('unauthorized', 'code not accepted', 401);
			};
			let res;
			if (opts.method === 'PUT') res = putLimits(opts.body, checkCode);
			else {
				checkCode();
				res = deleteLimits();
			}
			emitMockEvent({ type: 'settings_changed' });
			return res as T;
		}
		const res = getLimits();
		if (flag('staleconfig')) res.config.max_lot = 2;
		return res as T;
	}
	if (path === '/api/killswitch' && opts.method !== 'POST') return killState() as T;
	if (path === '/api/killswitch') {
		const b = opts.body as { engaged: boolean; reason?: string; totp?: string };
		if (!b.engaged && flag('auth') && b.totp !== '123456')
			throw new ApiError('unauthorized', 'invalid TOTP code', 401);
		return setKill(b.engaged, b.reason ?? null) as T;
	}
	if (flag('bridge'))
		throw new ApiError('bridge_unavailable', 'bridge did not reply within 3000 ms', 503);

	if (path === '/api/account') return account() as T;
	if (path === '/api/positions') return { positions: positions() } satisfies PositionsResponse as T;
	if (path === '/api/orders') return { orders: pending.map((o) => ({ ...o })) } satisfies OrdersResponse as T;
	if (path === '/api/watchlist') return watchlist() as T;

	if (path === '/api/chat') {
		const req = opts.body as ChatRequest;
		if (!req?.message || req.message.length > 2000)
			throw new ApiError('bad_request', 'message must be 1-2000 characters', 400);
		await sleep(900, opts.signal);
		return chat(req) as T;
	}

	if (path.startsWith('/api/confirm/')) return confirm(tokenOf(path, '/api/confirm/')) as T;
	if (path.startsWith('/api/cancel/')) {
		cancelToken(tokenOf(path, '/api/cancel/'));
		return undefined as T;
	}
	if (path.startsWith('/api/readback/')) return readback(tokenOf(path, '/api/readback/')) as T;

	let m: RegExpMatchArray | null;
	if ((m = path.match(/^\/api\/positions\/(\d+)\/close$/))) {
		assertWritable();
		const p = book.find((x) => x.ticket === Number(m![1]));
		if (!p) throw new ApiError('gate_rejected', `position ${m[1]} not found`, 422);
		const vol = (opts.body as { volume: number | null } | undefined)?.volume ?? null;
		return closePreview([p], { volume: vol, batch: false }) as T;
	}
	if (path === '/api/positions/close') {
		assertWritable();
		const b = opts.body as { symbol: string | null; side: 'buy' | 'sell' | null; all: boolean };
		const targets = book.filter(
			(p) => (b.all || b.symbol || b.side) && (!b.symbol || p.symbol === b.symbol) && (!b.side || p.side === b.side)
		);
		return closePreview(targets, { batch: true }) as T;
	}
	if ((m = path.match(/^\/api\/orders\/(\d+)\/cancel$/))) {
		assertWritable();
		return cancelOrdersPreview([Number(m[1])]) as T;
	}

	if (path === '/api/audit' || path.startsWith('/api/audit?')) return audit(path) as T;

	if (path === '/api/voice/transcribe' || path === '/api/voice/transcribe?wake=1') {
		if (flag('novoice')) throw new ApiError('voice_unavailable', 'voice is disabled', 503);
		await sleep(350, opts.signal);
		return (await transcribe(opts.blob, path.endsWith('wake=1'))) as T;
	}
	if (path === '/api/voice/speak') {
		if (flag('novoice')) throw new ApiError('voice_unavailable', 'voice is disabled', 503);
		const body = opts.body as { text?: string; persona?: string } | undefined;
		const text = body?.text ?? '';
		if (body?.persona && !getPersonas().personas.some((p) => p.id === body.persona))
			throw new ApiError('bad_request', `unknown persona: ${body.persona}`, 400);
		if (text.length === 0 || text.length > 600)
			throw new ApiError('bad_request', 'text must be 1-600 characters', 400);
		await sleep(200, opts.signal);
		return new Blob([toneWav(Math.min(2500, 500 + text.length * 12))], { type: 'audio/wav' }) as T;
	}

	throw new ApiError('unknown', `No mock for ${path}`, 404);
}

// ---- Audit ---------------------------------------------------------------------

const KINDS = ['utterance', 'intent', 'tool_call', 'preview', 'confirm', 'confirm_second', 'trade_result', 'cancel', 'kill_switch', 'login', 'voice_stt'];

function audit(path: string) {
	const q = new URLSearchParams(path.split('?')[1] ?? '');
	const limit = Math.min(200, Number(q.get('limit') ?? 100));
	const before = q.get('before_id') ? Number(q.get('before_id')) : 240;
	const entries = [];
	for (let id = before - 1; id >= 1 && entries.length < limit; id--) {
		const kind = KINDS[id % KINDS.length];
		entries.push({
			id,
			ts: Date.now() - (240 - id) * 47_000,
			session_id: id % 3 === 0 ? null : '4b0d7a1e-2c3f-4d5e-8f90-a1b2c3d4e5f6',
			kind,
			payload: payloadFor(kind, id)
		});
	}
	return { entries };
}

function payloadFor(kind: string, id: number): unknown {
	switch (kind) {
		case 'utterance':
			return { text: 'close all gold', voice: id % 2 === 0 };
		case 'tool_call':
			return { name: 'propose_close', arguments: { symbol: 'XAUUSD', side: null, all: false }, ok: true, duration_ms: 12 };
		case 'preview':
			return { preview_id: '6f1c0a52', tickets: [884201337, 884201401], total_volume: 0.3, is_batch: true, expires_at_ms: Date.now() };
		case 'trade_result':
			return { ticket: 884201337, ok: true, retcode: 10009, price: 2391.4 };
		case 'kill_switch':
			return { engaged: id % 2 === 0, reason: 'manual' };
		case 'login':
			return { username: 'ninad', ip: '127.0.0.1' };
		default:
			return { note: `${kind} #${id}` };
	}
}
