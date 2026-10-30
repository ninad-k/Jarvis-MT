import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// The Rust server (jarvis-api) listens here; see docs/api.md.
const API_TARGET = 'http://127.0.0.1:8787';

export default defineConfig({
	build: {
		// three.js is lazy-loaded (dynamic import in Orb.svelte), so its ~550 kB chunk is expected.
		chunkSizeWarningLimit: 700
	},
	plugins: [
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// Single-page app: the Rust server serves build/ and falls back to index.html.
			adapter: adapter({ fallback: 'index.html' })
		})
	],
	server: {
		proxy: { '/api': { target: API_TARGET } }
	},
	preview: {
		proxy: { '/api': { target: API_TARGET } }
	}
});
