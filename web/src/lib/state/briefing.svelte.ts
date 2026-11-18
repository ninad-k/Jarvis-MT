import { api, type BriefingDto } from '#lib/api.ts';
import { briefingAnnouncement } from '#lib/briefing.ts';
import { describeError, toApiError, type ApiError } from '#lib/errors.ts';

export type BriefingPhase = 'idle' | 'loading' | 'ready' | 'error';

/** A server without the endpoint answers 404 with no error body, which reads as `unknown`. */
const isMissing = (e: ApiError) => e.code === 'unknown';

function briefingError(e: ApiError): string {
	if (isMissing(e)) return 'This server has no briefing yet. Update the server to use it.';
	if (e.code === 'bridge_unavailable' || e.code === 'bridge_error')
		return `MT5 is not answering, so there is no account or prices to brief on. ${e.message}`;
	return describeError(e);
}

/**
 * The "Brief me" card. It is built on demand and never refreshes by itself: the server makes a
 * computed analysis for every watchlist symbol, which takes a few seconds.
 */
class BriefingStore {
	data = $state<BriefingDto | null>(null);
	phase = $state<BriefingPhase>('idle');
	error = $state<string | null>(null);
	/** For "in 40 min" on the events; set when the briefing arrives. */
	now = $state(Date.now());

	private ctrl: AbortController | null = null;

	get loading(): boolean {
		return this.phase === 'loading';
	}

	get announcement(): string {
		if (this.phase === 'loading') return 'Preparing your briefing…';
		if (this.phase === 'error') return `The briefing failed. ${this.error ?? ''}`;
		return this.data && this.phase === 'ready' ? briefingAnnouncement(this.data) : '';
	}

	async load(): Promise<void> {
		this.ctrl?.abort();
		const ctrl = new AbortController();
		this.ctrl = ctrl;
		this.phase = 'loading';
		this.error = null;
		try {
			const next = await api.briefing(ctrl.signal);
			if (ctrl.signal.aborted) return;
			this.data = next;
			this.now = Date.now();
			this.phase = 'ready';
		} catch (e) {
			const err = toApiError(e);
			if (err.code === 'aborted' || ctrl.signal.aborted) return;
			this.phase = 'error';
			this.error = briefingError(err);
		}
	}

	reset(): void {
		this.ctrl?.abort();
		this.data = null;
		this.phase = 'idle';
		this.error = null;
	}
}

export const briefing = new BriefingStore();
