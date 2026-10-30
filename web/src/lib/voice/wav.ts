/** Audio helpers for the voice upload: concatenate, resample to 16 kHz mono, encode 16-bit WAV. */

export const TARGET_RATE = 16_000;
/** The server rejects clips longer than 30 s. */
export const MAX_SECONDS = 30;

export function concat(chunks: readonly Float32Array[]): Float32Array {
	let n = 0;
	for (const c of chunks) n += c.length;
	const out = new Float32Array(n);
	let o = 0;
	for (const c of chunks) {
		out.set(c, o);
		o += c.length;
	}
	return out;
}

/** Root-mean-square level of a block of samples, 0..1. */
export function rms(samples: Float32Array): number {
	if (samples.length === 0) return 0;
	let sum = 0;
	for (let i = 0; i < samples.length; i++) sum += samples[i] * samples[i];
	return Math.sqrt(sum / samples.length);
}

/**
 * Resample mono audio. Downsampling averages every input sample that falls inside an output
 * sample's span (a box filter), which keeps speech clean without aliasing hiss.
 * Upsampling interpolates linearly.
 */
export function resample(input: Float32Array, fromRate: number, toRate: number): Float32Array {
	if (fromRate <= 0 || toRate <= 0) throw new RangeError('sample rates must be positive');
	if (fromRate === toRate || input.length === 0) return input.slice();
	const outLen = Math.max(1, Math.round((input.length * toRate) / fromRate));
	const out = new Float32Array(outLen);
	const ratio = fromRate / toRate;

	if (ratio > 1) {
		for (let i = 0; i < outLen; i++) {
			const start = i * ratio;
			const end = Math.min(input.length, start + ratio);
			let sum = 0;
			let weight = 0;
			for (let j = Math.floor(start); j < end; j++) {
				const w = Math.min(j + 1, end) - Math.max(j, start);
				if (w <= 0) continue;
				sum += input[j] * w;
				weight += w;
			}
			out[i] = weight > 0 ? sum / weight : 0;
		}
	} else {
		for (let i = 0; i < outLen; i++) {
			const pos = i * ratio;
			const j = Math.floor(pos);
			const frac = pos - j;
			const a = input[Math.min(j, input.length - 1)];
			const b = input[Math.min(j + 1, input.length - 1)];
			out[i] = a + (b - a) * frac;
		}
	}
	return out;
}

/** Encode mono float samples (-1..1) as a 16-bit PCM WAV file. */
export function encodeWav(samples: Float32Array, sampleRate: number): ArrayBuffer {
	const dataBytes = samples.length * 2;
	const buf = new ArrayBuffer(44 + dataBytes);
	const v = new DataView(buf);
	const str = (off: number, s: string) => {
		for (let i = 0; i < s.length; i++) v.setUint8(off + i, s.charCodeAt(i));
	};
	str(0, 'RIFF');
	v.setUint32(4, 36 + dataBytes, true);
	str(8, 'WAVE');
	str(12, 'fmt ');
	v.setUint32(16, 16, true); // fmt chunk size
	v.setUint16(20, 1, true); // PCM
	v.setUint16(22, 1, true); // mono
	v.setUint32(24, sampleRate, true);
	v.setUint32(28, sampleRate * 2, true); // byte rate
	v.setUint16(32, 2, true); // block align
	v.setUint16(34, 16, true); // bits per sample
	str(36, 'data');
	v.setUint32(40, dataBytes, true);
	for (let i = 0; i < samples.length; i++) {
		const s = Math.max(-1, Math.min(1, samples[i]));
		v.setInt16(44 + i * 2, s < 0 ? Math.round(s * 0x8000) : Math.round(s * 0x7fff), true);
	}
	return buf;
}

/** Captured chunks at the device rate -> a 16 kHz mono WAV, trimmed to the 30 s limit. */
export function chunksToWav16k(chunks: readonly Float32Array[], sourceRate: number): ArrayBuffer {
	const all = concat(chunks);
	const max = Math.round(sourceRate * MAX_SECONDS);
	const limited = all.length > max ? all.subarray(0, max) : all;
	return encodeWav(resample(limited, sourceRate, TARGET_RATE), TARGET_RATE);
}

/** Duration of an encoded WAV in ms (reads the header; 0 for anything malformed). */
export function wavDurationMs(buf: ArrayBuffer): number {
	if (buf.byteLength < 44) return 0;
	const v = new DataView(buf);
	const rate = v.getUint32(24, true);
	const bytes = v.getUint32(40, true);
	return rate > 0 ? Math.round((bytes / 2 / rate) * 1000) : 0;
}

/** A short two-tone beep as WAV, used by the mock TTS endpoint. */
export function toneWav(ms = 900, sampleRate = 22_050): ArrayBuffer {
	const n = Math.round((sampleRate * ms) / 1000);
	const out = new Float32Array(n);
	for (let i = 0; i < n; i++) {
		const t = i / sampleRate;
		const env = Math.max(0, Math.min(1, t / 0.03, (n / sampleRate - t) / 0.12));
		// Amplitude wobble so the orb visibly follows the audio.
		const wob = 0.6 + 0.4 * Math.sin(2 * Math.PI * 4 * t);
		const tone = Math.sin(2 * Math.PI * 392 * t) + 0.6 * Math.sin(2 * Math.PI * 587 * t);
		out[i] = (0.25 * env * wob * tone) / 1.6;
	}
	return encodeWav(out, sampleRate);
}
