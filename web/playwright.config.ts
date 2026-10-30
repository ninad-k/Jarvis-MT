import { defineConfig, devices } from '@playwright/test';

const PORT = 5199;

// Smoke tests run against the mock backend (`vite dev --mode mock`): no server or MT5 needed.
export default defineConfig({
	testDir: 'e2e',
	timeout: 45_000,
	expect: { timeout: 10_000 },
	fullyParallel: true,
	// The orb renders WebGL in every page; keep contention low so countdown timing stays honest.
	workers: process.env.CI ? 2 : 4,
	forbidOnly: !!process.env.CI,
	retries: process.env.CI ? 1 : 0,
	reporter: process.env.CI ? [['github'], ['list']] : 'list',
	use: {
		baseURL: `http://127.0.0.1:${PORT}`,
		trace: 'retain-on-failure',
		screenshot: 'only-on-failure',
		serviceWorkers: 'block',
		permissions: ['microphone']
	},
	projects: [
		{
			name: 'chromium',
			use: {
				...devices['Desktop Chrome'],
				launchOptions: {
					// A synthetic microphone, so the voice path runs without hardware or a prompt.
					args: ['--use-fake-ui-for-media-stream', '--use-fake-device-for-media-stream']
				}
			}
		}
	],
	webServer: {
		command: `pnpm exec vite dev --mode mock --host 127.0.0.1 --port ${PORT} --strictPort`,
		url: `http://127.0.0.1:${PORT}`,
		reuseExistingServer: !process.env.CI,
		timeout: 120_000
	}
});
