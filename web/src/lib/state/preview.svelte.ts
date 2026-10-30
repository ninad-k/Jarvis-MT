import { api, type PreviewDto, type TradeResult } from '#lib/api.ts';
import { announcement, countdown, remainingMs, windowFor, type Countdown } from '#lib/countdown.ts';
import { ApiError, RETRYABLE_CONFIRM_CODES, describeError, toApiError } from '#lib/errors.ts';
import type { PendingState } from '#lib/voice/intent.ts';
import { chat } from './chat.svelte';
import { refreshTrading, status } from './hud.svelte';
import { toasts } from './toasts.svelte';

export type PreviewPhase = 'review' | 'executing' | 'results' | 'error';

export interface PreviewFailure {
	code: string;
	message: string;
	/** Nothing more can be done with this token: the modal offers Close only. */
	terminal: boolean;
}

const TICK_MS = 100;

/**
 * The one confirmation in flight (SPEC rules 2-4): the preview, its countdown, the batch second
 * stage and the execution result. Confirmation only ever happens through `confirm()`, which the
 * Confirm button and a spoken "confirm" both call; nothing here sends a write on its own.
 */
class PreviewStore {
	preview = $state<PreviewDto | null>(null);
	phase = $state<PreviewPhase>('review');
	results = $state<TradeResult[]>([]);
	failure = $state<PreviewFailure | null>(null);
	/** Server message for the second stage, e.g. "Confirm again to close 3 positions". */
	stageMessage = $state<string | null>(null);
	/** Inline notice that does not end the confirmation (rate limit). */
	notice = $state<string | null>(null);
	expiresAt = $state(0);
	totalMs = $state(15_000);
	/** Voice turn: the window starts when the reply finishes playing (`readbackDone`). */
	awaitingReadback = $state(false);
	/** The server told us the token is dead before our clock did. */
	serverExpired = $state(false);
	now = $state(Date.now());
	/** Text for the screen-reader live region. */
	liveText = $state('');

	private timer: ReturnType<typeof setInterval> | undefined;
	private prevSeconds: number | null = null;

	get isOpen(): boolean {
		return this.preview !== null;
	}

	get cd(): Countdown {
		return countdown(this.expiresAt, this.totalMs, this.now);
	}

	/** Confirmable right now: a live token, not expired, nothing in flight. */
	get expired(): boolean {
		return this.phase === 'review' && (this.serverExpired || this.cd.expired);
	}

	get canConfirm(): boolean {
		return this.preview !== null && this.phase === 'review' && !this.expired;
	}

	pendingState(): PendingState {
		return {
			open: this.preview !== null && this.phase === 'review',
			expired: this.expired,
			busy: this.phase === 'executing'
		};
	}

	/** Show a new preview. A preview still waiting for a decision is cancelled first. */
	open(p: PreviewDto): void {
		if (this.preview && this.phase === 'review') this.cancelQuietly(this.preview.token);
		this.preview = p;
		this.phase = 'review';
		this.results = [];
		this.failure = null;
		this.stageMessage = null;
		this.notice = null;
		this.serverExpired = false;
		this.awaitingReadback = p.timing === 'after_readback';
		this.setWindow(p.expires_at_ms);
		this.liveText = `Confirmation required: ${p.description}`;
		this.startTicker();
	}

	/** The reply has been read aloud (or could not be): start the 15 s window. */
	async readbackDone(): Promise<void> {
		const p = this.preview;
		if (!p || !this.awaitingReadback) return;
		this.awaitingReadback = false;
		try {
			const r = await api.readback(p.token);
			if (this.preview?.token === p.token) this.setWindow(r.expires_at_ms);
		} catch (e) {
			const err = toApiError(e);
			if (this.preview?.token !== p.token) return;
			if (err.code === 'token_expired' || err.code === 'token_used' || err.code === 'token_unknown')
				this.serverExpired = true;
		}
	}

	/** Confirm button, or a spoken "confirm". Returns after the request settles. */
	async confirm(): Promise<void> {
		const p = this.preview;
		if (!p || !this.canConfirm) return;
		this.phase = 'executing';
		this.notice = null;
		try {
			const res = await api.confirm(p.token);
			if (this.preview?.token !== p.token) return;
			if (res.status === 'needs_second_confirmation') {
				this.preview = { ...p, token: res.token, stage: 'second', expires_at_ms: res.expires_at_ms };
				this.stageMessage = res.message;
				this.awaitingReadback = false;
				this.setWindow(res.expires_at_ms);
				this.phase = 'review';
				this.liveText = res.message;
				return;
			}
			this.results = res.results;
			this.phase = 'results';
			this.stopTicker();
			refreshTrading();
			this.noteResults(p, res.results);
		} catch (e) {
			this.fail(p, toApiError(e));
		}
	}

