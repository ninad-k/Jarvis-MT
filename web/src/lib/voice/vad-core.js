// @ts-nocheck  (plain JS so it can run inside an AudioWorklet; the public types are in vad-core.d.ts)
/**
 * Energy-based voice activity detector for hands-free mode.
 *
 * Self-contained on purpose (no imports, plain JavaScript): `listener.ts` loads this very file as the
 * source of an AudioWorklet module, so the detector runs on the audio thread, and the unit tests
 * import the same code. Types are in vad-core.d.ts.
 *
 * It cuts a stream of mono samples into speech segments:
 *   - a segment opens after `startMs` of consecutive frames louder than the "on" threshold,
 *   - it keeps `preRollMs` of audio from before that point so the first syllable is not clipped,
 *   - it closes after `silenceMs` of frames quieter than the "off" threshold (hysteresis),
 *   - it is dropped when the speech inside it is shorter than `minSpeechMs`,
 *   - it is cut at `maxSegmentMs` whatever happens.
 * The thresholds follow a slowly adapting noise floor, so a fan or a hissy microphone does not
 * count as speech, with absolute minimums so a silent room does not trigger on dither. Steady noise
 * that is louder than the minimums (a fan next to the microphone) opens a segment too; it never
 * pauses, so the 8 s cut catches it. Speech has a lively envelope, steady noise does not: a cut
 * segment whose level barely varies is thrown away and its level becomes the new noise floor.
 */

export const VAD_DEFAULTS = {
	frameMs: 20,
	startMs: 60,
	silenceMs: 700,
	minSpeechMs: 400,
	maxSegmentMs: 8000,
	preRollMs: 300,
	tailMs: 200,
	minOn: 0.02,
	minOff: 0.012,
	onFactor: 3.5,
	offFactor: 2.2,
	maxFloor: 0.05,
	/** A cut segment whose frame levels vary less than this (std / mean) is noise, not speech. */
	minVariation: 0.25
};

export class VadSegmenter {
	/**
	 * @param {number} sampleRate
	 * @param {Partial<typeof VAD_DEFAULTS>} [options]
	 */
	constructor(sampleRate, options = {}) {
		const o = { ...VAD_DEFAULTS, ...options };
		this.o = o;
		this.sampleRate = sampleRate;
		this.frameLen = Math.max(1, Math.round((sampleRate * o.frameMs) / 1000));
		this.startFrames = Math.max(1, Math.round(o.startMs / o.frameMs));
		this.silenceFrames = Math.max(1, Math.round(o.silenceMs / o.frameMs));
		this.minSpeechFrames = Math.max(1, Math.round(o.minSpeechMs / o.frameMs));
		this.maxFrames = Math.max(1, Math.round(o.maxSegmentMs / o.frameMs));
		this.preRollFrames = Math.max(0, Math.round(o.preRollMs / o.frameMs));
		this.tailFrames = Math.max(0, Math.round(o.tailMs / o.frameMs));
		this.reset();
	}

	/** Forget everything: buffered audio, the open segment and the noise estimate. */
	reset() {
		this.frame = new Float32Array(this.frameLen);
		this.fill = 0;
		this.floor = 0.004;
		this.calibrating = 25;
		/** Idle: the last frames, kept as pre-roll. */
		this.ring = [];
		/** Idle: loud frames seen in a row (not yet a segment). */
		this.candidate = [];
		this.open = false;
		/** Open: every frame of the segment so far, pre-roll first. */
		this.seg = [];
		this.segPre = 0;
		this.lastSpeech = 0;
		this.quiet = 0;
		this.lastRms = 0;
		this.sum = 0;
		this.sumSq = 0;
	}

	/** Current speech-on threshold (RMS 0..1). */
	get onThreshold() {
		return Math.max(this.o.minOn, this.floor * this.o.onFactor);
	}

	get offThreshold() {
		return Math.max(this.o.minOff, this.floor * this.o.offFactor);
	}

	get noiseFloor() {
		return this.floor;
	}

	/** True while a segment is open (speech is being heard). */
	get speaking() {
		return this.open;
	}

