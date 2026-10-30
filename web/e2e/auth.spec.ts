import { expect, test } from '@playwright/test';

test('unauthenticated visit redirects to /login; login, username and logout work', async ({ page }) => {
	await page.goto('/?mock=auth');
	await expect(page).toHaveURL(/\/login$/);

	const totp = page.getByLabel('Authenticator code');
	await expect(totp).toHaveAttribute('inputmode', 'numeric');
	await expect(totp).toHaveAttribute('autocomplete', 'one-time-code');
	await expect(page.getByRole('button', { name: 'Sign in' })).toBeDisabled();

	await page.getByLabel('Username').fill('ninad');
	await page.getByLabel('Password').fill('hunter2');
	await totp.fill('000000');
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByRole('alert')).toContainText('Sign-in failed');
	await expect(page).toHaveURL(/\/login$/);

	await page.getByLabel('Password').fill('hunter2');
	await page.getByLabel('Authenticator code').fill('123456');
	await page.getByRole('button', { name: 'Sign in' }).click();

	await expect(page).toHaveURL(/\/$/);
	await expect(page.getByTestId('position-row').first()).toBeVisible();
	await page.getByRole('button', { name: 'Menu for ninad' }).click();
	await expect(page.getByRole('menuitem', { name: 'Signed in as ninad' })).toBeVisible();
	await page.getByRole('menuitem', { name: 'Log out' }).click();
	await expect(page).toHaveURL(/\/login$/);

	// Back on the HUD URL the session is gone again.
	await page.goto('/');
	await expect(page).toHaveURL(/\/login$/);
});
