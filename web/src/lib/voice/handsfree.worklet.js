// @ts-nocheck  (AudioWorklet global scope: AudioWorkletProcessor / registerProcessor / sampleRate)
// Hands-free voice activity detection on the audio thread. Vite bundles this module
// (with vad-core.js) into one file served from the app's own origin, so loading it with
// `audioWorklet.addModule()` passes the server's Content-Security-Policy (script-src
// 'self'); no blob: or data: URLs are involved.
import { VadSegmenter } from './vad-core.js';

class ArcHandsFree extends AudioWorkletProcessor {
	constructor() {
		super();
		this.vad = new VadSegmenter(sampleRate);
		this.paused = false;
		this.sum = 0;
		this.n = 0;
		this.port.onmessage = (e) => {
			if (e.data && e.data.type === 'pause') {
				this.paused = !!e.data.value;
				this.vad.reset();
			}
		};
	}
	process(inputs) {
		const ch = inputs[0] && inputs[0][0];
		if (!ch || this.paused) return true;
		for (let i = 0; i < ch.length; i++) this.sum += ch[i] * ch[i];
		this.n += ch.length;
		if (this.n >= sampleRate / 20) {
			this.port.postMessage({ type: 'level', rms: Math.sqrt(this.sum / this.n) });
			this.sum = 0;
			this.n = 0;
		}
		for (const e of this.vad.push(ch)) {
			if (e.type === 'segment') this.port.postMessage({ type: 'segment', samples: e.samples, rate: sampleRate }, [e.samples.buffer]);
			else this.port.postMessage({ type: e.type });
		}
		return true;
	}
}
registerProcessor('arcmt-handsfree', ArcHandsFree);
