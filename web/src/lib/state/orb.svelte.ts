export type OrbState = 'idle' | 'listening' | 'thinking' | 'speaking' | 'error';

export const ORB_LABEL: Record<OrbState, string> = {
	idle: 'Standing by',
	listening: 'Listening…',
	thinking: 'Thinking…',
	speaking: 'Speaking',
	error: 'Something went wrong'
};

const reduced = () =>
	typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;

/**
 * Single source of truth for the orb. The chat store and the voice stores drive it:
 * `listening` follows the live microphone level, `speaking` follows the playing TTS audio's level
 * (`speakLive`), and `speak(ms)` is a synthetic envelope kept for dev pinning.
 */
class OrbStore {
	state = $state<OrbState>('idle');

	private timer: ReturnType<typeof setTimeout> | undefined;
	private speakStart = 0;
	private speakMs = 0;
	private level = 0;
	/** True while `speaking` is driven by a real audio level instead of the synthetic envelope. */
	private live = false;

	get label(): string {
		return ORB_LABEL[this.state];
	}

	set(next: OrbState): void {
		clearTimeout(this.timer);
		this.live = false;
		this.state = next;
		if (next !== 'listening' && next !== 'speaking') this.level = 0;
	}

	/** Externally measured audio level (0..1), used while listening or speaking real audio. */
	setLevel(v: number): void {
		this.level = Math.min(1, Math.max(0, v));
	}

	/** Speaking driven by `setLevel()` from real playback; stays until `set()` is called. */
	speakLive(): void {
		this.set('speaking');
		this.live = true;
	}

	/** Synthetic speech envelope for ~`ms`, then back to idle (dev pinning only). */
	speak(ms = 1500): void {
		this.set('speaking');
		this.speakStart = performance.now();
		this.speakMs = ms;
		this.timer = setTimeout(() => this.set('idle'), ms);
	}

	/** Brief red flash, then back to idle. */
	flashError(ms = 1100): void {
		this.set('error');
		this.timer = setTimeout(() => this.set('idle'), ms);
	}

	/** Current amplitude 0..1. Read every frame by the renderer, so intentionally not reactive. */
	get amplitude(): number {
		if (this.state === 'speaking' && this.live) return this.level;
		if (this.state === 'speaking') {
			if (reduced()) return 0.4;
			const t = (performance.now() - this.speakStart) / 1000;
			const total = this.speakMs / 1000;
			const attack = Math.min(1, t / 0.12);
			const release = Math.min(1, Math.max(0, (total - t) / 0.3));
			// Syllable-like bursts: two incommensurate sines, rectified.
			const syll = Math.abs(Math.sin(t * 11.3)) * (0.55 + 0.45 * Math.abs(Math.sin(t * 4.1 + 1)));
			return Math.min(1, attack * release * (0.25 + 0.75 * syll));
		}
		return this.state === 'listening' ? this.level : 0;
	}
}

export const orb = new OrbStore();
