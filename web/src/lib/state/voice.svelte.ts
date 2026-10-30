import { api, type InputMode, type TranscribeResponse } from '#lib/api.ts';
import { describeError, toApiError } from '#lib/errors.ts';
import { Recorder, micErrorMessage, primeAudio } from '#lib/voice/audio.ts';
import { routeTranscript } from '#lib/voice/intent.ts';
import { MAX_SECONDS } from '#lib/voice/wav.ts';
import { chat } from './chat.svelte';
import { backend } from './hud.svelte';
import { orb } from './orb.svelte';
import { preview } from './preview.svelte';
import { settings } from './settings.svelte';
import { speech } from './speech.svelte';
import { sendTurn } from './turn.svelte';

export type VoicePhase = 'idle' | 'starting' | 'listening' | 'transcribing';

/** Held at least this long = push-to-talk (release stops); shorter = tap-to-toggle. */
export const HOLD_MS = 350;
/** Clips shorter than this are discarded: a stray tap, not speech. */
const MIN_CLIP_MS = 400;

/** Linear mic level -> orb amplitude. Speech sits around 0.02-0.2 RMS. */
const levelToOrb = (rms: number) => Math.min(1, rms * 6);

/**
 * Microphone input: press-and-hold or tap-to-toggle (mouse, touch, Space bar), capture,
 * transcription, and routing of the words to confirm / cancel / chat.
 */
class VoiceStore {
	phase = $state<VoicePhase>('idle');
	/** Smoothed mic level 0..1 for the button's meter. */
	level = $state(0);
	elapsedMs = $state(0);
	/** Inline message under the composer (permission denied, nothing heard...). */
	message = $state<string | null>(null);

	private recorder: Recorder | null = null;
	private pressAt = 0;
	private holding = false;
	/** How long the last press lasted once released (null while still held). */
	private heldMs: number | null = null;
	private starting: Promise<void> | null = null;
	private clock: ReturnType<typeof setInterval> | undefined;

	/** The microphone can be used: the server has STT and is reachable. */
	get enabled(): boolean {
		return backend.sttAvailable && !backend.offline;
	}

	/**
	 * How the mic button behaves (settings.voice.input_mode). "hold" is push-to-talk (a quick tap
	 * still toggles, as before), "toggle" is tap to start and tap to stop, and "wake_word" hands the
	 * microphone to hands-free mode (see handsfree.svelte.ts), so push-to-talk steps aside.
	 */
	get mode(): InputMode {
		return settings.data?.voice.input_mode ?? 'hold';
	}

	get listening(): boolean {
		return this.phase === 'listening' || this.phase === 'starting';
	}

	get busy(): boolean {
		return this.phase !== 'idle';
	}

	/** Pointer/key went down on the mic (or the Space bar). */
	press(): void {
		if (!this.enabled || this.mode === 'wake_word' || chat.pending || this.phase === 'transcribing') return;
		primeAudio();
		speech.stop(); // barge-in: talking over the reply silences it
		if (this.phase === 'listening') {
			// A second tap while toggled on = stop.
			void this.finish();
			return;
		}
		if (this.phase === 'starting') return;
		this.holding = true;
		this.heldMs = null;
		this.pressAt = performance.now();
		this.message = null;
		this.starting = this.begin();
	}

	/** Pointer/key came up. Long hold = push-to-talk release; a tap leaves it recording. */
	release(): void {
		if (!this.holding) return;
		this.holding = false;
		this.heldMs = performance.now() - this.pressAt;
		if (this.heldMs >= HOLD_MS && this.mode !== 'toggle') void this.finish();
	}

	/** Esc, or the page is going away: throw the clip away. */
	abort(): void {
		this.holding = false;
		if (this.recorder) {
			this.recorder.cancel();
			this.recorder = null;
		}
		this.stopClock();
		this.phase = 'idle';
		this.level = 0;
		if (orb.state === 'listening') orb.set('idle');
	}

