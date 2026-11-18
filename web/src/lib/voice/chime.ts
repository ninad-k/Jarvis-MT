/**
 * The short, quiet sound for a price alert. It is made with the Web Audio API, so there is no
 * audio file to load (the CSP only allows same-origin scripts and this needs nothing else).
 */

export interface ChimeNote {
	/** Hertz. */
	freq: number;
	/** Seconds after the chime starts. */
	start: number;
	/** Seconds the note rings for. */
	dur: number;
}

/** Peak gain of one note: audible over a quiet room, never startling. */
export const CHIME_GAIN = 0.06;

/** Two rising notes, 240 ms in all. Kept under 300 ms on purpose. */
export const CHIME: readonly ChimeNote[] = [
	{ freq: 880, start: 0, dur: 0.11 },
	{ freq: 1318.5, start: 0.09, dur: 0.15 }
];

export function chimeLength(notes: readonly ChimeNote[] = CHIME): number {
	return Math.max(0, ...notes.map((n) => n.start + n.dur));
}

/**
 * Play the chime on a running context. Returns false (and plays nothing) when the context is not
 * running, which is what a browser gives before the user has touched the page.
 */
export function playChime(ctx: AudioContext | null, notes: readonly ChimeNote[] = CHIME): boolean {
	if (!ctx || ctx.state !== 'running') return false;
	const t0 = ctx.currentTime + 0.01;
	for (const n of notes) {
		const osc = ctx.createOscillator();
		const gain = ctx.createGain();
		osc.type = 'sine';
		osc.frequency.value = n.freq;
		// A quick attack and a decay to silence avoid clicks at both ends.
		gain.gain.setValueAtTime(0.0001, t0 + n.start);
		gain.gain.linearRampToValueAtTime(CHIME_GAIN, t0 + n.start + 0.015);
		gain.gain.exponentialRampToValueAtTime(0.0001, t0 + n.start + n.dur);
		osc.connect(gain);
		gain.connect(ctx.destination);
		osc.start(t0 + n.start);
		osc.stop(t0 + n.start + n.dur + 0.02);
		osc.onended = () => {
			osc.disconnect();
			gain.disconnect();
		};
	}
	return true;
}
