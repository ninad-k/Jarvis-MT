// @ts-nocheck  (AudioWorklet global scope: AudioWorkletProcessor / registerProcessor)
// Push-to-talk capture tap. Loaded with `audioWorklet.addModule()` from a URL that Vite
// emits for this file, so it is served from the app's own origin and passes the
// server's Content-Security-Policy (script-src 'self'). It only forwards mono frames to
// the main thread, where they are buffered and encoded.
class Tap extends AudioWorkletProcessor {
	process(inputs) {
		const ch = inputs[0] && inputs[0][0];
		if (ch && ch.length) this.port.postMessage(ch.slice(0));
		return true;
	}
}
registerProcessor('jarvis-tap', Tap);
