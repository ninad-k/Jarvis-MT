import { expect, test } from '@playwright/test';
import { modal, openHud } from './helpers';

type MockWindow = { __jarvisMock: { say: (t: string) => void } };

test('tap the mic, speak a trade, then confirm by voice with Space', async ({ page }) => {
	await openHud(page);
	const mic = page.getByTestId('mic');
	await expect(mic).toBeEnabled();

	// Tap-to-toggle: tap to start, tap again to send.
	await page.evaluate(() => (window as unknown as MockWindow).__jarvisMock.say('Buy 0.1 gold with stop loss 2300'));
	await mic.click({ delay: 20 });
	await expect(page.getByTestId('listening-row')).toBeVisible();
	await page.waitForTimeout(900);
	await mic.click({ delay: 20 });

	await expect(page.getByRole('log').getByText('Buy 0.1 gold with stop loss 2300')).toBeVisible();
	const m = modal(page);
	await expect(m).toBeVisible();
	await expect(m.getByTestId('countdown')).toContainText(/s left to confirm|Reading/);

	// Push-to-talk: hold Space with focus in the dialog body (not on a button), say "confirm".
	await page.evaluate(() => (window as unknown as MockWindow).__jarvisMock.say('confirm'));
	await page.keyboard.down('Space');
	await page.waitForTimeout(900);
	await page.keyboard.up('Space');
	await expect(m.getByTestId('result-summary')).toHaveText('Executed');
});

test('voice is disabled when the server has no speech-to-text', async ({ page }) => {
	await openHud(page, 'novoice');
	await expect(page.getByTestId('mic')).toBeDisabled();
});
