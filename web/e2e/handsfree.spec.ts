import { expect, test, type Page } from '@playwright/test';
import { openHud, openSettings, section } from './helpers';

/** Switch the stored setting to hands-free, as a user would on the settings page. */
async function chooseHandsFree(page: Page): Promise<void> {
	await openSettings(page);
	await page.getByTestId('mode-wake_word').check();
	await section(page, 'voice').getByTestId('voice-save').click();
	await expect(section(page, 'voice').getByTestId('voice-feedback')).toContainText('Voice settings saved.');
}

test('hands-free mode shows a clear indicator and a one-click stop', async ({ page }) => {
	await chooseHandsFree(page);
	await openHud(page);

	const bar = page.getByTestId('handsfree-bar');
	await expect(bar).toBeVisible();
	await expect(bar).toHaveAttribute('data-active', 'false');
	await expect(bar).toContainText('Hands-free is off');
	// The mic button is now the hands-free switch.
	const mic = page.getByTestId('mic');
	await expect(mic).toHaveAttribute('aria-label', 'Hands-free listening');
	await expect(mic).toHaveAttribute('aria-pressed', 'false');

	await page.getByTestId('handsfree-start').click();
	await expect(bar).toHaveAttribute('data-active', 'true');
	await expect(page.getByTestId('handsfree-indicator')).toContainText('Hands-free listening');
	await expect(page.getByTestId('handsfree-status')).toContainText('Say “hey jarvis”');
	await expect(mic).toHaveAttribute('aria-pressed', 'true');

	await page.getByTestId('handsfree-stop').click();
	await expect(bar).toHaveAttribute('data-active', 'false');
	await expect(mic).toHaveAttribute('aria-pressed', 'false');

	// The mic button works as the switch too.
	await mic.click();
	await expect(bar).toHaveAttribute('data-active', 'true');
	await mic.click();
	await expect(bar).toHaveAttribute('data-active', 'false');
});

test('there is no hands-free bar in the other input modes', async ({ page }) => {
	await openHud(page);
	await expect(page.getByTestId('handsfree-bar')).toHaveCount(0);
	await expect(page.getByTestId('mic')).toHaveAttribute('aria-label', 'Voice input');
});

test('hands-free cannot start when the server has no speech recognition', async ({ page }) => {
	await chooseHandsFree(page);
	await openHud(page, 'novoice');
	await expect(page.getByTestId('handsfree-start')).toBeDisabled();
});
