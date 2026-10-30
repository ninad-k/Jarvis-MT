import { api } from '#lib/api.ts';
import { describeError, toApiError } from '#lib/errors.ts';
import { playBlob, primeAudio, type Playback } from '#lib/voice/audio.ts';
import { clipForSpeech } from '#lib/voice/text.ts';
import { backend } from './hud.svelte';
import { orb } from './orb.svelte';
import { settings } from './settings.svelte';
import { toasts } from './toasts.svelte';

const KEY = 'jarvis.readAloud';

function read(): boolean {
	try {
		return localStorage.getItem(KEY) === '1';
	} catch {
		return false;
	}
}

/** Text-to-speech playback. The orb's `speaking` state follows the real audio level. */
class SpeechStore {
	/** Browser-local copy, used only when the server has no settings. */
	private local = $state(read());
	speaking = $state(false);

	private playback: Playback | null = null;
	private ctrl: AbortController | null = null;

	get available(): boolean {
		return backend.ttsAvailable;
	}

	/** "Read replies aloud": the user's server-side setting, or this browser's when there is none. */
	get readAloud(): boolean {
		return settings.data ? settings.data.voice.read_aloud : this.local;
	}

	setReadAloud(on: boolean): void {
		if (on) primeAudio();
		else this.stop();
		const s = settings.data;
		if (!s) {
			this.local = on;
			try {
				localStorage.setItem(KEY, on ? '1' : '0');
			} catch {
				/* not persisted */
			}
			return;
		}
		// The nested object goes whole: the server contract does not promise a deep merge.
		settings
			.update(
				{ voice: { read_aloud: on, input_mode: s.voice.input_mode, language: s.voice.language } },
				{ optimistic: true }
			).catch((e) => {
			toasts.error(`Could not save the setting. ${describeError(toApiError(e))}`);
		});
	}

	/**
	 * Fetch and play `text`. Resolves when playback ends or fails; never throws, so callers can
	 * always continue (for example to start a confirmation window).
	 */
	async speak(text: string, persona?: string, force = false): Promise<boolean> {
		const clip = clipForSpeech(text);
		// `force`: the caller already knows speech works (the settings page reads /api/status itself).
		if ((!this.available && !force) || clip.length === 0) return false;
		this.stop();
		const ctrl = new AbortController();
		this.ctrl = ctrl;
		try {
			const blob = await api.speak(clip, ctrl.signal, persona);
			if (ctrl.signal.aborted) return false;
			this.speaking = true;
			orb.speakLive();
			const playback = await playBlob(blob, (v) => orb.setLevel(Math.min(1, v * 4)));
			this.playback = playback;
			await playback.done;
			return true;
		} catch (e) {
			const err = toApiError(e);
			if (err.code !== 'aborted') toasts.error(`Could not read the reply aloud. ${describeError(err)}`);
			return false;
		} finally {
			if (this.ctrl === ctrl) {
				this.ctrl = null;
				this.playback = null;
				this.speaking = false;
				if (orb.state === 'speaking') orb.set('idle');
			}
		}
	}

	/** Stop playback now (barge-in: the user pressed the mic). */
	stop(): void {
		this.ctrl?.abort();
		this.playback?.stop();
	}
}

export const speech = new SpeechStore();