	/** Cancel button, Esc, or a spoken "cancel". Tells the server so the token dies at once. */
	async cancel(): Promise<void> {
		const p = this.preview;
		if (!p) return;
		if (this.phase === 'executing') return;
		if (this.phase === 'review') {
			if (!this.expired) chat.note(`Cancelled: ${p.description}`, 'warn');
			this.cancelQuietly(p.token);
		}
		this.close();
	}

	/** Close the results or error view. */
	dismiss(): void {
		if (this.phase === 'executing') return;
		this.close();
	}

	/** WebSocket `preview_expired`. Also fires when the kill switch revokes tokens. */
	onExpiredEvent(token: string): void {
		const p = this.preview;
		if (!p || p.token !== token || this.phase !== 'review') return;
		this.serverExpired = true;
	}

	/** The kill switch engaged while a confirmation was open: the server revoked its tokens. */
	onKillSwitch(): void {
		if (this.preview && this.phase === 'review') {
			this.failure = {
				code: 'kill_switch',
				message: describeError(new ApiError('kill_switch', 'kill switch engaged', 423)),
				terminal: true
			};
			this.phase = 'error';
			this.stopTicker();
		}
	}

	// ---- internals ----

	private setWindow(expiresAt: number): void {
		this.expiresAt = expiresAt;
		this.totalMs = windowFor(remainingMs(expiresAt, Date.now()));
		this.now = Date.now();
		this.prevSeconds = null;
	}

	private fail(p: PreviewDto, err: ApiError): void {
		if (this.preview?.token !== p.token) return;
		// A timeout or dropped connection after the request left may still have executed.
		const unknown = err.code === 'outcome_unknown' || err.code === 'timeout' || err.isOffline;
		const message = unknown
			? describeError(new ApiError('outcome_unknown', err.message, err.status))
			: describeError(err);
		if (RETRYABLE_CONFIRM_CODES.has(err.code)) {
			// The token is still good: stay on the review screen and say so.
			this.phase = 'review';
			this.notice = message;
			return;
		}
		this.failure = { code: unknown ? 'outcome_unknown' : err.code, message, terminal: true };
		this.phase = 'error';
		this.stopTicker();
		if (unknown) {
			// Look at the book rather than assume.
			refreshTrading();
			chat.note('Outcome unknown — check positions.', 'bad');
		} else if (err.code === 'kill_switch') {
			status.refresh();
		} else if (err.code === 'trade_failed') {
			refreshTrading();
		}
	}

	private noteResults(p: PreviewDto, results: TradeResult[]): void {
		const ok = results.filter((r) => r.ok).length;
		const bad = results.length - ok;
		if (bad === 0) chat.note(`Executed: ${p.description}`, 'ok');
		else if (ok === 0) chat.note(`Failed: ${p.description}`, 'bad');
		else chat.note(`Partly executed: ${ok} of ${results.length} succeeded, ${bad} failed.`, 'warn');
	}

	private cancelQuietly(token: string): void {
		api.cancel(token).catch(() => {
			/* already expired or used: either way it can no longer execute */
		});
	}

	private close(): void {
		this.stopTicker();
		this.preview = null;
		this.phase = 'review';
		this.results = [];
		this.failure = null;
		this.stageMessage = null;
		this.notice = null;
		this.awaitingReadback = false;
		this.serverExpired = false;
		this.liveText = '';
	}

	private startTicker(): void {
		this.stopTicker();
		this.timer = setInterval(() => this.tick(), TICK_MS);
	}

	private stopTicker(): void {
		clearInterval(this.timer);
		this.timer = undefined;
	}

	private tick(): void {
		this.now = Date.now();
		if (this.phase !== 'review') return;
		if (this.awaitingReadback) return;
		const msg = this.serverExpired ? null : announcement(this.prevSeconds, this.cd);
		this.prevSeconds = this.cd.seconds;
		if (msg) this.liveText = msg;
		if (this.serverExpired && this.prevSeconds !== 0) {
			this.liveText = 'Expired. Confirm is disabled.';
			this.prevSeconds = 0;
		}
	}
}

export const preview = new PreviewStore();

/** Used by components that surface errors from starting a close (before any preview exists). */
export function reportActionError(e: unknown): void {
	toasts.error(describeError(toApiError(e)));
}
