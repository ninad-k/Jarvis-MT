import { defineConfig } from 'vitest/config';

// Unit tests cover the pure modules (no DOM, no Svelte), so no framework plugin is needed.
export default defineConfig({
	test: {
		environment: 'node',
		include: ['src/**/*.test.ts']
	}
});
