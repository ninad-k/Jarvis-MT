import { api, type TranscribeResponse } from '#lib/api.ts';
import { describeError, toApiError } from '#lib/errors.ts';
import { micErrorMessage, primeAudio } from '#lib/voice/audio.ts';
import { HandsFreeListener } from '#lib/voice/listener.ts';
import { routeWake } from '#lib/voice/wake.ts';
import { chunksToWav16k } from '#lib/voice/wav.ts';
import { backend } from './hud.svelte';
import { orb } from './orb.svelte';
import { persona } from './persona.svelte';
import { preview } from './preview.svelte';
import { settings } from './settings.svelte';
import { voice } from './voice.svelte';

export type HandsFreePhase = 'off' | 'starting' | 'listening' | 'hearing' | 'transcribing';

/** After a bare wake phrase the next segment is the command; give up after this long. */
export const COMMAND_WINDOW_MS = 8000;
/** How long "<Name> is listening" stays up after a wake phrase that came with its command. */
const AWAKE_FLASH_MS = 2500;
/** The microphone stays deaf this long after speech playback ends, so the room echo is not heard. */
const ECHO_TAIL_MS = 400;
/** The server allows 20 transcriptions a minute; stay under it. */
const MAX_PER_MINUTE = 15;

/**
 * Hands-free mode (settings.voice.input_mode = "wake_word"). The listener cuts speech out of the
 * open microphone; each segment is sent to `POST /api/voice/transcribe?wake=1`, and only a segment
 * that starts with a wake phrase (or the command that follows a bare one) is acted on. What counts,
 * and what is ignored while a confirmation is open, is decided by `routeWake` (tested).
 */
class HandsFreeStore {
	phase = $state<HandsFreePhase>('off');
	/** Mic level 0..1 for the indicator. */
	level = $state(0);
	/** Why hearing is paused right now, or null. */
	pausedReason = $state<string | null>(null);
	/** Set while "<Name> is listening" is on screen. */
	awake = $state<{ name: string; until: number; waitingForCommand: boolean } | null>(null);
	message = $state<string | null>(null);

	private listener: HandsFreeListener | null = null;
	private ctrl: AbortController | null = null;
	private awakeTimer: ReturnType<typeof setTimeout> | undefined;
	private resumeTimer: ReturnType<typeof setTimeout> | undefined;
	private cooldownUntil = 0;
	private sent: number[] = [];
	private blockedUntil = 0;
	private starting: Promise<void> | null = null;

	/** The setting asks for hands-free mode. */
	get wanted(): boolean {
		return settings.data?.voice.input_mode === 'wake_word';
	}

	/** Hands-free can be used: the mode is on, the server has speech-to-text and is reachable. */
	get enabled(): boolean {
		return this.wanted && backend.sttAvailable && !backend.offline;
	}

	get active(): boolean {
		return this.phase !== 'off';
	}

	get unavailableReason(): string | null {
		if (backend.offline) return 'Backend offline';
		if (!backend.sttAvailable) return 'Speech recognition is not available on this server';
		return null;
	}

	toggle(): void {
		if (this.active) this.stop();
		else void this.start();
	}

	start(): Promise<void> {
		this.starting ??= this.doStart().finally(() => (this.starting = null));
		return this.starting;
	}

	private async doStart(): Promise<void> {
		if (this.phase !== 'off' || !this.enabled) return;
		primeAudio();
		this.phase = 'starting';
		this.message = null;
		try {
			const l = await HandsFreeListener.start({
				onLevel: (rms) => {
					this.level = Math.min(1, rms * 6);
					if (this.awake?.waitingForCommand) orb.setLevel(this.level);
				},
				onSpeechStart: () => {
					if (this.phase === 'listening' && !this.pausedReason) this.phase = 'hearing';
				},
				onDiscard: () => {
					if (this.phase === 'hearing') this.phase = 'listening';
				},
				onSegment: (samples, rate) => void this.onSegment(samples, rate)
			});
			if (this.phase !== 'starting') {
				// stop() was called while the permission prompt was open.
				l.stop();
				return;
			}
			this.listener = l;
			this.phase = 'listening';
			this.syncPause();
		} catch (e) {
			this.phase = 'off';
			this.message = micErrorMessage(e);
		}
	}

	/** One-click stop. Releases the microphone. */
	stop(): void {
		this.listener?.stop();
		this.listener = null;
		this.ctrl?.abort();
		this.ctrl = null;
		clearTimeout(this.awakeTimer);
		clearTimeout(this.resumeTimer);
		this.awake = null;
		this.level = 0;
		this.pausedReason = null;
		if (this.phase !== 'off' && orb.state === 'listening') orb.set('idle');
		this.phase = 'off';
	}

