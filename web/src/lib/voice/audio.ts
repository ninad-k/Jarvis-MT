/**
 * Browser audio plumbing: microphone capture (AudioWorklet, ScriptProcessor fallback) and
 * playback with an AnalyserNode so the orb can follow the real signal.
 * Everything here needs a browser; the maths it feeds lives in ./wav.ts and is unit tested.
 */
import { MAX_SECONDS, chunksToWav16k, rms } from './wav';

// ---- Shared context -------------------------------------------------------

type AudioCtor = typeof AudioContext;
let shared: AudioContext | null = null;

export function audioCtor(): AudioCtor | null {
	if (typeof window === 'undefined') return null;
	return window.AudioContext ?? (window as unknown as { webkitAudioContext?: AudioCtor }).webkitAudioContext ?? null;
}

/** Create/resume the playback context. Call from a user gesture (click, key) so Safari allows it. */
export function primeAudio(): AudioContext | null {
	const Ctor = audioCtor();
	if (!Ctor) return null;
	shared ??= new Ctor();
	if (shared.state === 'suspended') void shared.resume().catch(() => {});
	return shared;
}

// ---- Microphone errors ----------------------------------------------------

/** Plain-language text for a getUserMedia / audio failure. */
export function micErrorMessage(e: unknown): string {
	const name = e instanceof Error || (typeof e === 'object' && e !== null) ? (e as { name?: string }).name : '';
	switch (name) {
		case 'NotAllowedError':
		case 'SecurityError':
		case 'PermissionDeniedError':
			return 'Microphone access was denied. Allow the microphone for this site in your browser settings, then try again.';
		case 'NotFoundError':
		case 'DevicesNotFoundError':
		case 'OverconstrainedError':
			return 'No microphone found. Connect one and try again.';
		case 'NotReadableError':
		case 'TrackStartError':
			return 'The microphone is in use by another app.';
		case 'InsecureContext':
			return 'Voice input needs a secure connection (HTTPS or localhost).';
		case 'Unsupported':
			return 'This browser cannot record audio.';
		default:
			return 'Could not start the microphone.';
	}
}

// ---- Capture --------------------------------------------------------------

// The capture tap runs in an AudioWorklet loaded from a same-origin module file
// (see tap.worklet.js); a blob: URL would be blocked by the server's CSP.
import tapWorkletUrl from './tap.worklet.js?worker&url';

export interface RecordingResult {
	wav: ArrayBuffer;
	durationMs: number;
}

export interface RecorderOptions {
	/** RMS level 0..1, about 20 times a second. */
	onLevel: (rms: number) => void;
	/** The 30 s cap was reached; the caller should `stop()`. */
	onLimit: () => void;
}

export class Recorder {
	private chunks: Float32Array[] = [];
	private samples = 0;
	private limited = false;
	private lastLevel = 0;
	private closed = false;

	private constructor(
		private readonly ctx: AudioContext,
		private readonly stream: MediaStream,
		private readonly cleanup: () => void,
		private readonly opts: RecorderOptions
	) {}

