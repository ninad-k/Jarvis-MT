import { expect, test } from '@playwright/test';
import { modal, openHud, say } from './helpers';

test('chat proposal -> preview modal -> confirm -> results -> positions refresh', async ({ page }) => {
	await openHud(page);
	await say(page, 'buy 0.1 gold sl 2300 tp 2420');

	const m = modal(page);
	await expect(m).toBeVisible();
	await expect(m.getByTestId('preview-description')).toContainText(/Buy 0\.10 XAUUSD at market/);
	await expect(m.getByText('At SL')).toBeVisible();
	await expect(m.getByText('At TP')).toBeVisible();
	await expect(m.getByTestId('countdown')).toContainText(/\d+ s left to confirm/);
	await expect(m).toHaveAttribute('data-phase', 'review');

	await m.getByRole('button', { name: 'Confirm', exact: true }).click();
	await expect(m).toHaveAttribute('data-phase', 'results');
	await expect(m.getByTestId('result-summary')).toHaveText('Executed');
	await expect(m.getByText(/10009 · Request executed/)).toBeVisible();

	await m.getByRole('button', { name: 'Close' }).click();
	await expect(m).toHaveCount(0);
	await expect(page.getByTestId('position-row')).toHaveCount(6);
	await expect(page.getByTestId('note').first()).toContainText('Executed');
});

test('Enter never confirms; focus stays trapped; Esc cancels', async ({ page }) => {
	await openHud(page);
	await say(page, 'close 884201552');
	const m = modal(page);
	await expect(m).toBeVisible();

	const confirm = m.getByRole('button', { name: 'Confirm', exact: true });
	await confirm.focus();
	await page.keyboard.press('Enter');
	await expect(m).toHaveAttribute('data-phase', 'review');
	for (let i = 0; i < 6; i++) {
		await page.keyboard.press('Tab');
		expect(await page.evaluate(() => !!document.activeElement?.closest('dialog'))).toBe(true);
	}

	await page.keyboard.press('Escape');
	await expect(m).toHaveCount(0);
	await expect(page.getByTestId('position-row')).toHaveCount(5);
	await expect(page.getByTestId('note').first()).toContainText('Cancelled');
});

test('batch close needs a second confirmation that repeats the count', async ({ page }) => {
	await openHud(page);
	await say(page, 'close gold');

	const m = modal(page);
	await expect(m).toBeVisible();
	await expect(m).toHaveAttribute('data-stage', 'first');
	await expect(m.getByText('Step 1 of 2')).toBeVisible();

	await m.getByRole('button', { name: 'Confirm batch' }).click();
	await expect(m).toHaveAttribute('data-stage', 'second');
	await expect(m.getByTestId('second-stage-message')).toHaveText('Confirm again to close 2 positions');
	await expect(m.getByText('Step 2 of 2')).toBeVisible();
	await expect(m.getByTestId('countdown')).toContainText(/1\d s left to confirm again/);

	await m.getByRole('button', { name: 'Confirm again' }).click();
	await expect(m.getByTestId('result-summary')).toHaveText('Executed: all 2 succeeded');
	await m.getByRole('button', { name: 'Close' }).click();
	await expect(page.getByTestId('position-row')).toHaveCount(3);
});

test('Close all reports a partial failure per ticket', async ({ page }) => {
	await openHud(page);
	await page.getByRole('button', { name: 'Close positions' }).click();
	await page.getByRole('menuitem', { name: /Close all/ }).click();

	const m = modal(page);
	await expect(m).toBeVisible();
	await m.getByRole('button', { name: 'Confirm batch' }).click();
	await m.getByRole('button', { name: 'Confirm again' }).click();
	await expect(m.getByTestId('result-summary')).toHaveText('Partly executed: 4 of 5 succeeded');
	await expect(m.getByText(/10018 · Market is closed/)).toBeVisible();
	await m.getByRole('button', { name: 'Close' }).click();
	await expect(page.getByTestId('position-row')).toHaveCount(1);
});

test('row Close, Close half and pending-order Cancel only open previews', async ({ page }) => {
	await openHud(page);
	await page.getByRole('button', { name: /^Close EURUSD sell/ }).click();
	await expect(modal(page).getByTestId('preview-description')).toContainText(/Close sell 0\.50 EURUSD/);
	await page.keyboard.press('Escape');
	await expect(modal(page)).toHaveCount(0);

	await page.getByRole('button', { name: 'More actions for ticket 884201337' }).click();
	await page.getByRole('menuitem', { name: /Close half/ }).click();
	await expect(modal(page).getByTestId('preview-description')).toContainText(/Close 0\.05 of buy 0\.10 XAUUSD/);
	await page.keyboard.press('Escape');
	await expect(modal(page)).toHaveCount(0);

	await page.getByRole('button', { name: /^Cancel EURUSD buy limit/ }).click();
	await expect(modal(page).getByTestId('preview-description')).toContainText(/Cancel pending buy limit 0\.10 EURUSD/);
	await page.keyboard.press('Escape');

	// Nothing executed.
	await expect(page.getByTestId('position-row')).toHaveCount(5);
	await expect(page.getByTestId('order-row')).toHaveCount(2);
});

test('the countdown reaches Expired and disables Confirm', async ({ page }) => {
	await openHud(page, 'ttl=4');
	await say(page, 'close 884201552');
	const m = modal(page);
	// The window runs against the client clock, so no matter how slow the machine, it must end.
	await expect(m.getByTestId('countdown')).toContainText('Expired', { timeout: 15_000 });
	await expect(m.getByRole('button', { name: 'Confirm', exact: true })).toBeDisabled();
	await expect(m.getByTestId('live')).toContainText('Expired');
	await page.keyboard.press('Escape');
	await expect(m).toHaveCount(0);
});

test('outcome_unknown says so plainly and refreshes positions', async ({ page }) => {
	await openHud(page, 'unknown');
	await say(page, 'close 884201552');
	const m = modal(page);
	await m.getByRole('button', { name: 'Confirm', exact: true }).click();
	const err = m.getByTestId('confirm-error');
	await expect(err).toContainText('Outcome unknown — check positions');
	await expect(err).toHaveAttribute('data-code', 'outcome_unknown');
	await expect(page.getByTestId('note').first()).toContainText('Outcome unknown');
});

test('a gate rejection is relayed in chat and no modal opens', async ({ page }) => {
	await openHud(page);
	await say(page, 'buy 5 gold');
	await expect(page.getByRole('log').getByText(/exceeds max_lot/)).toBeVisible();
	await expect(modal(page)).toHaveCount(0);
});
