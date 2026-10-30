import { describe, expect, it } from 'vitest';
import {
	MAX_SECONDS,
	TARGET_RATE,
	chunksToWav16k,
	concat,
	encodeWav,
	resample,
	rms,
	toneWav,
	wavDurationMs
} from './wav';

const ascii = (v: DataView, off: number, n: number) =>
	String.fromCharCode(...Array.from({ length: n }, (_, i) => v.getUint8(off + i)));

function sine(rate: number, hz: number, seconds: number, amp = 0.5): Float32Array {
	const out = new Float32Array(Math.round(rate * seconds));
	for (let i = 0; i < out.length; i++) out[i] = amp * Math.sin((2 * Math.PI * hz * i) / rate);
	return out;
}

describe('encodeWav', () => {
	it('writes a canonical 44-byte PCM header', () => {
		const buf = encodeWav(new Float32Array([0, 0.5, -0.5, 1, -1]), 16_000);
		const v = new DataView(buf);
		expect(buf.byteLength).toBe(44 + 5 * 2);
		expect(ascii(v, 0, 4)).toBe('RIFF');
		expect(v.getUint32(4, true)).toBe(36 + 10);
		expect(ascii(v, 8, 4)).toBe('WAVE');
		expect(ascii(v, 12, 4)).toBe('fmt ');
		expect(v.getUint32(16, true)).toBe(16);
		expect(v.getUint16(20, true)).toBe(1); // PCM
		expect(v.getUint16(22, true)).toBe(1); // mono
		expect(v.getUint32(24, true)).toBe(16_000);
		expect(v.getUint32(28, true)).toBe(32_000);
		expect(v.getUint16(32, true)).toBe(2);
		expect(v.getUint16(34, true)).toBe(16);
		expect(ascii(v, 36, 4)).toBe('data');
		expect(v.getUint32(40, true)).toBe(10);
	});

	it('converts samples to little-endian int16 and clips out-of-range input', () => {
		const v = new DataView(encodeWav(new Float32Array([0, 0.5, -0.5, 1, -1, 2, -2]), 16_000));
		const at = (i: number) => v.getInt16(44 + i * 2, true);
		expect(at(0)).toBe(0);
		expect(at(1)).toBe(16384);
		expect(at(2)).toBe(-16384);
		expect(at(3)).toBe(32767);
		expect(at(4)).toBe(-32768);
		expect(at(5)).toBe(32767);
		expect(at(6)).toBe(-32768);
	});

	it('encodes empty input as a header-only file', () => {
		const buf = encodeWav(new Float32Array(0), 16_000);
		expect(buf.byteLength).toBe(44);
		expect(wavDurationMs(buf)).toBe(0);
	});
});

describe('resample', () => {
	it('returns a copy when the rates match', () => {
		const a = new Float32Array([0.1, 0.2, 0.3]);
		const b = resample(a, 16_000, 16_000);
		expect(Array.from(b)).toEqual(Array.from(a));
		expect(b).not.toBe(a);
	});

	it('produces the expected length for 48 kHz and 44.1 kHz sources', () => {
		expect(resample(new Float32Array(48_000), 48_000, TARGET_RATE).length).toBe(16_000);
		expect(resample(new Float32Array(44_100), 44_100, TARGET_RATE).length).toBe(16_000);
		expect(resample(new Float32Array(24_000), 24_000, TARGET_RATE).length).toBe(16_000);
	});

	it('preserves a DC level when downsampling', () => {
		const out = resample(new Float32Array(4800).fill(0.25), 48_000, 16_000);
		for (const s of out) expect(s).toBeCloseTo(0.25, 6);
	});

	it('keeps a 440 Hz tone at about the same RMS after 48k -> 16k', () => {
		const src = sine(48_000, 440, 0.5);
		const out = resample(src, 48_000, 16_000);
		expect(rms(out)).toBeGreaterThan(rms(src) * 0.95);
		expect(rms(out)).toBeLessThan(rms(src) * 1.05);
	});

	it('attenuates content above the new Nyquist instead of aliasing it', () => {
		// 7.9 kHz sits just below Nyquist at 16 kHz; 15 kHz is far above it.
		const high = resample(sine(48_000, 15_000, 0.25), 48_000, 16_000);
		const speech = resample(sine(48_000, 1_000, 0.25), 48_000, 16_000);
		expect(rms(high)).toBeLessThan(rms(speech) * 0.5);
	});

	it('upsamples by linear interpolation', () => {
		const out = resample(new Float32Array([0, 1]), 8_000, 16_000);
		expect(Array.from(out)).toEqual([0, 0.5, 1, 1]);
	});

	it('handles empty input and rejects non-positive rates', () => {
		expect(resample(new Float32Array(0), 48_000, 16_000).length).toBe(0);
		expect(() => resample(new Float32Array(4), 0, 16_000)).toThrow(RangeError);
	});
});

describe('chunksToWav16k', () => {
	it('concatenates chunks, resamples and reports the right duration', () => {
		const chunks = Array.from({ length: 10 }, () => new Float32Array(4800)); // 1 s at 48 kHz
		const wav = chunksToWav16k(chunks, 48_000);
		expect(new DataView(wav).getUint32(24, true)).toBe(16_000);
		expect(wavDurationMs(wav)).toBe(1000);
	});

	it('caps the clip at 30 seconds', () => {
		const chunks = [new Float32Array(48_000 * 40)];
		const wav = chunksToWav16k(chunks, 48_000);
		expect(wavDurationMs(wav)).toBe(MAX_SECONDS * 1000);
		expect(wav.byteLength).toBeLessThan(5 * 1024 * 1024);
	});
});

describe('helpers', () => {
	it('concat joins chunks in order', () => {
		expect(Array.from(concat([new Float32Array([1, 2]), new Float32Array([3])]))).toEqual([1, 2, 3]);
	});

	it('rms of silence is 0 and of a full-scale square wave is 1', () => {
		expect(rms(new Float32Array(10))).toBe(0);
		expect(rms(new Float32Array([1, -1, 1, -1]))).toBe(1);
		expect(rms(new Float32Array(0))).toBe(0);
	});

	it('toneWav is a valid WAV of the requested length', () => {
		const wav = toneWav(500, 22_050);
		expect(wavDurationMs(wav)).toBe(500);
	});

	it('wavDurationMs returns 0 for a truncated buffer', () => {
		expect(wavDurationMs(new ArrayBuffer(10))).toBe(0);
	});
});
