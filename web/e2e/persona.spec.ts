import { expect, test } from '@playwright/test';
import { accentVar, modal, openHud, say } from './helpers';

type DevWindow = {
	__orb: { set: (s: string) => void };
	__scene?: { targetColors: { a: string; b: string } };
};

const toggle = (page: import('@playwright/test').Page) => page.getByRole('group', { name: 'Persona' });

test('the toggle switches persona: name, accent colour and orb colours follow', async ({ page }) => {
	await openHud(page);
	const t = toggle(page);
	await expect(t.getByRole('button', { name: 'Jarvis' })).toHaveAttribute('aria-pressed', 'true');
	await expect(t.getByRole('button', { name: 'Friday' })).toHaveAttribute('aria-pressed', 'false');
	await expect(page.getByTestId('orb-persona')).toHaveText('Jarvis');
	await expect.poll(() => accentVar(page)).toBe('#38bdf8');
	await expect(page.getByRole('heading', { name: /ARC/ })).toBeVisible();

	// Orb colours: the scene is created lazily; wait for it.
	await expect
		.poll(() => page.evaluate(() => (window as unknown as DevWindow).__scene?.targetColors.a))
		.toBe('#7dd3fc');

	await t.getByRole('button', { name: 'Friday' }).click();

	await expect(t.getByRole('button', { name: 'Friday' })).toHaveAttribute('aria-pressed', 'true');
	await expect(t.getByRole('button', { name: 'Jarvis' })).toHaveAttribute('aria-pressed', 'false');
	await expect(page.getByTestId('orb-persona')).toHaveText('Friday');
	await expect.poll(() => accentVar(page)).toBe('#f5b942');
	await expect
		.poll(() => page.evaluate(() => (window as unknown as DevWindow).__scene?.targetColors.a))
		.toBe('#fcd77f');
	// The computed colour the whole UI uses follows (after the ~400 ms transition).
	await expect
		.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--color-accent').trim()))
		.toBe('rgb(245, 185, 66)');

	// The assistant is named by the persona in the UI text.
	await expect(page.getByText('Ask Friday about your markets.')).toBeVisible();
	await page.evaluate(() => (window as unknown as DevWindow).__orb.set('listening'));
	await expect(page.getByRole('status').filter({ hasText: 'Friday is listening…' })).toBeVisible();
	await page.evaluate(() => (window as unknown as DevWindow).__orb.set('thinking'));
	await expect(page.getByRole('status').filter({ hasText: 'Friday is thinking…' })).toBeVisible();
	await page.evaluate(() => (window as unknown as DevWindow).__orb.set('idle'));

	// A reply is labelled with the persona that wrote it, and the choice survives a reload.
	await say(page, "What's gold at?");
	await expect(page.getByRole('log').getByText(/XAUUSD is [\d,.]+ bid/)).toBeVisible();
	await page.reload();
	await expect(page.getByTestId('orb-persona')).toHaveText('Friday');
	await expect.poll(() => accentVar(page)).toBe('#f5b942');
});

test('the toggle is keyboard accessible', async ({ page }) => {
	await openHud(page);
	const jarvis = toggle(page).getByRole('button', { name: 'Jarvis' });
	await jarvis.focus();
	await page.keyboard.press('ArrowRight');
	await expect(toggle(page).getByRole('button', { name: 'Friday' })).toBeFocused();
	await page.keyboard.press('Enter');
	await expect(toggle(page).getByRole('button', { name: 'Friday' })).toHaveAttribute('aria-pressed', 'true');
	// A second switch while the first is still being saved is ignored; wait for the save to land.
	await expect(toggle(page)).toHaveAttribute('aria-busy', 'false');
	await expect(toggle(page).getByRole('button', { name: 'Friday' })).toBeFocused();
	await page.keyboard.press('Shift+Tab');
	await page.keyboard.press('Space');
	await expect(toggle(page).getByRole('button', { name: 'Jarvis' })).toHaveAttribute('aria-pressed', 'true');
});

test('the toggle is disabled while a confirmation is open, and works again after', async ({ page }) => {
	await openHud(page);
	await say(page, 'close 884201552');
	const m = modal(page);
	await expect(m).toBeVisible();

	const friday = toggle(page).getByRole('button', { name: 'Friday', includeHidden: true });
	await expect(friday).toBeDisabled();
	await expect(toggle(page).getByRole('button', { name: 'Jarvis', includeHidden: true })).toBeDisabled();
	// Even a forced click cannot switch it.
	await friday.dispatchEvent('click');
	await expect(page.getByTestId('orb-persona')).toHaveText('Jarvis');
	expect(await accentVar(page)).toBe('#38bdf8');

	await page.keyboard.press('Escape');
	await expect(m).toHaveCount(0);
	await expect(friday).toBeEnabled();
	await friday.click();
	await expect(page.getByTestId('orb-persona')).toHaveText('Friday');
});

test('a failed switch rolls back and says why', async ({ page }) => {
	await openHud(page, 'settingsfail');
	await toggle(page).getByRole('button', { name: 'Friday' }).click();
	await expect(page.getByRole('alert').filter({ hasText: 'Could not switch persona' })).toBeVisible();
	await expect(toggle(page).getByRole('button', { name: 'Jarvis' })).toHaveAttribute('aria-pressed', 'true');
	await expect(page.getByTestId('orb-persona')).toHaveText('Jarvis');
	await expect.poll(() => accentVar(page)).toBe('#38bdf8');
});
