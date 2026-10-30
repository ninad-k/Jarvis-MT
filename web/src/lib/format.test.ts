import { describe, expect, it } from 'vitest';
import { auditMs, payloadSummary } from './format';

describe('auditMs', () => {
	it('accepts seconds, milliseconds and ISO strings', () => {
		expect(auditMs(1_793_341_200)).toBe(1_793_341_200_000);
		expect(auditMs(1_793_341_200_000)).toBe(1_793_341_200_000);
		expect(auditMs('2026-10-30T09:00:00Z')).toBe(Date.parse('2026-10-30T09:00:00Z'));
		expect(auditMs('not a date')).toBe(0);
	});
});

describe('payloadSummary', () => {
	it('lists scalar fields and stringifies nested ones', () => {
		expect(payloadSummary({ ticket: 1, ok: true, tickets: [1, 2] })).toBe('ticket=1 ok=true tickets=[1,2]');
	});

	it('skips nulls and clips long values and lines', () => {
		expect(payloadSummary({ a: null, b: 'x'.repeat(40) })).toBe(`b=${'x'.repeat(31)}…`);
		expect(payloadSummary({ a: 'y'.repeat(30), b: 'z'.repeat(30), c: 'w'.repeat(30), d: 'v'.repeat(30) }, 50).length).toBe(50);
	});

	it('handles non-objects', () => {
		expect(payloadSummary(null)).toBe('');
		expect(payloadSummary('plain text')).toBe('plain text');
	});
});
