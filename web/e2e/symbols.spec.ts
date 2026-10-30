import { readFileSync } from 'node:fs';
import { expect, test, type Page } from '@playwright/test';
import { openHud, openSettings, section } from './helpers';

const CFI = 'CFI Financial Markets L.L.C';
const IC = 'Raw Trading Ltd';

const symbols = (page: Page) => section(page, 'symbols');
const row = (page: Page, canonical: string) => symbols(page).locator(`[data-testid="symbol-row"][data-canonical="${canonical}"]`);
const editor = (page: Page) => symbols(page).getByTestId('symbol-editor');
const feedback = (page: Page) => symbols(page).getByTestId('symbols-feedback');

async function openSymbols(page: Page, mock = 'reset'): Promise<void> {
	await openSettings(page, mock);
	await expect(row(page, 'XAUUSD')).toBeVisible();
}

test('the section lists the broker and the map grouped by class', async ({ page }) => {
	await openSymbols(page);

	const header = symbols(page).getByTestId('symbols-header');
	await expect(header.getByTestId('symbols-broker')).toHaveText(`${CFI} · CFI3-Demo`);
	await expect(header.getByTestId('symbols-catalog')).toContainText('symbols');
	await expect(header.getByTestId('symbols-suffix')).toHaveText('"_"');
	await expect(header.getByTestId('symbols-prefix')).toHaveText('none');
	await expect(header.getByTestId('symbols-detected-at')).not.toBeEmpty();

	// Metals, Indices, Energy, Crypto, FX - in that order.
	const groups = symbols(page).getByTestId('symbol-group');
	await expect(groups).toHaveCount(5);
	await expect(groups.locator('h3 > span:first-child')).toHaveText(['Metals', 'Indices', 'Energy', 'Crypto', 'FX']);
	await expect(groups.nth(0).getByTestId('symbol-row')).toHaveCount(2);

	// Canonical name with its aliases, the broker symbol and where it came from.
	const us100 = row(page, 'US100');
	await expect(us100.getByTestId('symbol-name')).toHaveText('US100 · Nasdaq, NAS100, USTEC');
	await expect(us100.getByTestId('symbol-broker')).toHaveText('US100_Spot');
	await expect(us100.getByTestId('source-badge')).toHaveText('auto');
	await expect(row(page, 'XAUUSD').getByTestId('symbol-broker')).toHaveText('XAUUSD_');
	await expect(row(page, 'XAUUSD').getByTestId('source-badge')).toHaveText('curated');
	await expect(row(page, 'GER40').getByTestId('source-badge')).toHaveText('pattern');
	await expect(row(page, 'JP225').getByTestId('source-badge')).toHaveText('unchanged');

	// The tooltip explains the badge and opens on keyboard focus too.
	const badge = us100.getByTestId('source-badge');
	await badge.focus();
	await expect(us100.getByRole('tooltip')).toBeVisible();
	await expect(us100.getByRole('tooltip')).toContainText('Found automatically');
	await badge.press('Escape');
	await expect(us100.getByRole('tooltip')).toBeHidden();

	// A name the broker does not list is flagged; instruments with no match are listed apart.
	await expect(row(page, 'JP225')).toHaveAttribute('data-available', 'false');
	await expect(row(page, 'JP225').getByTestId('symbol-unavailable')).toContainText("Not in this broker's catalog");
	await expect(row(page, 'XAUUSD').getByTestId('symbol-unavailable')).toHaveCount(0);
	await expect(symbols(page).getByTestId('symbols-attention')).toContainText('3 instruments need attention');
	const unmapped = symbols(page).getByTestId('symbols-unmapped');
	await expect(unmapped.getByTestId('unmapped-row')).toHaveText([/HK50/, /LTCUSD/]);
});

