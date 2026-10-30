import { expect, test } from '@playwright/test';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { encodeWav } from '../src/lib/voice/wav';
import { openHud, openSettings, section } from './helpers';

type MockWindow = { __jarvisMock: { say: (t: string) => void } };

// A real utterance through the whole hands-free path. Chromium plays this file as the
// microphone: a loud, speech-like burst every few seconds, so the in-browser detector,
// the WAV upload, the (mock) transcription and the routing all run for real.

function speechWav(): string {
	const rate = 16_000;
	const burst = 1.4;
	const gap = 3.2;
	const out = new Float32Array(Math.round(rate * (burst + gap)));
	for (let i = 0; i < Math.round(rate * burst); i++) {
		const t = i / rate;
		out[i] = 0.4 * (0.6 + 0.4 * Math.sin(2 * Math.PI * 3.5 * t)) * Math.sin(2 * Math.PI * 190 * t);
	}
	const dir = mkdtempSync(join(tmpdir(), 'arcmt-mic-'));
	const file = join(dir, 'speech.wav');
	writeFileSync(file, Buffer.from(encodeWav(out, rate)));
	return file;
}

test.use({
	launchOptions: {
		args: [
			'--use-fake-ui-for-media-stream',
			'--use-fake-device-for-media-stream',
			`--use-file-for-fake-audio-capture=${speechWav()}`
		]
	}
});

test('a wake phrase switches persona and runs the command; other speech is ignored', async ({ page }) => {
	await openSettings(page);
	await page.getByTestId('mode-wake_word').check();
	await section(page, 'voice').getByTestId('voice-save').click();
	await expect(section(page, 'voice').getByTestId('voice-feedback')).toContainText('Voice settings saved.');

	await openHud(page);
	await page.getByTestId('handsfree-start').click();
	await expect(page.getByTestId('handsfree-bar')).toHaveAttribute('data-active', 'true');

	// A burst that is not addressed to anyone: nothing happens.
	await page.evaluate(() => (window as unknown as MockWindow).__jarvisMock.say('did you see the game last night'));
	await page.waitForTimeout(6500);
	await expect(page.getByTestId('orb-persona')).toHaveText('Jarvis');
	await expect(page.getByRole('log').getByText('game last night')).toHaveCount(0);

	// The next burst is addressed to Friday: the persona switches and the command runs.
	await page.evaluate(() => (window as unknown as MockWindow).__jarvisMock.say('hey friday what is gold at'));
	await expect(page.getByTestId('orb-persona')).toHaveText('Friday', { timeout: 15_000 });
	await expect(page.getByRole('log').getByText('what is gold at')).toBeVisible({ timeout: 15_000 });
	await expect(page.getByRole('log').getByText(/XAUUSD is [\d,.]+ bid/)).toBeVisible({ timeout: 15_000 });
	await page.getByTestId('handsfree-stop').click();
	await expect(page.getByTestId('handsfree-bar')).toHaveAttribute('data-active', 'false');
});
