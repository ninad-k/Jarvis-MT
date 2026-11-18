import { api, type AlertCreateBody, type AlertTriggeredEvent, type PriceAlert } from '#lib/api.ts';
import { alertTitle, toastText, spokenLine } from '#lib/alerts.ts';
import { describeError, toApiError } from '#lib/errors.ts';
import { armAudioUnlock, runningAudio } from '#lib/voice/audio.ts';
import { playChime } from '#lib/voice/chime.ts';
import { digitsForAny } from './hud.svelte';
import { orb } from './orb.svelte';
import { speech } from './speech.svelte';
import { toasts } from './toasts.svelte';

export type AlertsPhase = 'idle' | 'loading' | 'ready' | 'error' | 'unsupported';

/** Where the list is re-read when no push event arrives (an alert made from chat, a missed socket frame). */
const REFRESH_MS = 30_000;
/** Remember this many triggered ids so a repeated push frame cannot ring twice. */
const SEEN_MAX = 100;

/** What the alert dialog is about: one watchlist row. The dialog reads its live price itself. */
export interface AlertTarget {
	/** The canonical name (`XAUUSD`). */
	symbol: string;
	broker_symbol: string;
}

/**
 * Price alerts: the active and recent lists, the create dialog and what happens when the server
 * says one fired (toast, a short sound, a spoken line when voice output is on). The server owns
 * the alerts and the watcher; this only mirrors it.
 */
class AlertsStore {
	active = $state<PriceAlert[]>([]);
	recent = $state<PriceAlert[]>([]);
	phase = $state<AlertsPhase>('idle');
	error = $state<string | null>(null);
	/** Ids being cancelled, so a second click cannot send it twice. */
	cancelling = $state<string[]>([]);
	/** Text for the screen-reader live region of the list. */
	announcement = $state('');

	/** The dialog is open for this row. */
	target = $state<AlertTarget | null>(null);

	private ctrl: AbortController | null = null;
	private seen: string[] = [];

	get supported(): boolean {
		return this.phase !== 'unsupported';
	}

	/** Start refreshing and arm the audio unlock. Returns the stop function. */
	start(): () => void {
		void this.refresh();
		const timer = setInterval(() => void this.refresh(), REFRESH_MS);
		const unlock = armAudioUnlock();
		return () => {
			clearInterval(timer);
			unlock();
			this.ctrl?.abort();
		};
	}

	async refresh(): Promise<void> {
		if (this.phase === 'unsupported') return;
		this.ctrl?.abort();
		const ctrl = new AbortController();
		this.ctrl = ctrl;
		if (this.phase === 'idle') this.phase = 'loading';
		try {
			const res = await api.alerts(ctrl.signal);
			if (ctrl.signal.aborted) return;
			this.active = res.active ?? [];
			this.recent = res.recent ?? [];
			this.phase = 'ready';
			this.error = null;
		} catch (e) {
			const err = toApiError(e);
			if (err.code === 'aborted' || ctrl.signal.aborted) return;
			// A server without alerts answers 404 and no error body.
			if (err.status === 404 || err.code === 'unknown') this.phase = 'unsupported';
			else if (this.phase !== 'ready') {
				this.phase = 'error';
				this.error = describeError(err);
			}
		}
	}

	openFor(target: AlertTarget): void {
		this.target = target;
	}

	closeDialog(): void {
		this.target = null;
	}

	/** Throws ApiError so the dialog can show the reason where the user is looking. */
	async create(body: AlertCreateBody): Promise<PriceAlert> {
		try {
			const made = await api.createAlert(body);
			// Show it at once; the refresh confirms what the server holds.
			this.active = [made, ...this.active.filter((a) => a.id !== made.id)];
			this.announcement = `Alert set: ${alertTitle(made, digitsForAny(made.symbol))}.`;
			void this.refresh();
			return made;
		} catch (e) {
			throw toApiError(e);
		}
	}

	async cancel(a: PriceAlert): Promise<void> {
		if (this.cancelling.includes(a.id)) return;
		this.cancelling = [...this.cancelling, a.id];
		try {
			await api.cancelAlert(a.id);
			this.active = this.active.filter((x) => x.id !== a.id);
			this.announcement = `Alert cancelled: ${alertTitle(a, digitsForAny(a.symbol))}.`;
		} catch (e) {
			const err = toApiError(e);
			// Already gone (it fired a moment ago, or was cancelled elsewhere): the refresh shows where it went.
			if (err.status !== 404) toasts.error(`Could not cancel the alert. ${describeError(err)}`);
		} finally {
			this.cancelling = this.cancelling.filter((id) => id !== a.id);
			void this.refresh();
		}
	}

	/** `alert_triggered` over the WebSocket. */
	onTriggered(e: AlertTriggeredEvent): void {
		if (this.seen.includes(e.id)) return;
		this.seen = [...this.seen, e.id].slice(-SEEN_MAX);
		toasts.alert(toastText(e, digitsForAny(e.symbol)));
		// A chime or a spoken line over the user's own voice would be heard by the microphone.
		if (orb.state !== 'listening') {
			playChime(runningAudio());
			if (speech.readAloud && speech.available) void speech.speak(spokenLine(e));
		}
		void this.refresh();
	}

	/** Sign-out: the next user must not see this user's alerts. */
	reset(): void {
		this.ctrl?.abort();
		this.active = [];
		this.recent = [];
		this.phase = 'idle';
		this.error = null;
		this.target = null;
		this.seen = [];
	}
}

export const alerts = new AlertsStore();
