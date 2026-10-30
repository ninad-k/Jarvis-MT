import { describe, expect, it } from 'vitest';
import { VadSegmenter, VAD_DEFAULTS, type VadEvent } from './vad-core.js';

// ---- Synthetic signals ------------------------------------------------------

/** Small deterministic PRNG so noise is the same on every run. */
function prng(seed: number) {
	let s = seed >>> 0;
	return () => {
		s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
		return s / 0xffffffff - 0.5;
	};
}

const rand = prng(7);

function silence(ms: number, rate: number, noise = 0.002): Float32Array {
	const out = new Float32Array(Math.round((rate * ms) / 1000));
	for (let i = 0; i < out.length; i++) out[i] = rand() * 2 * noise;
	return out;
}

/** A voiced-sounding burst: a 180 Hz tone with a syllable-rate (3.5 Hz) envelope, `amp` peak. */
function speech(ms: number, rate: number, amp = 0.25): Float32Array {
	const out = new Float32Array(Math.round((rate * ms) / 1000));
	for (let i = 0; i < out.length; i++) {
		const t = i / rate;
		out[i] = amp * (0.6 + 0.4 * Math.sin(2 * Math.PI * 3.5 * t)) * Math.sin(2 * Math.PI * 180 * t);
	}
	return out;
}

function join(...parts: Float32Array[]): Float32Array {
	const out = new Float32Array(parts.reduce((n, p) => n + p.length, 0));
	let o = 0;
	for (const p of parts) {
		out.set(p, o);
		o += p.length;
	}
	return out;
}

/** Feed `signal` in blocks of `block` samples (128 = what an AudioWorklet delivers). */
function run(vad: VadSegmenter, signal: Float32Array, block = 128): VadEvent[] {
	const events: VadEvent[] = [];
	for (let i = 0; i < signal.length; i += block) events.push(...vad.push(signal.subarray(i, i + block)));
	return events;
}

const segments = (events: VadEvent[]) =>
	events.filter((e): e is Extract<VadEvent, { type: 'segment' }> => e.type === 'segment');

const RATES = [16_000, 44_100, 48_000];

