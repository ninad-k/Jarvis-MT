import { describe, expect, it } from 'vitest';
import {
	ApiError,
	SERVER_ERROR_CODES,
	RETRYABLE_CONFIRM_CODES,
	describeError,
	errorFromResponse,
	isErrorBody,
	parseRetryAfter,
	toApiError
} from './errors';

const body = (code: string, message = 'm') => ({ error: { code, message } });

describe('errorFromResponse', () => {
	it('keeps every documented server code', () => {
		for (const code of SERVER_ERROR_CODES) {
			const e = errorFromResponse(400, body(code, 'text'));
			expect(e.code).toBe(code);
			expect(e.message).toBe('text');
			expect(e.status).toBe(400);
		}
	});

	it('maps an unknown code to "unknown" but keeps the message', () => {
		const e = errorFromResponse(418, body('teapot', 'short and stout'));
		expect(e.code).toBe('unknown');
		expect(e.message).toBe('short and stout');
	});

	it('maps the documented confirm failures', () => {
		expect(errorFromResponse(410, body('token_expired')).code).toBe('token_expired');
		expect(errorFromResponse(409, body('token_used')).code).toBe('token_used');
		expect(errorFromResponse(423, body('kill_switch')).code).toBe('kill_switch');
		expect(errorFromResponse(504, body('outcome_unknown')).code).toBe('outcome_unknown');
	});

	it('treats a bare 401 as unauthorized so the UI can redirect', () => {
		const e = errorFromResponse(401, undefined);
		expect(e.code).toBe('unauthorized');
		expect(e.isUnauthorized).toBe(true);
	});

	it('reads Retry-After on a 429', () => {
		const e = errorFromResponse(429, body('rate_limited', 'slow down'), '7');
		expect(e.code).toBe('rate_limited');
		expect(e.retryAfterMs).toBe(7000);
		expect(errorFromResponse(429, undefined).code).toBe('rate_limited');
	});

	it('treats a bare 5xx as the server being unreachable', () => {
		const e = errorFromResponse(502, undefined);
		expect(e.code).toBe('network');
		expect(e.isOffline).toBe(true);
	});

	it('does not treat a server-shaped 5xx as offline', () => {
		expect(errorFromResponse(502, body('bridge_error', 'retcode 10006')).isOffline).toBe(false);
	});

	it('falls back to unknown for other bare statuses', () => {
		expect(errorFromResponse(404, undefined)).toMatchObject({ code: 'unknown', status: 404 });
	});
});

describe('helpers', () => {
	it('isErrorBody validates the shape', () => {
		expect(isErrorBody(body('internal'))).toBe(true);
		expect(isErrorBody({ error: 'x' })).toBe(false);
		expect(isErrorBody({ error: { code: 1, message: 'x' } })).toBe(false);
		expect(isErrorBody(null)).toBe(false);
	});

	it('parseRetryAfter accepts delta-seconds only', () => {
		expect(parseRetryAfter('3')).toBe(3000);
		expect(parseRetryAfter('0')).toBe(0);
		expect(parseRetryAfter('soon')).toBeNull();
		expect(parseRetryAfter(null)).toBeNull();
		expect(parseRetryAfter('-1')).toBeNull();
	});

	it('toApiError wraps non-ApiError values', () => {
		const e = new ApiError('internal', 'x');
		expect(toApiError(e)).toBe(e);
		expect(toApiError(new Error('boom'))).toMatchObject({ code: 'unknown', message: 'boom' });
		expect(toApiError('str').code).toBe('unknown');
	});
});

describe('describeError', () => {
	it('says plainly that the outcome is unknown', () => {
		expect(describeError(new ApiError('outcome_unknown', 'x', 504))).toMatch(
			/^Outcome unknown — check positions/
		);
	});

	it('explains each confirm failure without echoing server jargon', () => {
		expect(describeError(new ApiError('token_expired', 'x'))).toMatch(/expired/i);
		expect(describeError(new ApiError('token_used', 'x'))).toMatch(/already used or cancelled/i);
		expect(describeError(new ApiError('kill_switch', 'x'))).toMatch(/kill switch/i);
	});

	it('shows the retry delay for rate limits', () => {
		expect(describeError(new ApiError('rate_limited', 'x', 429, 2500))).toContain('3 s');
		expect(describeError(new ApiError('rate_limited', 'x', 429))).toMatch(/shortly/);
	});

	it('passes through gate rejections, which name the limit', () => {
		expect(describeError(new ApiError('gate_rejected', 'volume 5 exceeds max_lot 1', 422))).toBe(
			'volume 5 exceeds max_lot 1'
		);
	});

	it('only rate limiting leaves the confirmation open for another try', () => {
		for (const c of ['token_expired', 'token_used', 'kill_switch', 'outcome_unknown', 'trade_failed']) {
			expect(RETRYABLE_CONFIRM_CODES.has(c)).toBe(false);
		}
		expect(RETRYABLE_CONFIRM_CODES.has('rate_limited')).toBe(true);
	});
});
