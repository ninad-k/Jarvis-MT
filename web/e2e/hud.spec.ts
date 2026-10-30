import { expect, test } from '@playwright/test';
import { openHud, say } from './helpers';

test('HUD loads with watchlist, positions and demo badge', async ({ page }) => {
	const errors: string[] = [];
	page.on('pageerror', (e) => errors.push(e.message));
	await openHud(page);

	await expect(page.getByRole('heading', { name: /ARC/ })).toBeVisible();
	await expect(page.getByText('Demo', { exact: true })).toBeVisible();
	await expect(page.getByRole('region', { name: 'Watchlist' }).getByText('XAUUSD', { exact: true })).toBeVisible();
	await expect(page.getByTestId('position-row')).toHaveCount(5);
	await expect(page.getByTestId('order-row')).toHaveCount(2);
	await expect(page.getByText(/MT5 connected/).first()).toBeVisible();
	expect(errors).toEqual([]);
});

test('a read-only question gets an answer without any confirmation', async ({ page }) => {
	await openHud(page);
	await say(page, "What's gold at?");
	await expect(page.getByRole('log').getByText(/XAUUSD is [\d,.]+ bid/)).toBeVisible();
	await expect(page.getByTestId('confirm-modal')).toHaveCount(0);
});

test('LIVE mode shows the banner', async ({ page }) => {
	await openHud(page, 'live');
	await expect(page.getByText('LIVE ACCOUNT — real money')).toBeVisible();
});
