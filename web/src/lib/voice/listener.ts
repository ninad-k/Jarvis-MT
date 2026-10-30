/**
 * Hands-free microphone: an always-open input whose speech detection runs on the audio thread
 * (an AudioWorklet that embeds the VadSegmenter from ./vad-core.js) and hands finished speech
 * segments to the page. Nothing leaves the browser until a segment is complete, and segments are
 * only ever sent to the local server by the hands-free store.
 * Browsers without AudioWorklet fall back to a ScriptProcessor and run the same segmenter on the
 * main thread. Needs a browser, so it is covered by the e2e smoke test, not unit tests.
 */
import { audioCtor } from './audio';
import { VadSegmenter } from './vad-core.js';
// Same-origin module file bundled by Vite (a blob: URL would be blocked by the CSP).
import handsFreeWorkletUrl from './handsfree.worklet.js?worker&url';


export interface ListenerOptions {
	/** Mic level 0..1, about 20 times a second. */
	onLevel: (rms: number) => void;
	/** Speech began (a segment is being collected). */
	onSpeechStart: () => void;
	/** A burst that was too short or too steady to be speech. */
	onDiscard?: () => void;
	/** One finished utterance, mono samples at `rate`. */
	onSegment: (samples: Float32Array, rate: number) => void;
}

export class HandsFreeListener {
	private closed = false;
	private paused = false;

	private constructor(
		private readonly ctx: AudioContext,
		private readonly stream: MediaStream,
		private readonly teardown: () => void,
		private readonly setPaused: (on: boolean) => void
	) {}

	/** Ask for the microphone and start listening. Rejects with a DOMException-like error. */
	static async start(opts: ListenerOptions): Promise<HandsFreeListener> {
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

			let teardown: () => void;
			let setPaused: (on: boolean) => void;

			if (ctx.audioWorklet) {
				await ctx.audioWorklet.addModule(handsFreeWorkletUrl);
				const node = new AudioWorkletNode(ctx, 'arcmt-handsfree', { numberOfOutputs: 1, channelCount: 1 });
				node.port.onmessage = (ev: MessageEvent<{ type: string; rms?: number; samples?: Float32Array; rate?: number }>) => {
					const m = ev.data;
					if (m.type === 'level') opts.onLevel(m.rms ?? 0);
					else if (m.type === 'start') opts.onSpeechStart();
					else if (m.type === 'discard') opts.onDiscard?.();
					else if (m.type === 'segment' && m.samples) opts.onSegment(m.samples, m.rate ?? ctx.sampleRate);
				};
				source.connect(node).connect(sink);
				setPaused = (on) => node.port.postMessage({ type: 'pause', value: on });
				teardown = () => {
					node.port.onmessage = null;
					node.disconnect();
				};
			} else {
				// Older Safari: ScriptProcessor is deprecated but still works.
				const vad = new VadSegmenter(ctx.sampleRate);
				let paused = false;
				const node = ctx.createScriptProcessor(4096, 1, 1);
				node.onaudioprocess = (ev) => {
					if (paused) return;
					const data = ev.inputBuffer.getChannelData(0).slice(0);
					let sum = 0;
					for (let i = 0; i < data.length; i++) sum += data[i] * data[i];
					opts.onLevel(Math.sqrt(sum / data.length));
					for (const e of vad.push(data)) {
						if (e.type === 'segment') opts.onSegment(e.samples, ctx.sampleRate);
						else if (e.type === 'start') opts.onSpeechStart();
						else opts.onDiscard?.();
					}
				};
				source.connect(node);
				node.connect(sink);
				setPaused = (on) => {
					paused = on;
					vad.reset();
				};
				teardown = () => {
					node.onaudioprocess = null;
					node.disconnect();
				};
			}
			return new HandsFreeListener(
				ctx,
				stream,
				() => {
					teardown();
					source.disconnect();
					sink.disconnect();
				},
				setPaused
			);
		} catch (e) {
			for (const t of stream.getTracks()) t.stop();
			void ctx.close().catch(() => {});
			throw e;
		}
	}

	/** Stop hearing (and drop any half-heard utterance) without giving up the microphone. */
	pause(on: boolean): void {
		if (this.closed || on === this.paused) return;
		this.paused = on;
		this.setPaused(on);
	}

	/** Release the microphone. */
	stop(): void {
		if (this.closed) return;
		this.closed = true;
		this.teardown();
		for (const t of this.stream.getTracks()) t.stop();
		void this.ctx.close().catch(() => {});
	}
}
