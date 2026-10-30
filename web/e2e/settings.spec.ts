import { expect, test, type Page } from '@playwright/test';
import { accentVar, openHud, openSettings, section } from './helpers';

const feedback = (page: Page, id: string) => section(page, id).getByTestId(`${id}-feedback`);
const save = (page: Page, id: string) => section(page, id).getByTestId(`${id}-save`);

test('persona cards show the packs and select one', async ({ page }) => {
	await openSettings(page);
	const cards = page.getByTestId('persona-card');
	await expect(cards).toHaveCount(2);
	const friday = cards.filter({ hasText: 'Friday' });
	await expect(friday).toContainText('Warm, upbeat and casual.');
	await expect(friday).toContainText('“hey friday”');
	await expect(cards.filter({ hasText: 'Jarvis' })).toHaveAttribute('data-active', 'true');

	await friday.getByRole('button', { name: 'Select Friday' }).click();
	await expect(friday).toHaveAttribute('data-active', 'true');
	await expect(feedback(page, 'persona')).toContainText('Friday is now your assistant');
	await expect.poll(() => accentVar(page)).toBe('#f5b942');

	// Preview voice plays the sample through the mock TTS without an error.
	await friday.getByRole('button', { name: 'Preview voice' }).click();
	await expect(friday.getByRole('button', { name: 'Playing…' })).toBeVisible();
	await expect(friday.getByRole('button', { name: 'Preview voice' })).toBeVisible();
	await expect(page.getByRole('alert').filter({ hasText: 'Could not read' })).toHaveCount(0);
});

test('voice settings save and persist', async ({ page }) => {
	await openSettings(page);
	const voice = section(page, 'voice');
	await expect(save(page, 'voice')).toBeDisabled();

	await page.getByTestId('read-aloud').check();
	await page.getByTestId('mode-toggle').check();
	await voice.getByLabel('Language').selectOption('auto');
	await expect(voice.getByTestId('voice-unsaved')).toBeVisible();
	await save(page, 'voice').click();
	await expect(feedback(page, 'voice')).toContainText('Voice settings saved.');
	await expect(save(page, 'voice')).toBeDisabled();

	await page.reload();
	await expect(page.getByTestId('read-aloud')).toBeChecked();
	await expect(page.getByTestId('mode-toggle')).toBeChecked();
	await expect(section(page, 'voice').getByLabel('Language')).toHaveValue('auto');

	// Hands-free explains itself and its privacy note when chosen.
	await expect(page.getByTestId('handsfree-help')).toHaveCount(0);
	await page.getByTestId('mode-wake_word').check();
	await expect(page.getByTestId('handsfree-help')).toContainText('only when speech is detected');
});

test('the model list disables providers that are not available', async ({ page }) => {
	await openSettings(page);
	const select = page.getByTestId('llm-setting');
	await expect(select.locator('option', { hasText: 'cloud' })).toBeDisabled();
	await expect(select.locator('option', { hasText: 'cloud' })).toContainText('API key not set');
	await select.selectOption('local');
	await save(page, 'model').click();
	await expect(feedback(page, 'model')).toContainText('New messages use local.');
	await select.selectOption('');
	await save(page, 'model').click();
	await expect(feedback(page, 'model')).toContainText('server default');
});

test('watchlist: add, reorder, remove, validate, save, and the HUD follows', async ({ page }) => {
	await openSettings(page);
	const items = page.getByTestId('wl-item');
	await expect(items).toHaveCount(7);
	await expect(page.getByTestId('wl-count')).toHaveText('7 / 30');

	const add = page.getByLabel('Add a symbol');
	await add.fill('has;bad');
	await add.press('Enter');
	await expect(page.getByRole('alert').filter({ hasText: 'Letters, digits' })).toBeVisible();
	await add.fill('xauusd');
	await add.press('Enter');
	await expect(page.getByRole('alert').filter({ hasText: 'already in the list' })).toBeVisible();

	await add.fill('yellow metal');
	await add.press('Enter');
	await expect(items).toHaveCount(8);

	await page.getByRole('button', { name: 'Remove US30' }).click();
	await page.getByRole('button', { name: 'Move BTCUSD up' }).click();
	await expect(items.nth(4)).toContainText('BTCUSD');
	await expect(items.nth(5)).toContainText('US100');

	await save(page, 'watchlist').click();
	await expect(feedback(page, 'watchlist')).toContainText('Watchlist saved (7 symbols).');

	await page.getByTestId('back-to-hud').click();
	const wl = page.getByRole('region', { name: 'Watchlist' });
	await expect(wl.getByText('BTCUSD', { exact: true })).toBeVisible();
	await expect(wl.getByText('US30', { exact: true })).toHaveCount(0);
	// "yellow metal" resolves to XAUUSD, so gold now appears twice.
	await expect(wl.getByText('XAUUSD', { exact: true })).toHaveCount(2);
});