test('edit and save an override from the catalog, then reset it to automatic', async ({ page }) => {
	await openSymbols(page);
	const us100 = row(page, 'US100');

	// Keyboard: the editor takes focus; the suggestions come with their reasons.
	await us100.getByRole('button', { name: 'Edit mapping for US100' }).press('Enter');
	const combo = editor(page).getByRole('combobox');
	await expect(combo).toBeFocused();
	await expect(combo).toHaveAttribute('aria-expanded', 'true');
	await expect(editor(page).getByRole('option').first()).toContainText('score');

	// Typing searches the whole catalog (debounced); ArrowDown + Enter picks the first match.
	await combo.fill('nas100');
	const first = editor(page).getByRole('option').first();
	await expect(first).toContainText('NAS100.cash');
	await combo.press('ArrowDown');
	await expect(combo).toHaveAttribute('aria-activedescendant', /-opt-0$/);
	await combo.press('Enter');
	await expect(combo).toHaveValue('NAS100.cash');
	await expect(combo).toHaveAttribute('aria-expanded', 'false');

	await editor(page).getByTestId('symbol-save').click();
	await expect(feedback(page)).toContainText('US100 now maps to NAS100.cash.');
	await expect(editor(page)).toHaveCount(0);
	await expect(us100.getByTestId('symbol-broker')).toHaveText('NAS100.cash');
	await expect(us100.getByTestId('source-badge')).toHaveText('user');
	// Focus is back on the button that opened the editor.
	await expect(us100.getByTestId('symbol-edit')).toBeFocused();

	// It survives a reload (and the watchlist now shows the new broker name).
	await page.reload();
	await expect(row(page, 'US100').getByTestId('symbol-broker')).toHaveText('NAS100.cash');

	// Reset to automatic: the override goes, the detected name comes back.
	await row(page, 'US100').getByTestId('symbol-edit').click();
	await editor(page).getByTestId('symbol-reset').click();
	await expect(feedback(page)).toContainText('US100 is back to automatic mapping.');
	await expect(row(page, 'US100').getByTestId('symbol-broker')).toHaveText('US100_Spot');
	await expect(row(page, 'US100').getByTestId('source-badge')).toHaveText('auto');
	// No override left, so no reset button.
	await row(page, 'US100').getByTestId('symbol-edit').click();
	await expect(editor(page).getByTestId('symbol-reset')).toHaveCount(0);
});

test('a catalog search result can be picked with the mouse; Escape closes the list, then the editor', async ({ page }) => {
	await openSymbols(page);
	await row(page, 'EURUSD').getByTestId('symbol-edit').click();
	const combo = editor(page).getByRole('combobox');
	await combo.fill('eurgbp');
	await expect(editor(page).getByRole('option', { name: /EURGBP_/ })).toBeVisible();
	await editor(page).getByRole('option', { name: /EURGBP_/ }).click();
	await expect(combo).toHaveValue('EURGBP_');

	await combo.fill('xx-no-match');
	await expect(editor(page).getByText('No symbols match "xx-no-match".')).toBeVisible();
	await combo.press('Escape');
	await expect(combo).toHaveAttribute('aria-expanded', 'false');
	await expect(editor(page)).toBeVisible();
	await combo.press('Escape');
	await expect(editor(page)).toHaveCount(0);
});

test('a symbol the broker does not list is refused inline', async ({ page }) => {
	await openSymbols(page);
	await row(page, 'XAGUSD').getByTestId('symbol-edit').click();
	const combo = editor(page).getByRole('combobox');
	await combo.fill('NOPE.cash');
	await editor(page).getByTestId('symbol-save').click();
	await expect(editor(page).getByRole('alert')).toContainText(`NOPE.cash is not in the ${CFI} catalog`);
	await expect(combo).toHaveAttribute('aria-invalid', 'true');
	// Nothing changed behind the editor.
	await expect(row(page, 'XAGUSD').getByTestId('symbol-broker')).toHaveText('XAGUSD_');
	// Empty box: asked for a name, nothing sent.
	await combo.fill('');
	await editor(page).getByTestId('symbol-save').click();
	await expect(editor(page).getByRole('alert')).toContainText('Type or pick the broker symbol first.');
});

