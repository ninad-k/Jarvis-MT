import { expect, type Page } from '@playwright/test';

/** Open the HUD in mock mode. `mock` is the comma-separated switch list (see src/lib/mock.ts). */
export async function openHud(page: Page, mock = 'reset'): Promise<void> {
	await page.goto(`/?mock=${mock}`);
	await expect(page.getByRole('heading', { name: /JARVIS/ })).toBeVisible();
	await expect(page.getByTestId('position-row').first()).toBeVisible();
}

/** Type into the composer and send. */
export async function say(page: Page, text: string): Promise<void> {
	await page.getByLabel('Message Jarvis').fill(text);
	await page.getByRole('button', { name: 'Send', exact: true }).click();
}

export const modal = (page: Page) => page.getByTestId('confirm-modal');
