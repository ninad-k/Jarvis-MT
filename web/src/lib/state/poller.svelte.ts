import { ApiError } from '#lib/api.ts';

export interface PollerOptions<T> {
	/** Normal poll interval in ms. */
	interval: number;
	/** Upper bound for the exponential back-off after failures. */
	maxInterval: number;
	/** Called with every successful payload before it replaces `data`. */
	onData?: (next: T, prev: T | null) => void;
}

export type ViewKind = 'loading' | 'ok' | 'offline' | 'error';

/**
 * Polls an async source with exponential back-off while it fails, pauses while the tab is hidden
 * and keeps the last good payload so panels can show it as stale instead of blanking.
 */
export class Poller<T> {
	data = $state<T | null>(null);
	error = $state<ApiError | null>(null);
	/** True once the first attempt (success or failure) has finished. */
	settled = $state(false);
	lastOk = $state<number | null>(null);
	failures = $state(0);

	private timer: ReturnType<typeof setTimeout> | undefined;
	private ctrl: AbortController | undefined;
	private running = false;
	private generation = 0;
	private readonly fn: (signal: AbortSignal) => Promise<T>;
	private readonly opts: PollerOptions<T>;

	constructor(fn: (signal: AbortSignal) => Promise<T>, opts: PollerOptions<T>) {
		this.fn = fn;
		this.opts = opts;
	}

	/** What a panel should render right now. */
	get view(): ViewKind {
		if (!this.settled) return 'loading';
		if (this.error) return this.error.isOffline ? 'offline' : 'error';
		return 'ok';
	}

	/** Showing the last good payload while the latest attempt failed. */
	get stale(): boolean {
		return this.data !== null && this.error !== null;
	}

	start(): void {
		if (this.running) return;
		this.running = true;
		document.addEventListener('visibilitychange', this.onVisibility);
		void this.poll();
	}

	stop(): void {
		this.running = false;
		this.generation++;
		clearTimeout(this.timer);
		this.ctrl?.abort();
		document.removeEventListener('visibilitychange', this.onVisibility);
	}

	/** Forget everything (sign-out): the next poll starts from a blank state. */
	reset(): void {
		this.generation++;
		this.ctrl?.abort();
		this.data = null;
		this.error = null;
		this.settled = false;
		this.lastOk = null;
		this.failures = 0;
		if (this.running) void this.poll();
	}

	/** Poll now and restart the schedule. */
	refresh(): void {
		if (this.running) void this.poll();
	}

	private onVisibility = (): void => {
		if (!this.running) return;
		if (!document.hidden) void this.poll();
		else {
			clearTimeout(this.timer);
			this.ctrl?.abort();
			this.generation++;
		}
	};

	private async poll(): Promise<void> {
		clearTimeout(this.timer);
		this.ctrl?.abort();
		const gen = ++this.generation;
		const ctrl = new AbortController();
		this.ctrl = ctrl;
		try {
			const next = await this.fn(ctrl.signal);
			if (gen !== this.generation) return;
			this.opts.onData?.(next, this.data);
			this.data = next;
			this.error = null;
			this.failures = 0;
			this.lastOk = Date.now();
		} catch (e) {
			if (gen !== this.generation) return;
			this.error =
				e instanceof ApiError
					? e
					: new ApiError('unknown', e instanceof Error ? e.message : 'Unknown error');
			this.failures++;
		}
		this.settled = true;
		if (this.running && !document.hidden) {
			const backoff = Math.min(this.opts.interval * 2 ** this.failures, this.opts.maxInterval);
			const jitter = this.failures > 0 ? 1 + Math.random() * 0.2 : 1;
			this.timer = setTimeout(() => void this.poll(), backoff * jitter);
		}
	}
}

/** The read-only slice of a Poller that status panels need (avoids generic variance issues). */
export interface PollState {
	readonly data: unknown;
	readonly error: ApiError | null;
	readonly view: ViewKind;
	readonly failures: number;
}
