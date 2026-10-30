import { describe, expect, it } from 'vitest';
import { micErrorMessage } from './audio';

const named = (name: string) => Object.assign(new Error(name), { name });

describe('micErrorMessage', () => {
	it('explains a denied permission', () => {
		expect(micErrorMessage(named('NotAllowedError'))).toMatch(/denied/i);
	});

	it('explains a missing device', () => {
		expect(micErrorMessage(named('NotFoundError'))).toMatch(/no microphone/i);
	});

	it('explains a busy device and an insecure page', () => {
		expect(micErrorMessage(named('NotReadableError'))).toMatch(/in use/i);
		expect(micErrorMessage(named('InsecureContext'))).toMatch(/HTTPS or localhost/);
	});

	it('has a generic fallback', () => {
		expect(micErrorMessage(new Error('x'))).toBe('Could not start the microphone.');
		expect(micErrorMessage(undefined)).toBe('Could not start the microphone.');
	});
});
