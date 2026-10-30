import { expect, test, type Page } from '@playwright/test';
import { modal, say } from './helpers';

test.use({ viewport: { width: 375, height: 812 }, hasTouch: true });

const widths = (page: Page) =>
	page.evaluate(() => ({
		scroll: document.documentElement.scrollWidth,
		client: document.documentElement.clientWidth,
		body: document.body.scrollWidth
	}));

test('HUD has no horizontal overflow at 375 px, including the confirmation sheet', async ({ page }) => {
	await page.goto('/?mock=reset');
	// Below lg only one panel shows at a time; the tab badge proves the data has arrived.
	await expect(page.getByRole('tab', { name: /Positions/ })).toContainText('5');
	let w = await widths(page);
	expect(w.scroll).toBeLessThanOrEqual(w.client);
	expect(w.body).toBeLessThanOrEqual(w.client);

	await page.getByRole('tab', { name: /Positions/ }).click();
	w = await widths(page);
	expect(w.scroll).toBeLessThanOrEqual(w.client);

	await page.getByRole('tab', { name: /Chat/ }).click();
	await say(page, 'close gold');
	await expect(modal(page)).toBeVisible();
	const box = await modal(page).boundingBox();
	expect(box!.x).toBeGreaterThanOrEqual(0);
	expect(box!.x + box!.width).toBeLessThanOrEqual(375);
	w = await widths(page);
	expect(w.scroll).toBeLessThanOrEqual(w.client);
});

test('login and audit pages fit at 375 px', async ({ page }) => {
	await page.goto('/login?mock=reset');
	let w = await widths(page);
	expect(w.scroll).toBeLessThanOrEqual(w.client);

	await page.goto('/audit?mock=reset');
	await expect(page.getByTestId('audit-row').first()).toBeVisible();
	w = await widths(page);
	expect(w.scroll).toBeLessThanOrEqual(w.client);
});
