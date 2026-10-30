import { expect, type Page } from '@playwright/test';

/** Open the HUD in mock mode. `mock` is the comma-separated switch list (see src/lib/mock.ts). */
export async function openHud(page: Page, mock = 'reset'): Promise<void> {
	await page.goto(`/?mock=${mock}`);
	await expect(page.getByRole('heading', { name: /ARC/ })).toBeVisible();
	await expect(page.getByTestId('position-row').first()).toBeVisible();
}

/** Type into the composer and send. */
export async function say(page: Page, text: string): Promise<void> {
	await page.getByLabel(/^Message (Jarvis|Friday)$/).fill(text);
	await page.getByRole('button', { name: 'Send', exact: true }).click();
}

export const modal = (page: Page) => page.getByTestId('confirm-modal');

/** The CSS custom property the persona theme sets on <html>. */
export const accentVar = (page: Page) =>
	page.evaluate(() => document.documentElement.style.getPropertyValue('--persona-accent').trim());

/** Open the settings page in mock mode and wait for its sections. */
export async function openSettings(page: Page, mock = 'reset'): Promise<void> {
	await page.goto(`/settings?mock=${mock}`);
	await expect(page.getByRole('heading', { name: 'Settings', level: 1 })).toBeVisible();
	await expect(page.getByTestId('section-persona')).toBeVisible();
}

export const section = (page: Page, id: string) => page.getByTestId(`section-${id}`);
