import { describe, expect, it } from 'vitest';
import { MAX_SPEAK, clipForSpeech } from './text';

describe('clipForSpeech', () => {
	it('leaves short text alone and collapses whitespace', () => {
		expect(clipForSpeech('Buy   0.1\ngold.')).toBe('Buy 0.1 gold.');
	});

	it('never exceeds the server limit', () => {
		expect(clipForSpeech('word '.repeat(400)).length).toBeLessThanOrEqual(MAX_SPEAK);
	});

	it('cuts at a sentence end when one is far enough in', () => {
		const text = `${'a'.repeat(400)}. ${'b'.repeat(400)}`;
		expect(clipForSpeech(text)).toBe(`${'a'.repeat(400)}.`);
	});

	it('otherwise cuts at a word boundary', () => {
		const out = clipForSpeech('lorem '.repeat(200));
		expect(out.endsWith('lorem')).toBe(true);
		expect(out.length).toBeLessThanOrEqual(MAX_SPEAK);
	});
});