	/** Ask for the microphone and start capturing. Rejects with a DOMException-like error. */
	static async start(opts: RecorderOptions): Promise<Recorder> {
		const Ctor = audioCtor();
		if (!Ctor) throw Object.assign(new Error('no AudioContext'), { name: 'Unsupported' });
		if (!navigator.mediaDevices?.getUserMedia)
			throw Object.assign(new Error('insecure context'), { name: 'InsecureContext' });

		const stream = await navigator.mediaDevices.getUserMedia({
			audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true, autoGainControl: true }
		});
		const ctx = new Ctor();
		try {
			if (ctx.state === 'suspended') await ctx.resume();
			const source = ctx.createMediaStreamSource(stream);
			const sink = ctx.createGain(); // silent: keeps the graph pulling without echoing the mic
			sink.gain.value = 0;
			sink.connect(ctx.destination);

			let rec!: Recorder;
			const push = (data: Float32Array) => rec.push(data);
			let cleanup: () => void;

			if (ctx.audioWorklet) {
				await ctx.audioWorklet.addModule(tapWorkletUrl);
				const node = new AudioWorkletNode(ctx, 'jarvis-tap', { numberOfOutputs: 1, channelCount: 1 });
				node.port.onmessage = (ev: MessageEvent<Float32Array>) => push(ev.data);
				source.connect(node).connect(sink);
				cleanup = () => {
					node.port.onmessage = null;
					node.disconnect();
				};
			} else {
				// Older Safari: ScriptProcessor is deprecated but still works.
				const node = ctx.createScriptProcessor(4096, 1, 1);
				node.onaudioprocess = (ev) => push(ev.inputBuffer.getChannelData(0).slice(0));
				source.connect(node);
				node.connect(sink);
				cleanup = () => {
					node.onaudioprocess = null;
					node.disconnect();
				};
			}
			rec = new Recorder(ctx, stream, () => {
				cleanup();
				source.disconnect();
				sink.disconnect();
			}, opts);
			return rec;
		} catch (e) {
			for (const t of stream.getTracks()) t.stop();
			void ctx.close();
			throw e;
		}
	}

	private push(data: Float32Array): void {
		if (this.closed) return;
		this.chunks.push(data);
		this.samples += data.length;
		const now = performance.now();
		if (now - this.lastLevel >= 50) {
			this.lastLevel = now;
			this.opts.onLevel(rms(data));
		}
		if (!this.limited && this.samples >= this.ctx.sampleRate * MAX_SECONDS) {
			this.limited = true;
			this.opts.onLimit();
		}
	}

	get elapsedMs(): number {
		return (this.samples / this.ctx.sampleRate) * 1000;
	}

	/** Stop capturing and return the clip as 16 kHz mono WAV. */
	async stop(): Promise<RecordingResult> {
		const rate = this.ctx.sampleRate;
		this.release();
		const wav = chunksToWav16k(this.chunks, rate);
		return { wav, durationMs: Math.min(MAX_SECONDS * 1000, (this.samples / rate) * 1000) };
	}

	/** Throw the clip away. */
	cancel(): void {
		this.release();
		this.chunks = [];
	}

	private release(): void {
		if (this.closed) return;
		this.closed = true;
		this.cleanup();
		for (const t of this.stream.getTracks()) t.stop();
		void this.ctx.close().catch(() => {});
	}
}

// ---- Playback -------------------------------------------------------------

export interface Playback {
	/** Resolves when the audio has finished or `stop()` was called. */
	done: Promise<void>;
	stop(): void;
}

/** Decode and play a WAV blob, reporting the live output level (RMS 0..1) every frame. */
export async function playBlob(blob: Blob, onLevel: (rms: number) => void): Promise<Playback> {
	const ctx = primeAudio();
	if (!ctx) throw Object.assign(new Error('no AudioContext'), { name: 'Unsupported' });
	if (ctx.state === 'suspended') await ctx.resume();
	const buffer = await ctx.decodeAudioData(await blob.arrayBuffer());

	const src = ctx.createBufferSource();
	src.buffer = buffer;
	const analyser = ctx.createAnalyser();
	analyser.fftSize = 1024;
	src.connect(analyser);
	analyser.connect(ctx.destination);

	const data = new Float32Array(analyser.fftSize);
	let raf = 0;
	const loop = () => {
		analyser.getFloatTimeDomainData(data);
		onLevel(rms(data));
		raf = requestAnimationFrame(loop);
	};

	let finish!: () => void;
	const done = new Promise<void>((resolve) => (finish = resolve));
	let ended = false;
	const end = () => {
		if (ended) return;
		ended = true;
		cancelAnimationFrame(raf);
		onLevel(0);
		src.disconnect();
		analyser.disconnect();
		finish();
	};
	src.onended = end;
	src.start();
	raf = requestAnimationFrame(loop);

	return {
		done,
		stop: () => {
			try {
				src.stop();
			} catch {
				/* already stopped */
			}
			end();
		}
	};
}
