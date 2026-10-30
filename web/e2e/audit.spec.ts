import { expect, test } from '@playwright/test';
import { openHud } from './helpers';

test('audit log lists entries, expands JSON and pages with before_id', async ({ page }) => {
	await openHud(page);
	await page.getByRole('button', { name: 'Menu', exact: true }).click();
	await page.getByRole('menuitem', { name: 'Audit log' }).click();
	await expect(page).toHaveURL(/\/audit$/);

	const rows = page.getByTestId('audit-row');
	await expect(rows).toHaveCount(50);
	await rows.first().getByRole('button').click();
	await expect(rows.first().locator('pre')).toContainText('{');

	await page.getByRole('button', { name: 'Load older' }).click();
	await expect(rows).toHaveCount(100);

	await page.getByLabel('Kind').selectOption('kill_switch');
	await expect(rows.first()).toContainText('kill_switch');
	await page.getByRole('link', { name: /HUD/ }).click();
	await expect(page).toHaveURL(/\/$/);
});
