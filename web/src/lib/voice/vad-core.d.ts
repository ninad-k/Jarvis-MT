export interface VadOptions {
	frameMs: number;
	startMs: number;
	silenceMs: number;
	minSpeechMs: number;
	maxSegmentMs: number;
	preRollMs: number;
	tailMs: number;
	minOn: number;
	minOff: number;
	onFactor: number;
	offFactor: number;
	maxFloor: number;
	minVariation: number;
}

export type VadEvent =
	| { type: 'start' }
	| { type: 'segment'; samples: Float32Array; durationMs: number; reason: 'silence' | 'max' }
	| { type: 'discard'; reason: 'short' | 'noise' };

export const VAD_DEFAULTS: Readonly<VadOptions>;

export class VadSegmenter {
	constructor(sampleRate: number, options?: Partial<VadOptions>);
	readonly sampleRate: number;
	readonly lastRms: number;
	readonly noiseFloor: number;
	readonly onThreshold: number;
	readonly offThreshold: number;
	/** True while a segment is open. */
	readonly speaking: boolean;
	reset(): void;
	push(samples: Float32Array): VadEvent[];
}
