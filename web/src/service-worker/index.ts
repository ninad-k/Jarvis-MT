/// <reference types="@sveltejs/kit" />
/// <reference lib="webworker" />
/**
 * Caches the app shell only: the built JS/CSS/fonts and the static files. It never sees API
 * traffic: any request under /api/ is left to the network untouched (prices and trades must
 * never come from a cache). If the page itself cannot be fetched, offline.html is shown.
 */
import { dev, version } from '$app/env';
import { assets, immutable } from '$app/manifest';
import { self as sw } from '$app/service-worker';

const CACHE = `jarvis-shell-${version}`;
// Manifest paths are relative to the base path; make them absolute so they match `url.pathname`.
const abs = (path: string) => (path.startsWith('/') ? path : `/${path}`);
const SHELL = [...immutable, ...assets].map((e) => abs(e.path));
const OFFLINE = '/offline.html';

sw.addEventListener('install', (event) => {
	if (dev) return;
	event.waitUntil(
		caches
			.open(CACHE)
			.then((cache) => cache.addAll(SHELL))
			.then(() => sw.skipWaiting())
	);
});

sw.addEventListener('activate', (event) => {
	event.waitUntil(
		caches
			.keys()
			.then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
			.then(() => sw.clients.claim())
	);
});

sw.addEventListener('fetch', (event) => {
	if (dev) return;
	const req = event.request;
	if (req.method !== 'GET') return;
	const url = new URL(req.url);
	if (url.origin !== sw.location.origin) return;
	// Live data and the WebSocket upgrade: never cached, never intercepted.
	if (url.pathname.startsWith('/api/')) return;

	if (req.mode === 'navigate') {
		// Always try the network for the page; fall back to the offline notice.
		event.respondWith(
			fetch(req).catch(async () => (await caches.match(OFFLINE)) ?? Response.error())
		);
		return;
	}

	if (SHELL.includes(url.pathname)) {
		event.respondWith(caches.match(req).then((hit) => hit ?? fetch(req)));
	}
});