describe.each(RATES)('VadSegmenter at %i Hz', (rate) => {
	const vad = () => new VadSegmenter(rate);

	it('stays quiet on silence and low room noise', () => {
		const events = run(vad(), join(silence(3000, rate), silence(2000, rate, 0.006)));
		expect(events).toEqual([]);
	});

	it('cuts one segment out of speech between silences', () => {
		const events = run(vad(), join(silence(1000, rate), speech(1500, rate), silence(1500, rate)));
		expect(events.map((e) => e.type)).toEqual(['start', 'segment']);
		const [seg] = segments(events);
		expect(seg.reason).toBe('silence');
		// pre-roll (300) + speech (1500) + tail (200), within a frame or two
		expect(seg.durationMs).toBeGreaterThan(1900);
		expect(seg.durationMs).toBeLessThan(2100);
		expect(seg.samples.length).toBe(Math.round((seg.durationMs * rate) / 1000));
	});

	it('waits about 700 ms of silence before closing', () => {
		const v = vad();
		const early = run(v, join(silence(500, rate), speech(1000, rate), silence(600, rate)));
		expect(segments(early)).toHaveLength(0);
		expect(v.speaking).toBe(true);
		const late = run(v, silence(200, rate));
		expect(segments(late)).toHaveLength(1);
		expect(v.speaking).toBe(false);
	});

	it('drops a burst shorter than 400 ms', () => {
		const events = run(vad(), join(silence(1000, rate), speech(250, rate), silence(1500, rate)));
		expect(segments(events)).toHaveLength(0);
		expect(events.map((e) => e.type)).toEqual(['start', 'discard']);
	});

	it('ignores a click shorter than the start window', () => {
		const events = run(vad(), join(silence(1000, rate), speech(30, rate, 0.8), silence(1500, rate)));
		expect(events).toEqual([]);
	});

	it('keeps a speech burst of 450 ms', () => {
		const events = run(vad(), join(silence(1000, rate), speech(450, rate), silence(1500, rate)));
		expect(segments(events)).toHaveLength(1);
	});

	it('joins words separated by a pause shorter than the silence window', () => {
		const events = run(
			vad(),
			join(silence(800, rate), speech(700, rate), silence(400, rate), speech(700, rate), silence(1500, rate))
		);
		expect(segments(events)).toHaveLength(1);
		expect(segments(events)[0].durationMs).toBeGreaterThan(1900);
	});

	it('splits utterances separated by more than the silence window', () => {
		const events = run(
			vad(),
			join(silence(800, rate), speech(800, rate), silence(1200, rate), speech(800, rate), silence(1500, rate))
		);
		expect(segments(events)).toHaveLength(2);
	});

	it('cuts at 8 s and carries on with the rest as a new segment', () => {
		const events = run(vad(), join(silence(500, rate), speech(12_000, rate), silence(1500, rate)));
		const segs = segments(events);
		expect(segs.length).toBe(2);
		expect(segs[0].reason).toBe('max');
		expect(segs[0].durationMs).toBeLessThanOrEqual(VAD_DEFAULTS.maxSegmentMs + 40);
		expect(segs[0].durationMs).toBeGreaterThan(VAD_DEFAULTS.maxSegmentMs - 40);
		expect(segs[1].reason).toBe('silence');
		expect(segs[1].durationMs).toBeGreaterThan(3500);
	});

	it('includes the pre-roll so the first syllable is not clipped', () => {
		const events = run(vad(), join(silence(1000, rate), speech(800, rate), silence(1500, rate)));
		const [seg] = segments(events);
		// 800 ms of speech plus 300 ms pre-roll plus the tail: well over the speech alone.
		expect(seg.durationMs).toBeGreaterThan(1250);
		// The first 200 ms are the quiet lead-in.
		let peak = 0;
		for (let i = 0; i < Math.round(rate * 0.2); i++) peak = Math.max(peak, Math.abs(seg.samples[i]));
		expect(peak).toBeLessThan(0.05);
	});

	it('gives the same result whatever the block size', () => {
		const signal = join(
			silence(700, rate),
			speech(1200, rate),
			silence(1500, rate),
			speech(900, rate),
			silence(1500, rate)
		);
		const a = segments(run(vad(), signal, 128));
		const b = segments(run(vad(), signal, 441));
		const c = segments(run(vad(), signal, signal.length));
		expect(a.length).toBe(2);
		expect(b.map((s) => s.samples.length)).toEqual(a.map((s) => s.samples.length));
		expect(c.map((s) => s.samples.length)).toEqual(a.map((s) => s.samples.length));
	});

	it('adapts to a steady noise floor instead of treating it as speech', () => {
		const hum = silence(2000, rate, 0.03);
		expect(run(vad(), hum)).toEqual([]);
	});

	it('still hears speech over background noise', () => {
		const bg = (ms: number) => silence(ms, rate, 0.01);
		const talk = speech(1000, rate, 0.3);
		const mixed = new Float32Array(talk.length);
		const n = bg(1000);
		for (let i = 0; i < talk.length; i++) mixed[i] = talk[i] + n[i];
		const events = run(vad(), join(bg(1500), mixed, bg(1500)));
		expect(segments(events)).toHaveLength(1);
	});

	it('reset() drops an open segment and starts clean', () => {
		const v = vad();
		run(v, join(silence(500, rate), speech(600, rate)));
		expect(v.speaking).toBe(true);
		v.reset();
		expect(v.speaking).toBe(false);
		expect(segments(run(v, silence(2000, rate)))).toHaveLength(0);
	});
});

describe('VadSegmenter thresholds', () => {
	it('never goes below the absolute minimums in a silent room', () => {
		const v = new VadSegmenter(16_000);
		run(v, silence(2000, 16_000, 0));
		expect(v.onThreshold).toBeGreaterThanOrEqual(VAD_DEFAULTS.minOn);
		expect(v.offThreshold).toBeGreaterThanOrEqual(VAD_DEFAULTS.minOff);
	});

	it('raises the thresholds with moderate room noise', () => {
		const v = new VadSegmenter(16_000);
		run(v, silence(4000, 16_000, 0.03)); // about 0.017 rms, under the absolute minimum
		expect(v.noiseFloor).toBeGreaterThan(0.01);
		expect(v.noiseFloor).toBeLessThanOrEqual(VAD_DEFAULTS.maxFloor);
		expect(v.onThreshold).toBeGreaterThan(VAD_DEFAULTS.minOn);
	});

	it('learns steady noise louder than the minimum after one 8 s cut, without sending it', () => {
		const v = new VadSegmenter(16_000);
		const events = run(v, silence(14_000, 16_000, 0.05)); // about 0.029 rms: opens a segment
		expect(segments(events)).toHaveLength(0);
		expect(events.filter((e) => e.type === 'discard')).toEqual([{ type: 'discard', reason: 'noise' }]);
		expect(v.noiseFloor).toBeGreaterThan(0.02);
		// Speech over the learned noise is still heard.
		const talk = run(v, join(silence(500, 16_000, 0.05), speech(1200, 16_000, 0.5), silence(1500, 16_000, 0.05)));
		expect(segments(talk)).toHaveLength(1);
	});

	it('does not learn speech as noise', () => {
		const v = new VadSegmenter(16_000);
		run(v, join(silence(1000, 16_000), speech(3000, 16_000), silence(1500, 16_000)));
		expect(v.noiseFloor).toBeLessThan(0.01);
	});
});