test('an unmapped instrument can be mapped by hand', async ({ page }) => {
	await openSymbols(page);
	const unmapped = symbols(page).getByTestId('symbols-unmapped');
	await unmapped.getByRole('button', { name: 'Map HK50 to a broker symbol' }).click();
	const combo = editor(page).getByRole('combobox');
	await combo.fill('xpdusd');
	await editor(page).getByRole('option', { name: /XPDUSD_/ }).click();
	await editor(page).getByTestId('symbol-save').click();
	await expect(feedback(page)).toContainText('HK50 now maps to XPDUSD_.');
	await expect(row(page, 'HK50').getByTestId('symbol-broker')).toHaveText('XPDUSD_');
	await expect(unmapped.getByTestId('unmapped-row')).toHaveCount(1);
});

test('re-detect shows progress, then the result', async ({ page }) => {
	await openSymbols(page);
	const button = symbols(page).getByTestId('symbols-redetect');
	await button.click();
	await expect(button).toBeDisabled();
	await expect(button).toHaveText('Detecting…');
	await expect(symbols(page).getByText("Reading the broker's symbol list")).toBeVisible();
	await expect(feedback(page)).toContainText(`Detection finished for ${CFI}`, { timeout: 10_000 });
	await expect(button).toBeEnabled();
	await expect(button).toHaveText('Re-detect');
});

test('export downloads the effective map as JSON', async ({ page }) => {
	await openSymbols(page);
	const [download] = await Promise.all([page.waitForEvent('download'), symbols(page).getByTestId('symbols-export').click()]);
	expect(download.suggestedFilename()).toBe('broker-symbols-cfi-financial-markets-l-l-c.json');
	const doc = JSON.parse(readFileSync((await download.path())!, 'utf8')) as { broker: string; map: Record<string, string> };
	expect(doc.broker).toBe(CFI);
	expect(doc.map.XAUUSD).toBe('XAUUSD_');
	expect(doc.map.US100).toBe('US100_Spot');
	await expect(feedback(page)).toContainText('Exported the map');
});

test('a broker switch re-maps the section and says so', async ({ page }) => {
	await openSymbols(page, 'switchbroker');
	await expect(symbols(page).getByTestId('symbols-broker')).toContainText(CFI);
	await expect(row(page, 'US100').getByTestId('symbol-broker')).toHaveText('US100_Spot');

	const toast = page.getByRole('region', { name: 'Notifications' });
	await expect(toast).toContainText(`Broker changed to ${IC} — symbols re-mapped`, { timeout: 12_000 });
	await expect(symbols(page).getByTestId('symbols-broker')).toHaveText(`${IC} · ICMarketsSC-Demo`);
	await expect(row(page, 'US100').getByTestId('symbol-broker')).toHaveText('USTEC');
	await expect(row(page, 'US100').getByTestId('source-badge')).toHaveText('curated');
	await expect(symbols(page).getByTestId('symbols-suffix')).toHaveText('none');
	// This broker lists everything, so nothing is unmapped or unavailable.
	await expect(symbols(page).getByTestId('symbols-unmapped')).toHaveCount(0);
	await expect(symbols(page).locator('[data-available="false"]')).toHaveCount(0);
});

test('the HUD shows canonical names with the broker symbol beside them, and follows a broker switch', async ({ page }) => {
	await openHud(page, 'switchbroker');
	const wl = page.getByRole('region', { name: 'Watchlist' });
	await expect(wl.getByTestId('wl-symbol').filter({ hasText: /^XAUUSD$/ })).toBeVisible();
	await expect(wl.getByTestId('wl-broker-symbol').filter({ hasText: /^XAUUSD_$/ })).toBeVisible();
	const gold = page.getByTestId('position-row').first();
	await expect(gold.getByTestId('position-symbol')).toHaveText('XAUUSD');
	await expect(gold.getByTestId('position-broker-symbol')).toHaveText('XAUUSD_');
	await expect(page.getByTestId('bridge-broker')).toContainText(CFI);
	await expect(page.getByTestId('bridge-broker')).toContainText('CFI3-Demo');

	await expect(page.getByRole('region', { name: 'Notifications' })).toContainText(`Broker changed to ${IC}`, { timeout: 12_000 });
	await expect(wl.getByTestId('wl-broker-symbol').filter({ hasText: /^USTEC$/ })).toBeVisible();
	// Where both names are equal nothing extra is shown.
	await expect(wl.getByTestId('wl-broker-symbol').filter({ hasText: /^XAUUSD_?$/ })).toHaveCount(0);
	await expect(page.getByTestId('bridge-broker')).toContainText(IC);
});

