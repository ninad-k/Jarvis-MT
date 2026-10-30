import { expect, test } from '@playwright/test';

test('the web manifest is served with the required fields and reachable icons', async ({ page, request }) => {
	await page.goto('/?mock=reset');
	await expect(page.locator('link[rel="manifest"]')).toHaveAttribute('href', '/manifest.webmanifest');

	const res = await request.get('/manifest.webmanifest');
	expect(res.ok()).toBe(true);
	expect(res.headers()['content-type']).toMatch(/json/);
	const m = await res.json();
	expect(m).toMatchObject({
		name: 'Jarvis MT',
		short_name: 'Jarvis',
		display: 'standalone',
		start_url: '/',
		theme_color: '#04070d',
		background_color: '#04070d'
	});
	const sizes = m.icons.map((i: { sizes: string; purpose: string }) => `${i.sizes}:${i.purpose}`);
	expect(sizes).toEqual(
		expect.arrayContaining(['192x192:any', '512x512:any', '192x192:maskable', '512x512:maskable'])
	);
	for (const icon of m.icons) {
		const r = await request.get(icon.src);
		expect(r.ok(), icon.src).toBe(true);
		expect(r.headers()['content-type']).toBe('image/png');
	}
	expect((await request.get('/icons/apple-touch-icon.png')).ok()).toBe(true);
});

test('iOS meta tags and the offline page are present', async ({ page, request }) => {
	await page.goto('/?mock=reset');
	await expect(page.locator('meta[name="apple-mobile-web-app-capable"]')).toHaveAttribute('content', 'yes');
	await expect(page.locator('link[rel="apple-touch-icon"]')).toHaveAttribute('href', /apple-touch-icon\.png/);
	const offline = await request.get('/offline.html');
	expect(await offline.text()).toContain('Offline — Jarvis needs the backend');
});
