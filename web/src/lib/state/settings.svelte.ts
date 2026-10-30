import { api, type Settings, type SettingsPatch } from '#lib/api.ts';
import { toApiError } from '#lib/errors.ts';

export type SettingsLoad = 'idle' | 'loading' | 'ready' | 'unsupported' | 'error';

/**
 * The signed-in user's preferences (`GET/PUT /api/settings`). `unsupported` means the server
 * answered 404: an older build without settings, so every setting-driven control hides itself.
 */
class SettingsStore {
	data = $state<Settings | null>(null);
	load = $state<SettingsLoad>('idle');
	error = $state<string | null>(null);

	/** Writes in flight. A refresh that arrives meanwhile would show stale data, so it waits. */
	private saving = 0;
	private refreshAfter = false;
	private ctrl: AbortController | null = null;

	get ready(): boolean {
		return this.data !== null;
	}

	/** Load once. Safe to call from several pages. */
	async ensureLoaded(): Promise<void> {
		if (this.load === 'ready' || this.load === 'loading' || this.load === 'unsupported') return;
		await this.refresh(true);
	}

	/** Re-read from the server (page load, a `settings_changed` event, after a wake-word switch). */
	async refresh(initial = false): Promise<void> {
		if (this.saving > 0) {
			this.refreshAfter = true;
			return;
		}
		this.ctrl?.abort();
		const ctrl = new AbortController();
		this.ctrl = ctrl;
		if (initial || this.data === null) this.load = 'loading';
		try {
			const next = await api.settings(ctrl.signal);
			if (this.saving > 0 || ctrl.signal.aborted) return;
			this.data = next;
			this.load = 'ready';
			this.error = null;
		} catch (e) {
			const err = toApiError(e);
			if (err.code === 'aborted') return;
			if (err.status === 404 || err.code === 'unknown') {
				if (this.data === null) this.load = 'unsupported';
			} else if (this.data === null) {
				this.load = 'error';
				this.error = err.message;
			}
		}
	}

	/**
	 * Save a partial update. With `optimistic` the change shows at once and is rolled back if the
	 * server refuses. Throws ApiError either way, so the caller can show the message where it belongs.
	 */
	async update(patch: SettingsPatch, opts: { optimistic?: boolean } = {}): Promise<Settings> {
		const before = this.data;
		this.saving++;
		this.ctrl?.abort();
		if (opts.optimistic && before) this.data = { ...before, ...patch };
		try {
			const next = await api.updateSettings(patch);
			this.data = next;
			this.load = 'ready';
			return next;
		} catch (e) {
			if (opts.optimistic) this.data = before;
			throw toApiError(e);
		} finally {
			this.saving--;
			if (this.saving === 0 && this.refreshAfter) {
				this.refreshAfter = false;
				void this.refresh();
			}
		}
	}

	/** Adopt a value the server told us about in another response (a wake-word persona switch). */
	adopt(patch: Partial<Settings>): void {
		if (this.data) this.data = { ...this.data, ...patch };
	}

	/** Sign-out: the next user must not see this user's settings. */
	reset(): void {
		this.ctrl?.abort();
		this.data = null;
		this.load = 'idle';
		this.error = null;
	}
}

export const settings = new SettingsStore();