	/**
	 * Feed samples (any length). Returns the events they completed:
	 *   {type:'start'}                      speech began
	 *   {type:'segment', samples, durationMs, reason:'silence'|'max'}
	 *   {type:'discard', reason:'short'}    a burst too short to be speech
	 */
	push(samples) {
		const events = [];
		let i = 0;
		while (i < samples.length) {
			const n = Math.min(this.frameLen - this.fill, samples.length - i);
			this.frame.set(samples.subarray(i, i + n), this.fill);
			this.fill += n;
			i += n;
			if (this.fill === this.frameLen) {
				const done = this.frame;
				this.frame = new Float32Array(this.frameLen);
				this.fill = 0;
				this.step(done, events);
			}
		}
		return events;
	}

	/** @private */
	step(frame, events) {
		let sum = 0;
		for (let i = 0; i < frame.length; i++) sum += frame[i] * frame[i];
		const level = Math.sqrt(sum / frame.length);
		this.lastRms = level;
		if (this.open) this.stepOpen(frame, level, events);
		else this.stepIdle(frame, level, events);
	}

	/** @private */
	stepIdle(frame, level, events) {
		if (level >= this.onThreshold) {
			this.candidate.push(frame);
			if (this.candidate.length >= this.startFrames) {
				this.open = true;
				this.segPre = this.ring.length;
				this.seg = [...this.ring, ...this.candidate];
				this.lastSpeech = this.seg.length;
				this.quiet = 0;
				this.sum = 0;
				this.sumSq = 0;
				this.ring = [];
				this.candidate = [];
				events.push({ type: 'start' });
			}
			return;
		}
		// Quiet frame: any pending loud frames were a click, not speech.
		for (const f of this.candidate) this.keep(f);
		this.candidate = [];
		this.keep(frame);
		// Follow the room: fast for the first half second, slow afterwards, never above maxFloor.
		const a = this.calibrating > 0 ? 0.2 : 0.02;
		if (this.calibrating > 0) this.calibrating--;
		this.floor = Math.min(this.o.maxFloor, this.floor + (level - this.floor) * a);
	}

	/** @private */
	keep(frame) {
		this.ring.push(frame);
		if (this.ring.length > this.preRollFrames) this.ring.shift();
	}

	/** @private */
	stepOpen(frame, level, events) {
		this.seg.push(frame);
		this.sum += level;
		this.sumSq += level * level;
		if (level >= this.offThreshold) {
			this.lastSpeech = this.seg.length;
			this.quiet = 0;
		} else {
			this.quiet++;
		}
		if (this.quiet >= this.silenceFrames) this.close('silence', events);
		else if (this.seg.length >= this.maxFrames) this.close('max', events);
	}

	/** @private */
	close(reason, events) {
		const speechFrames = this.lastSpeech - this.segPre;
		const keepTo = reason === 'max' ? this.seg.length : Math.min(this.seg.length, this.lastSpeech + this.tailFrames);
		const n = Math.max(1, this.seg.length - this.segPre);
		const mean = this.sum / n;
		const std = Math.sqrt(Math.max(0, this.sumSq / n - mean * mean));
		if (reason === 'silence' && speechFrames < this.minSpeechFrames) {
			events.push({ type: 'discard', reason: 'short' });
		} else if (reason === 'max' && mean > 0 && std / mean < this.o.minVariation) {
			// Eight seconds of the same level: a fan, not a person. Learn it.
			this.floor = Math.min(this.o.maxFloor, Math.max(this.floor, mean));
			events.push({ type: 'discard', reason: 'noise' });
		} else {
			const out = new Float32Array(keepTo * this.frameLen);
			for (let k = 0; k < keepTo; k++) out.set(this.seg[k], k * this.frameLen);
			events.push({
				type: 'segment',
				samples: out,
				durationMs: (keepTo * this.frameLen * 1000) / this.sampleRate,
				reason
			});
		}
		this.open = false;
		this.seg = [];
		this.segPre = 0;
		this.lastSpeech = 0;
		this.quiet = 0;
		this.ring = [];
		this.candidate = [];
	}
}
