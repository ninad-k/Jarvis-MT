import { api, type SymbolsResponse } from '#lib/api.ts';
import { describeError, toApiError } from '#lib/errors.ts';
import { toasts } from './toasts.svelte';

export type SymbolsLoad = 'idle' | 'loading' | 'ready' | 'unsupported' | 'error';

/**
 * The broker symbol map (`GET /api/symbols`) for the settings section, plus the one place that
 * remembers which broker the app last saw, so a `symbol_map_changed` event for a different broker
 * can say so. A 404 means an older server without symbol mapping: the section hides itself.
 */
class SymbolMapStore {
	data = $state<SymbolsResponse | null>(null);
	load = $state<SymbolsLoad>('idle');
	error = $state<string | null>(null);
	/** A re-detection is running. */
	detecting = $state(false);

	/** Company name of the broker last seen (status poll, this store or an event). */
	private company: string | null = null;
	private ctrl: AbortController | null = null;

	/** Remember the broker the first time it is seen; later changes arrive as events. */
	noteCompany(company: string | null | undefined): void {
		if (company && this.company === null) this.company = company;
	}

	async refresh(initial = false): Promise<void> {
		if (this.detecting) return;
		this.ctrl?.abort();
		const ctrl = new AbortController();
		this.ctrl = ctrl;
		if (initial || this.data === null) this.load = 'loading';
		try {
			const next = await api.symbols(ctrl.signal);
			if (ctrl.signal.aborted) return;
			this.data = next;
			this.load = 'ready';
			this.error = null;
			this.noteCompany(next.broker?.company);
		} catch (e) {
			const err = toApiError(e);
			if (err.code === 'aborted') return;
			if (err.status === 404 || err.code === 'unknown') {
				if (this.data === null) this.load = 'unsupported';
			} else if (this.data === null) {
				this.load = 'error';
				this.error = describeError(err);
			}
		}
	}

	/** Re-run detection on the server. Throws the ApiError so the caller can show it. */
	async detect(): Promise<SymbolsResponse> {
		this.detecting = true;
		this.ctrl?.abort();
		try {
			const next = await api.detectSymbols();
			this.data = next;
			this.load = 'ready';
			this.error = null;
			this.noteCompany(next.broker?.company);
			return next;
		} catch (e) {
			throw toApiError(e);
		} finally {
			this.detecting = false;
		}
	}

	/** `symbol_map_changed`: a new broker, a re-detection or an override. Refreshes what is on screen. */
	onChanged(company: string | undefined): void {
		const before = this.company;
		if (company) this.company = company;
		if (before && company && before !== company) {
			toasts.push(`Broker changed to ${company} — symbols re-mapped`);
		}
		if (this.load !== 'idle') void this.refresh();
	}
}

export const symbolMap = new SymbolMapStore();
