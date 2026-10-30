/**
 * API error model: the server error codes from docs/api.md, client-side codes for failures that
 * never produced a server body, and the pure mapping from an HTTP response to an ApiError.
 */

export const SERVER_ERROR_CODES = [
	'bad_request',
	'mode_mismatch',
	'bridge_unavailable',
	'bridge_error',
	'llm_error',
	'internal',
	'unauthorized',
	'gate_rejected',
	'token_unknown',
	'token_expired',
	'token_used',
	'kill_switch',
	'rate_limited',
	'voice_unavailable',
	'trade_failed',
	'outcome_unknown',
	'limit_not_stricter',
	'unknown_broker_symbol'
] as const;

export type ApiErrorCode = (typeof SERVER_ERROR_CODES)[number];

/** Client-side codes for failures that never produced a server error body. */
export type ClientErrorCode = 'network' | 'timeout' | 'aborted' | 'unknown';

export interface ApiErrorBody {
	error: { code: ApiErrorCode; message: string };
}

export class ApiError extends Error {
	readonly code: ApiErrorCode | ClientErrorCode;
	readonly status: number;
	/** From the `Retry-After` header (429), in ms. */
	readonly retryAfterMs: number | null;

	constructor(
		code: ApiErrorCode | ClientErrorCode,
		message: string,
		status = 0,
		retryAfterMs: number | null = null
	) {
		super(message);
		this.name = 'ApiError';
		this.code = code;
		this.status = status;
		this.retryAfterMs = retryAfterMs;
	}

	/** The Arc MT server itself could not be reached (as opposed to the MT5 bridge). */
	get isOffline(): boolean {
		return this.code === 'network';
	}

	get isUnauthorized(): boolean {
		return this.code === 'unauthorized';
	}
}

export function isErrorBody(v: unknown): v is ApiErrorBody {
	if (typeof v !== 'object' || v === null || !('error' in v)) return false;
	const e = (v as { error: unknown }).error;
	return (
		typeof e === 'object' &&
		e !== null &&
		typeof (e as { code?: unknown }).code === 'string' &&
		typeof (e as { message?: unknown }).message === 'string'
	);
}

function isServerCode(code: string): code is ApiErrorCode {
	return (SERVER_ERROR_CODES as readonly string[]).includes(code);
}

/** `Retry-After` is delta-seconds (HTTP-date is not used by the server). */
export function parseRetryAfter(header: string | null | undefined): number | null {
	if (!header) return null;
	const s = Number(header);
	return Number.isFinite(s) && s >= 0 ? Math.round(s * 1000) : null;
}

/**
 * Map a failed HTTP response to an ApiError.
 * - a server error body keeps its code (unknown codes become `unknown`)
 * - a bare 401 is still `unauthorized` so the UI can redirect to /login
 * - a bare 5xx is a dead proxy target, i.e. the server is unreachable
 */
export function errorFromResponse(
	status: number,
	body: unknown,
	retryAfter?: string | null
): ApiError {
	const retryMs = parseRetryAfter(retryAfter);
	if (isErrorBody(body)) {
		const code = isServerCode(body.error.code) ? body.error.code : 'unknown';
		return new ApiError(code, body.error.message, status, retryMs);
	}
	if (status === 401) return new ApiError('unauthorized', 'Sign-in required', status);
	if (status === 429) return new ApiError('rate_limited', 'Too many requests', status, retryMs);
	if (status >= 500) return new ApiError('network', 'Cannot reach the Arc MT server', status);
	return new ApiError('unknown', `Unexpected response (HTTP ${status})`, status);
}

/** Confirm failures where the same token can be tried again; everything else ends the confirmation. */
export const RETRYABLE_CONFIRM_CODES: ReadonlySet<string> = new Set(['rate_limited']);

/**
 * Plain-language text for an error shown to the trader. `outcome_unknown` is deliberately blunt:
 * the order may or may not have reached the market.
 */
export function describeError(err: ApiError): string {
	switch (err.code) {
		case 'outcome_unknown':
			return 'Outcome unknown — check positions. The request may or may not have been executed.';
		case 'token_expired':
			return 'This confirmation expired. Nothing was sent. Ask again to get a fresh preview.';
		case 'token_used':
			return 'This confirmation was already used or cancelled. Nothing more was sent.';
		case 'token_unknown':
			return 'This confirmation is no longer valid. Nothing was sent.';
		case 'kill_switch':
			return 'Kill switch is engaged. Nothing was sent.';
		case 'rate_limited': {
			const s = err.retryAfterMs !== null ? Math.max(1, Math.ceil(err.retryAfterMs / 1000)) : null;
			return s ? `Too many requests. Try again in ${s} s.` : 'Too many requests. Try again shortly.';
		}
		case 'gate_rejected':
			return err.message || 'The request was rejected by a safety limit.';
		case 'limit_not_stricter':
			return err.message || 'That value is looser than the config file allows.';
		case 'unknown_broker_symbol':
			return err.message || "That symbol is not in this broker's catalog.";
		case 'unauthorized':
			return 'Your session has ended. Sign in again.';
		case 'network':
			return 'Cannot reach the Arc MT server.';
		case 'timeout':
			return 'The server took too long to respond.';
		case 'voice_unavailable':
			return 'Voice is not available on the server.';
		default:
			return err.message || 'Something went wrong.';
	}
}

export function toApiError(e: unknown): ApiError {
	if (e instanceof ApiError) return e;
	return new ApiError('unknown', e instanceof Error ? e.message : 'Unexpected error');
}