	/**
	 * Called by the HUD whenever what the assistant is doing changes. Hearing is paused while a
	 * reply is read aloud (the microphone would hear it) and while a reply is being produced.
	 */
	setBusy(flags: { speaking: boolean; thinking: boolean }): void {
		const was = this.pausedReason;
		if (flags.speaking) {
			this.pausedReason = `Paused while ${persona.name} is speaking`;
			this.cooldownUntil = Date.now() + ECHO_TAIL_MS;
		} else if (flags.thinking) {
			this.pausedReason = `${persona.name} is working on it`;
		} else if (was !== null) {
			// Speech just ended: stay deaf for the echo tail, then resume.
			const wait = this.cooldownUntil - Date.now();
			if (wait > 0) {
				clearTimeout(this.resumeTimer);
				this.resumeTimer = setTimeout(() => {
					this.pausedReason = null;
					this.syncPause();
				}, wait);
				return;
			}
			this.pausedReason = null;
		}
		clearTimeout(this.resumeTimer);
		this.syncPause();
	}

	private syncPause(): void {
		const paused = this.pausedReason !== null;
		this.listener?.pause(paused);
		if (paused && this.phase === 'hearing') this.phase = 'listening';
	}

	private canSend(): boolean {
		const now = Date.now();
		if (now < this.blockedUntil) return false;
		this.sent = this.sent.filter((t) => now - t < 60_000);
		if (this.sent.length >= MAX_PER_MINUTE) return false;
		this.sent.push(now);
		return true;
	}

	private async onSegment(samples: Float32Array, rate: number): Promise<void> {
		if (this.phase === 'off' || this.pausedReason !== null || this.phase === 'transcribing') {
			if (this.phase === 'hearing') this.phase = 'listening';
			return;
		}
		if (!this.canSend()) {
			this.phase = 'listening';
			return;
		}
		this.phase = 'transcribing';
		const ctrl = new AbortController();
		this.ctrl = ctrl;
		let res: TranscribeResponse;
		try {
			const wav = new Blob([chunksToWav16k([samples], rate)], { type: 'audio/wav' });
			res = await api.transcribe(wav, ctrl.signal, { wake: true });
		} catch (e) {
			const err = toApiError(e);
			if (err.code === 'rate_limited') {
				this.blockedUntil = Date.now() + (err.retryAfterMs ?? 15_000);
				this.message = 'Too many speech segments. Hands-free is resting for a moment.';
			} else if (err.code === 'voice_unavailable') {
				this.message = 'Voice is not available on the server.';
				this.stop();
			} else if (err.code !== 'aborted' && !err.isOffline) {
				this.message = `Could not transcribe that. ${describeError(err)}`;
			}
			return;
		} finally {
			if (this.ctrl === ctrl) this.ctrl = null;
			if (this.phase === 'transcribing') this.phase = 'listening';
		}
		if (ctrl.signal.aborted || !this.active) return;
		await this.handle(res);
	}

	private async handle(res: TranscribeResponse): Promise<void> {
		const waiting = this.awake?.waitingForCommand === true && this.awake.until > Date.now();
		const route = routeWake(res, {
			pending: preview.pendingState(),
			awaitingCommand: waiting,
			activePersona: persona.activeId
		});
		if (route.kind === 'ignore') {
			// The server switches persona on any wake phrase. If we ignored one, show what it did.
			if (route.reason === 'other-persona') void settings.refresh();
			return;
		}
		this.message = null;
		if (route.persona && route.persona !== persona.activeId) persona.adoptFromServer(route.persona);
		const name = persona.nameFor(route.persona ?? persona.activeId ?? undefined);

		if (route.kind === 'arm') {
			this.wake(name, COMMAND_WINDOW_MS, true);
			return;
		}
		if (!waiting || route.persona) this.wake(name, AWAKE_FLASH_MS, false);
		else this.clearAwake();
		await voice.route({ text: route.text, intent: route.intent });
	}

	/** Show "<Name> is listening" and, for a bare wake phrase, wait for the command. */
	private wake(name: string, ms: number, waitingForCommand: boolean): void {
		clearTimeout(this.awakeTimer);
		this.awake = { name, until: Date.now() + ms, waitingForCommand };
		if (waitingForCommand) orb.set('listening');
		this.awakeTimer = setTimeout(() => this.clearAwake(), ms);
	}

	private clearAwake(): void {
		clearTimeout(this.awakeTimer);
		if (this.awake?.waitingForCommand && orb.state === 'listening') orb.set('idle');
		this.awake = null;
	}
}

export const handsfree = new HandsFreeStore();