test('the watchlist stops at 30', async ({ page }) => {
	await openSettings(page);
	const add = page.getByLabel('Add a symbol');
	for (let i = 0; i < 23; i++) {
		await add.fill(`SYM${i}`);
		await add.press('Enter');
	}
	await expect(page.getByTestId('wl-count')).toHaveText('30 / 30');
	await expect(add).toBeDisabled();
});

test('aliases: add, validate, save; config aliases are shown read-only', async ({ page }) => {
	await openSettings(page);
	const aliases = section(page, 'aliases');
	await expect(page.getByTestId('alias-row')).toHaveCount(1);
	await expect(page.getByTestId('config-aliases')).toContainText('nasdaq');
	await expect(page.getByTestId('config-aliases').getByRole('textbox')).toHaveCount(0);

	await aliases.getByRole('button', { name: 'Add alias' }).click();
	await expect(page.getByTestId('alias-row')).toHaveCount(2);
	await page.getByLabel('Spoken name (row 2)').fill('Silver');
	await page.getByLabel('Broker symbol (row 2)').fill('not valid');
	await expect(aliases.getByRole('alert').filter({ hasText: 'no spaces' })).toBeVisible();
	await expect(save(page, 'aliases')).toBeEnabled();
	await save(page, 'aliases').click();
	await expect(feedback(page, 'aliases')).toContainText('Fix the highlighted rows');

	await page.getByLabel('Broker symbol (row 2)').fill('XAGUSD');
	await save(page, 'aliases').click();
	await expect(feedback(page, 'aliases')).toContainText('Aliases saved (2).');
	await page.reload();
	// Rows are sorted by spoken name: silver comes before yellow metal.
	await expect(page.getByLabel('Spoken name (row 1)')).toHaveValue('silver');
	await page.getByRole('button', { name: 'Remove alias silver' }).click();
	await save(page, 'aliases').click();
	await expect(feedback(page, 'aliases')).toContainText('Aliases saved (1).');
});

test('limits: explained as tighten-only, a looser value is refused, a stricter one is applied', async ({ page }) => {
	await openSettings(page);
	const limits = section(page, 'limits');
	await expect(page.getByTestId('limits-explainer')).toContainText('Limits can only be tightened here.');
	await expect(page.getByTestId('limits-explainer')).toContainText('edit');
	await expect(page.getByTestId('limit-row-max_lot')).toContainText('1');

	// Looser than the config: refused inline, nothing sent.
	await page.getByLabel('Max lot per order').fill('2');
	await save(page, 'limits').click();
	await expect(limits.getByRole('alert').filter({ hasText: "Can't be above the config value (1)" })).toBeVisible();
	await expect(feedback(page, 'limits')).toContainText('not allowed');
	await page.getByLabel('Max positions').fill('9');
	await save(page, 'limits').click();
	await expect(limits.getByRole('alert').filter({ hasText: 'above the config value (5)' })).toBeVisible();

	// Stricter: accepted, and the table shows config versus effective.
	await page.getByLabel('Max lot per order').fill('0.5');
	await page.getByLabel('Max positions').fill('3');
	await page.getByLabel('Max daily loss').fill('200');
	await page.getByLabel('Allow GBPUSD').uncheck();
	await page.getByTestId('hours-enabled').check();
	await page.getByLabel('From').fill('08:00');
	await page.getByLabel('Until').fill('16:00');
	await page.getByRole('group', { name: 'Days' }).getByLabel('Fri').uncheck();
	await save(page, 'limits').click();
	await expect(feedback(page, 'limits')).toContainText('Stricter limits are active now');

	const lot = page.getByTestId('limit-row-max_lot');
	await expect(lot).toContainText('0.5');
	await expect(lot.locator('[data-tightened="true"]')).toHaveText(/0\.5/);
	await expect(page.getByTestId('limit-row-max_positions')).toContainText('3');
	await expect(page.getByTestId('limit-row-allowed_symbols').locator('[data-tightened="true"]')).not.toContainText('GBPUSD');
	await expect(page.getByTestId('limit-row-trading_hours').locator('[data-tightened="true"]')).toContainText('Mon–Thu 08:00–16:00');

	// The gate uses them: 0.6 lots is now over the limit.
	await page.getByTestId('back-to-hud').click();
	await page.getByLabel(/^Message (Jarvis|Friday)$/).fill('buy 0.6 gold');
	await page.getByRole('button', { name: 'Send', exact: true }).click();
	await expect(page.getByRole('log').getByText(/exceeds max_lot 0\.5/)).toBeVisible();

	// Reset to config.
	await page.getByTestId('settings-link').click();
	await page.getByTestId('limits-reset').click();
	await expect(feedback(page, 'limits')).toContainText('Back to the limits in the config file');
	await expect(page.getByTestId('limit-row-max_lot').locator('[data-tightened="true"]')).toHaveCount(0);
	await expect(page.getByTestId('limits-reset')).toBeDisabled();
});

