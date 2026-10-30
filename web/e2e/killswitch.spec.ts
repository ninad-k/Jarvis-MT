import { expect, test } from '@playwright/test';
import { modal, openHud, say } from './helpers';

test('kill switch disables every write control, shows the banner, and releases', async ({ page }) => {
	await openHud(page);
	const closeRow = page.getByRole('button', { name: /^Close EURUSD sell/ });
	await expect(closeRow).toBeEnabled();

	await page.getByTestId('kill-toggle').click();
	await page.getByRole('button', { name: 'Engage', exact: true }).click();

	await expect(page.getByTestId('kill-banner')).toHaveText(
		'Trading disabled — kill switch engaged (manual)'
	);
	await expect(closeRow).toBeDisabled();
	await expect(page.getByRole('button', { name: 'Close positions' })).toBeDisabled();
	await expect(page.getByRole('button', { name: /^Cancel EURUSD buy limit/ })).toBeDisabled();

	// A chat request cannot start a trade either.
	await say(page, 'close 884201552');
	await expect(modal(page)).toHaveCount(0);

	await page.getByTestId('kill-toggle').click();
	await page.getByRole('button', { name: 'Release', exact: true }).click();
	await expect(page.getByTestId('kill-banner')).toHaveCount(0);
	await expect(closeRow).toBeEnabled();
});

test('releasing needs the authenticator code when auth is on', async ({ page }) => {
	await page.goto('/?mock=auth');
	await page.getByLabel('Username').fill('ninad');
	await page.getByLabel('Password').fill('secret');
	await page.getByLabel('Authenticator code').fill('123456');
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByTestId('position-row').first()).toBeVisible();

	await page.getByTestId('kill-toggle').click();
	await page.getByRole('button', { name: 'Engage', exact: true }).click();
	await expect(page.getByTestId('kill-banner')).toBeVisible();

	await page.getByTestId('kill-toggle').click();
	const release = page.getByRole('button', { name: 'Release', exact: true });
	await expect(release).toBeDisabled();
	const code = page.getByRole('dialog', { name: 'Release kill switch' }).getByLabel('Authenticator code');
	await expect(code).toHaveAttribute('inputmode', 'numeric');
	await code.fill('000000');
	await release.click();
	await expect(page.getByRole('alert').filter({ hasText: 'not accepted' })).toBeVisible();
	await expect(page.getByTestId('kill-banner')).toBeVisible();

	await code.fill('123456');
	await release.click();
	await expect(page.getByTestId('kill-banner')).toHaveCount(0);
});