	private async begin(): Promise<void> {
		this.phase = 'starting';
		try {
			const rec = await Recorder.start({
				onLevel: (rms) => {
					const v = levelToOrb(rms);
					this.level = v;
					orb.setLevel(v);
				},
				onLimit: () => {
					this.message = `Stopped at ${MAX_SECONDS} s, the longest clip.`;
					void this.finish();
				}
			});
			if (this.phase !== 'starting') {
				// Aborted while the permission prompt was open.
				rec.cancel();
				return;
			}
			this.recorder = rec;
			this.phase = 'listening';
			this.elapsedMs = 0;
			orb.set('listening');
			this.clock = setInterval(() => (this.elapsedMs = rec.elapsedMs), 100);
			// A long hold that ended while the microphone was still starting (permission prompt)
			// has no later release to wait for; a short tap stays on until the next tap.
			if (this.heldMs !== null && this.heldMs >= HOLD_MS && this.mode !== 'toggle') void this.finish();
		} catch (e) {
			this.phase = 'idle';
			this.message = micErrorMessage(e);
			orb.flashError();
		}
	}

	private async finish(): Promise<void> {
		await this.starting;
		const rec = this.recorder;
		if (!rec || this.phase !== 'listening') return;
		this.recorder = null;
		this.stopClock();
		this.level = 0;
		this.phase = 'transcribing';
		orb.set('thinking');
		try {
			const { wav, durationMs } = await rec.stop();
			if (durationMs < MIN_CLIP_MS) {
				this.message = 'That was too short. Hold the mic (or Space) while you speak.';
				orb.set('idle');
				return;
			}
			const res = await api.transcribe(new Blob([wav], { type: 'audio/wav' }));
			await this.route(res);
		} catch (e) {
			const err = toApiError(e);
			this.message =
				err.code === 'voice_unavailable'
					? 'Voice is not available on the server.'
					: `Could not transcribe that. ${describeError(err)}`;
			orb.flashError();
		} finally {
			this.phase = 'idle';
			if (orb.state === 'thinking' && !chat.pending) orb.set('idle');
		}
	}

	/** Act on what was said: confirm or cancel the open preview, or send the words to chat. */
	async route(res: Pick<TranscribeResponse, 'text' | 'intent'>): Promise<void> {
		const decision = routeTranscript(res, preview.pendingState());
		switch (decision.kind) {
			case 'empty':
				this.message = "Didn't catch that. Try again.";
				break;
			case 'confirm':
				chat.addUser(res.text.trim(), true);
				await preview.confirm();
				break;
			case 'cancel':
				chat.addUser(res.text.trim(), true);
				await preview.cancel();
				break;
			case 'expired':
				chat.addUser(res.text.trim(), true);
				chat.note('That confirmation has expired. Ask again to get a fresh preview.', 'warn');
				break;
			case 'chat':
				// The transcript line appears at once; the reply is read aloud (voice turn).
				chat.addUser(decision.text, true);
				this.phase = 'idle';
				await sendTurn(decision.text, { voice: true, alreadyShown: true });
				break;
		}
	}

	private stopClock(): void {
		clearInterval(this.clock);
		this.clock = undefined;
	}
}

export const voice = new VoiceStore();

/** Space-bar push-to-talk. Returns the cleanup. Ignored while typing or on interactive controls. */
export function attachPushToTalk(): () => void {
	const interactive = (t: EventTarget | null) => {
		if (!(t instanceof HTMLElement)) return false;
		return (
			t.isContentEditable ||
			t.closest('input, textarea, select, button, a[href], summary, [role="button"], [role="tab"], [role="menuitem"], [role="menuitemcheckbox"]') !== null
		);
	};
	const down = (e: KeyboardEvent) => {
		if (e.code === 'Escape' && voice.listening) return voice.abort();
		if (e.code !== 'Space' || e.repeat || e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return;
		if (interactive(e.target) || !voice.enabled) return;
		e.preventDefault();
		voice.press();
	};
	const up = (e: KeyboardEvent) => {
		if (e.code !== 'Space') return;
		voice.release();
	};
	const hide = () => {
		if (document.hidden) voice.abort();
	};
	window.addEventListener('keydown', down);
	window.addEventListener('keyup', up);
	document.addEventListener('visibilitychange', hide);
	return () => {
		window.removeEventListener('keydown', down);
		window.removeEventListener('keyup', up);
		document.removeEventListener('visibilitychange', hide);
		voice.abort();
	};
}