test('limits: a server refusal appears under the field it names', async ({ page }) => {
	// The page is told a looser config (max lot 2) than the gate enforces (1).
	await openSettings(page, 'staleconfig');
	await page.getByLabel('Max lot per order').fill('1.5');
	await save(page, 'limits').click();
	await expect(
		section(page, 'limits').getByRole('alert').filter({ hasText: 'max_lot 1.5 is looser than the config value 1' })
	).toBeVisible();
});

test('limits need the authenticator code when sign-in is on', async ({ page }) => {
	await page.goto('/?mock=auth');
	await page.getByLabel('Username').fill('ninad');
	await page.getByLabel('Password').fill('secret');
	await page.getByLabel('Authenticator code').fill('123456');
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByTestId('position-row').first()).toBeVisible();
	await page.getByTestId('settings-link').click();
	await expect(page.getByTestId('section-limits')).toBeVisible();

	const limits = section(page, 'limits');
	await expect(limits.getByText('Needed to apply or reset limits.')).toBeVisible();
	await page.getByLabel('Max lot per order').fill('0.5');
	await save(page, 'limits').click();
	await expect(limits.getByRole('alert').filter({ hasText: 'Enter the 6-digit code' })).toBeVisible();

	await page.getByTestId('limits-totp').fill('000000');
	await save(page, 'limits').click();
	await expect(limits.getByRole('alert').filter({ hasText: 'code not accepted' })).toBeVisible();
	await expect(page.getByTestId('limit-row-max_lot').locator('[data-tightened="true"]')).toHaveCount(0);

	await page.getByTestId('limits-totp').fill('123456');
	await save(page, 'limits').click();
	await expect(feedback(page, 'limits')).toContainText('Stricter limits are active now');
	await expect(page.getByTestId('limits-totp')).toHaveValue('');

	// Reset needs a code too.
	await page.getByTestId('limits-reset').click();
	await expect(limits.getByRole('alert').filter({ hasText: 'Enter the 6-digit code' })).toBeVisible();
	await page.getByTestId('limits-totp').fill('123456');
	await page.getByTestId('limits-reset').click();
	await expect(feedback(page, 'limits')).toContainText('Back to the limits in the config file');
});

test('leaving with unsaved changes asks first', async ({ page }) => {
	await openSettings(page);
	await page.getByLabel('Add a symbol').fill('US500');
	await page.getByLabel('Add a symbol').press('Enter');
	await expect(section(page, 'watchlist').getByTestId('watchlist-unsaved')).toBeVisible();

	let message = '';
	page.once('dialog', (d) => {
		message = d.message();
		void d.dismiss();
	});
	await page.getByTestId('back-to-hud').click();
	await expect.poll(() => message).toContain('unsaved changes');
	await expect(page).toHaveURL(/\/settings/);

	page.once('dialog', (d) => void d.accept());
	await page.getByTestId('back-to-hud').click();
	await expect(page).toHaveURL(/\/$/);
	await expect(page.getByTestId('position-row').first()).toBeVisible();
});

test('settings are reachable from the gear button and the menu', async ({ page }) => {
	await openHud(page);
	await page.getByTestId('settings-link').click();
	await expect(page).toHaveURL(/\/settings/);
	await page.getByTestId('back-to-hud').click();
	await page.getByRole('button', { name: 'Menu' }).click();
	await page.getByRole('menuitem', { name: 'Settings' }).click();
	await expect(page).toHaveURL(/\/settings/);
});
