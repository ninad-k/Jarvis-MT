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

test('settings fits at 375 px with every section open', async ({ page }) => {
	await page.goto('/settings?mock=reset');
	await expect(page.getByTestId('section-limits')).toBeVisible();
	await page.getByTestId('mode-wake_word').check();
	await page.getByTestId('hours-enabled').check();
	await page.getByRole('button', { name: 'Add alias' }).click();
	let w = await widths(page);
	expect(w.scroll).toBeLessThanOrEqual(w.client);
	expect(w.body).toBeLessThanOrEqual(w.client);
	// No section pokes out of the viewport either.
	const boxes = await page.locator('main section').evaluateAll((els) =>
		els.map((e) => {
			const r = e.getBoundingClientRect();
			return { id: e.id, left: r.left, right: r.right };
		})
	);
	expect(boxes.length).toBe(7);
	for (const b of boxes) {
		expect(b.left, b.id).toBeGreaterThanOrEqual(0);
		expect(b.right, b.id).toBeLessThanOrEqual(375);
	}

	// A section with errors showing still fits.
	await page.getByLabel('Max lot per order').fill('2');
	await page.getByTestId('limits-save').click();
	w = await widths(page);
	expect(w.scroll).toBeLessThanOrEqual(w.client);
});

test('the top bar with the persona toggle fits at 375 px', async ({ page }) => {
	await page.goto('/?mock=reset');
	const group = page.getByRole('group', { name: 'Persona' });
	await expect(group).toBeVisible();
	let w = await widths(page);
	expect(w.scroll).toBeLessThanOrEqual(w.client);
	const box = await group.boundingBox();
	expect(box!.x + box!.width).toBeLessThanOrEqual(375);
	await group.getByRole('button', { name: 'Friday' }).click();
	await expect(group.getByRole('button', { name: 'Friday' })).toHaveAttribute('aria-pressed', 'true');
	w = await widths(page);
	expect(w.scroll).toBeLessThanOrEqual(w.client);
});