test('mapping changes need the authenticator code when sign-in is on', async ({ page }) => {
	await page.goto('/?mock=auth');
	await page.getByLabel('Username').fill('ninad');
	await page.getByLabel('Password').fill('secret');
	await page.getByLabel('Authenticator code').fill('123456');
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByTestId('position-row').first()).toBeVisible();
	await page.getByTestId('settings-link').click();
	await expect(row(page, 'XAUUSD')).toBeVisible();

	await row(page, 'XAGUSD').getByTestId('symbol-edit').click();
	const combo = editor(page).getByRole('combobox');
	await combo.fill('XAUUSD_');
	await combo.press('Escape');
	await editor(page).getByTestId('symbol-save').click();
	await expect(editor(page).getByRole('alert')).toContainText('Enter the 6-digit code');

	await editor(page).getByTestId('symbol-totp').fill('000000');
	await editor(page).getByTestId('symbol-save').click();
	await expect(editor(page).getByRole('alert')).toContainText('code not accepted');
	await expect(row(page, 'XAGUSD').getByTestId('symbol-broker')).toHaveText('XAGUSD_');

	await editor(page).getByTestId('symbol-totp').fill('123456');
	await editor(page).getByTestId('symbol-save').click();
	await expect(feedback(page)).toContainText('XAGUSD now maps to XAUUSD_.');
	await expect(row(page, 'XAGUSD').getByTestId('source-badge')).toHaveText('user');

	// Resetting needs a code too.
	await row(page, 'XAGUSD').getByTestId('symbol-edit').click();
	await editor(page).getByTestId('symbol-reset').click();
	await expect(editor(page).getByRole('alert')).toContainText('Enter the 6-digit code');
	await editor(page).getByTestId('symbol-totp').fill('123456');
	await editor(page).getByTestId('symbol-reset').click();
	await expect(row(page, 'XAGUSD').getByTestId('symbol-broker')).toHaveText('XAGUSD_');
	await expect(row(page, 'XAGUSD').getByTestId('source-badge')).toHaveText('curated');
});

test.describe('at 375 px', () => {
	test.use({ viewport: { width: 375, height: 812 }, hasTouch: true });

	const overflow = (page: Page) =>
		page.evaluate(() => {
			const main = document.querySelector('main')!;
			return {
				doc: document.documentElement.scrollWidth - document.documentElement.clientWidth,
				main: main.scrollWidth - main.clientWidth
			};
		});

	test('the section has no horizontal overflow, with the editor and its list open', async ({ page }) => {
		await openSymbols(page);
		let o = await overflow(page);
		expect(o.doc).toBeLessThanOrEqual(0);
		expect(o.main).toBeLessThanOrEqual(0);

		// The row with the most to say: aliases, an unavailable name, a long catalog hit.
		await row(page, 'JP225').scrollIntoViewIfNeeded();
		await row(page, 'JP225').getByTestId('symbol-edit').click();
		await expect(editor(page).getByRole('option').first()).toBeVisible();
		await editor(page).getByRole('combobox').fill('spot');
		await expect(editor(page).getByRole('option').nth(3)).toBeVisible();
		o = await overflow(page);
		expect(o.doc).toBeLessThanOrEqual(0);
		expect(o.main).toBeLessThanOrEqual(0);

		// The editor and its buttons stay inside the viewport.
		const box = await editor(page).boundingBox();
		expect(box!.x).toBeGreaterThanOrEqual(0);
		expect(box!.x + box!.width).toBeLessThanOrEqual(375);
		const save = editor(page).getByTestId('symbol-save');
		await save.scrollIntoViewIfNeeded();
		await expect(save).toBeInViewport();
	});
});
