import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

// The server sends `script-src 'self'`, which also governs AudioWorklet modules. A worklet
// loaded from a blob: or data: URL is blocked in production (the dev server sends no CSP,
// so the e2e tests would not notice): the microphone then fails with "Could not start the
// microphone." Worklets must be same-origin module files loaded via `?worker&url`.
const sources = ['audio.ts', 'listener.ts'].map((f) => ({
	file: f,
	text: readFileSync(new URL(`./${f}`, import.meta.url), 'utf8')
}));

describe('audio worklets are CSP-safe', () => {
	for (const { file, text } of sources) {
		it(`${file} loads its worklet from a same-origin module file`, () => {
			expect(text).toContain('?worker&url');
			expect(text).not.toMatch(/createObjectURL/);
			expect(text).not.toMatch(/addModule\(\s*['"`]?(blob|data):/);
		});
	}
});
